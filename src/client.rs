use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::{Opts, Message, UdpSocketExt};
use crate::identity::{load_or_generate_keypair, sign, verify, fingerprint};
use crate::wallet::{load_or_generate_btc_key, btc_address, load_or_create_wallet, save_wallet, debit, credit, Wallet};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub async fn main_client(opts: Opts) {
    let relay_addr: SocketAddr = opts.relay_addr.expect("--relay-addr required")
        .parse().expect("Wrong address format");
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await.expect("Failed to bind"));
    let local_addr = socket.local_addr().unwrap();
    let peer_id = opts.peer_id;

    // Chargement ou génération du couple de clés ed25519 (identité/signature)
    let signing_key = load_or_generate_keypair(&peer_id);
    let public_key = signing_key.verifying_key().to_bytes().to_vec();

    // Chargement ou génération du keypair secp256k1 (convention Bitcoin, paiements)
    let btc_key = load_or_generate_btc_key(&peer_id);
    let btc_pub = btc_key.public_key();
    let address = btc_address(&btc_pub);

    // Chargement ou création du portefeuille local
    let wallet = Arc::new(Mutex::new(load_or_create_wallet(&peer_id, &address)));
    println!("Bitcoin address : {}", address);
    println!("Balance         : {} sats", wallet.lock().await.balance);

    // Enregistrement auprès du relai
    let msg = Message::Register {
        src_addr: local_addr,
        src_id: peer_id.clone(),
        dst_addr: relay_addr,
        dst_id: "relay".to_string(),
        time: now(),
    };
    socket.send_msg(&msg, relay_addr).await.unwrap();
    println!("Connected as '{}'", peer_id);
    println!("Commands: /pay <peer_id> <amount>  |  /balance");

    // Heartbeat : re-enregistrement toutes les 30s pour rester dans la liste du relai
    let socket_hb = Arc::clone(&socket);
    let peer_id_hb = peer_id.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            let msg = Message::Register {
                src_addr: local_addr,
                src_id: peer_id_hb.clone(),
                dst_addr: relay_addr,
                dst_id: "relay".to_string(),
                time: now(),
            };
            let _ = socket_hb.send_msg(&msg, relay_addr).await;
        }
    });

    // Réception des messages en arrière-plan
    let socket_rx = Arc::clone(&socket);
    let peer_id_rx = peer_id.clone();
    let wallet_rx = Arc::clone(&wallet);
    tokio::spawn(async move {
        let mut buf = vec![0; 4096];
        loop {
            if let Ok((size, _)) = socket_rx.recv_from(&mut buf).await {
                if let Ok(msg) = bincode::deserialize::<Message>(&buf[..size]) {
                    handle_incoming(msg, &peer_id_rx, &socket_rx, relay_addr, &wallet_rx).await;
                }
            }
        }
    });

    // Boucle stdin : messages texte et commandes de paiement
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.starts_with("/balance") {
            let w = wallet.lock().await;
            println!("Balance : {} sats  ({})", w.balance, w.address);
        } else if let Some(args) = line.strip_prefix("/pay ") {
            handle_pay_command(args, &peer_id, &socket, relay_addr, local_addr, &wallet).await;
        } else {
            // Message texte classique
            let time_val = now();
            let msg_id: u64 = rand::random();
            let sig = sign(&signing_key, &peer_id, &line, time_val, msg_id);
            let msg = Message::Classic {
                src_addr: local_addr,
                src_id: peer_id.clone(),
                dst_addr: relay_addr,
                dst_id: "all".to_string(),
                txt: line,
                time: time_val,
                msg_id,
                ttl: 16,
                public_key: public_key.clone(),
                signature: sig,
            };
            let _ = socket.send_msg(&msg, relay_addr).await;
        }
    }
}

// Envoie un paiement : débite le wallet et envoie un message Payment au relai.
async fn handle_pay_command(
    args: &str,
    my_id: &str,
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    _local_addr: SocketAddr,
    wallet: &Arc<Mutex<Wallet>>,
) {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() != 2 {
        eprintln!("Usage: /pay <peer_id> <amount>");
        return;
    }
    let dst_id = parts[0].trim();
    let amount: u64 = match parts[1].trim().parse() {
        Ok(n) => n,
        Err(_) => { eprintln!("Montant invalide"); return; }
    };
    if amount == 0 {
        eprintln!("Montant doit être > 0");
        return;
    }

    let mut w = wallet.lock().await;
    if !debit(&mut w, amount) {
        eprintln!("Solde insuffisant ({} sats)", w.balance);
        return;
    }
    save_wallet(my_id, &w);
    drop(w);

    let payment_id: u64 = rand::random();
    let msg = Message::Payment {
        src_id: my_id.to_string(),
        dst_id: dst_id.to_string(),
        amount,
        payment_id,
    };
    match socket.send_msg(&msg, relay_addr).await {
        Ok(_) => println!("[Payment #{payment_id}] {amount} sats envoyés à {dst_id} (en attente de confirmation)"),
        Err(e) => {
            eprintln!("Erreur d'envoi : {}", e);
            // Recréditer en cas d'échec réseau
            wallet.lock().await.balance += amount;
        }
    }
}

// Traite un message entrant (Classic, Payment, PaymentAck).
async fn handle_incoming(
    msg: Message,
    my_id: &str,
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    wallet: &Arc<Mutex<Wallet>>,
) {
    match msg {
        Message::Classic { src_id, txt, time, msg_id, public_key, signature, .. } => {
            if verify(&public_key, &src_id, &txt, time, msg_id, &signature) {
                println!("[{} - {}] {}", src_id, fingerprint(&public_key), txt);
            } else {
                eprintln!("[WARN] Message de '{}' — signature invalide, ignoré", src_id);
            }
        }

        Message::Payment { src_id, dst_id, amount, payment_id } => {
            if dst_id != my_id {
                return; // Pas pour moi
            }
            // Créditer le wallet
            let mut w = wallet.lock().await;
            credit(&mut w, amount);
            println!("[Payment #{payment_id}] +{amount} sats reçus de {src_id} (balance: {} sats)", w.balance);
            save_wallet(my_id, &w);
            drop(w);

            // Envoyer un accusé de réception
            let ack = Message::PaymentAck {
                payment_id,
                from_id: my_id.to_string(),
                to_id: src_id,
            };
            let _ = socket.send_msg(&ack, relay_addr).await;
        }

        Message::PaymentAck { payment_id, from_id, to_id } => {
            if to_id != my_id {
                return; // Pas pour moi
            }
            // Sauvegarder l'état du wallet après confirmation
            let w = wallet.lock().await;
            save_wallet(my_id, &w);
            println!("[Payment #{payment_id}] Paiement confirmé par {from_id} (balance: {} sats)", w.balance);
        }

        _ => {}
    }
}
