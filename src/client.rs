use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use ed25519_dalek::SigningKey;
use crate::{Opts, Message, UdpSocketExt};
use crate::identity::{load_or_generate_keypair, sign, verify, fingerprint};
use crate::wallet::{load_or_generate_btc_key, btc_address, load_or_create_wallet, save_wallet, debit, credit};
use crate::app_state::{AppState, ChatMessage, SseEvent, UiCommand};

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
    let my_fingerprint = fingerprint(&public_key);

    // Chargement ou génération du keypair secp256k1 (convention Bitcoin, paiements)
    let btc_key = load_or_generate_btc_key(&peer_id);
    let btc_pub = btc_key.public_key();
    let address = btc_address(&btc_pub);

    // Chargement ou création du portefeuille local
    let wallet = Arc::new(Mutex::new(load_or_create_wallet(&peer_id, &address)));

    println!("Bitcoin address : {}", address);
    println!("Balance         : {} sats", wallet.lock().await.balance);

    // Création de l'état partagé entre UI web et boucle P2P
    let (events_tx, _) = broadcast::channel::<SseEvent>(256);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<UiCommand>(64);

    let state = Arc::new(AppState {
        node_id: peer_id.clone(),
        fingerprint: my_fingerprint.clone(),
        btc_address: address.clone(),
        messages: RwLock::new(Vec::new()),
        wallet: Arc::clone(&wallet),
        events_tx,
        cmd_tx,
    });

    // Démarrage du serveur web en arrière-plan
    let state_web = Arc::clone(&state);
    tokio::spawn(async move {
        crate::web::start_server(state_web, 8080).await;
    });

    // Enregistrement auprès du relai
    let msg = Message::Register {
        src_addr: local_addr,
        src_id: peer_id.clone(),
        dst_addr: relay_addr,
        dst_id: "relay".to_string(),
        time: now(),
    };
    socket.send_msg(&msg, relay_addr).await.unwrap();
    println!("Connecté en tant que '{}' (empreinte: {})", peer_id, my_fingerprint);
    println!("Commandes CLI : /pay <peer_id> <montant>  |  /balance");

    // Heartbeat : ré-enregistrement toutes les 30 s
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
    let fingerprint_rx = my_fingerprint.clone();
    let state_rx = Arc::clone(&state);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            if let Ok((size, _)) = socket_rx.recv_from(&mut buf).await {
                if let Ok(msg) = bincode::deserialize::<Message>(&buf[..size]) {
                    handle_incoming(
                        msg,
                        &peer_id_rx,
                        &fingerprint_rx,
                        &socket_rx,
                        relay_addr,
                        &state_rx,
                    ).await;
                }
            }
        }
    });

    // Boucle principale : stdin ET commandes web UI via cmd_rx
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        tokio::select! {
            // ── Entrée clavier ──
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) if line.starts_with("/balance") => {
                        let w = state.wallet.lock().await;
                        println!("Balance : {} sats  ({})", w.balance, w.address);
                    }
                    Ok(Some(ref line)) if line.starts_with("/pay ") => {
                        if let Some(args) = line.strip_prefix("/pay ") {
                            handle_pay_command(
                                args, &peer_id, &socket,
                                relay_addr, local_addr, &state,
                            ).await;
                        }
                    }
                    Ok(Some(line)) => {
                        send_classic_message(
                            &line, &peer_id, &socket, relay_addr,
                            local_addr, &signing_key, &public_key,
                        ).await;
                    }
                    _ => break,
                }
            }
            // ── Commandes depuis l'interface web ──
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(UiCommand::SendMessage(text)) => {
                        send_classic_message(
                            &text, &peer_id, &socket, relay_addr,
                            local_addr, &signing_key, &public_key,
                        ).await;
                    }
                    Some(UiCommand::Pay { dst_id, amount }) => {
                        let args = format!("{} {}", dst_id, amount);
                        handle_pay_command(
                            &args, &peer_id, &socket,
                            relay_addr, local_addr, &state,
                        ).await;
                    }
                    None => break,
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────
// Envoi d'un message Classic signé
// ────────────────────────────────────────────────────────────
async fn send_classic_message(
    text: &str,
    peer_id: &str,
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    local_addr: SocketAddr,
    signing_key: &SigningKey,
    public_key: &[u8],
) {
    let time_val = now();
    let msg_id: u64 = rand::random();
    let sig = sign(signing_key, peer_id, text, time_val, msg_id);
    let msg = Message::Classic {
        src_addr: local_addr,
        src_id: peer_id.to_string(),
        dst_addr: relay_addr,
        dst_id: "all".to_string(),
        txt: text.to_string(),
        time: time_val,
        msg_id,
        ttl: 16,
        public_key: public_key.to_vec(),
        signature: sig,
    };
    let _ = socket.send_msg(&msg, relay_addr).await;
}

// ────────────────────────────────────────────────────────────
// Envoi d'un paiement
// ────────────────────────────────────────────────────────────
async fn handle_pay_command(
    args: &str,
    my_id: &str,
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    _local_addr: SocketAddr,
    state: &Arc<AppState>,
) {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() != 2 {
        eprintln!("Usage: /pay <peer_id> <montant>");
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

    {
        let mut w = state.wallet.lock().await;
        if !debit(&mut w, amount) {
            eprintln!("Solde insuffisant ({} sats)", w.balance);
            return;
        }
        save_wallet(my_id, &w);
    }

    let payment_id: u64 = rand::random();
    let msg = Message::Payment {
        src_id: my_id.to_string(),
        dst_id: dst_id.to_string(),
        amount,
        payment_id,
    };
    match socket.send_msg(&msg, relay_addr).await {
        Ok(_) => {
            println!("[Payment #{payment_id}] {amount} sats envoyés à {dst_id} (en attente de confirmation)");
            // Ajouter l'événement dans le fil UI
            state.push_message(ChatMessage {
                from_id: dst_id.to_string(),
                fingerprint: String::new(),
                text: amount.to_string(),
                timestamp: now(),
                msg_type: "payment_sent".to_string(),
            }).await;
            state.broadcast_wallet_update().await;
        }
        Err(e) => {
            eprintln!("Erreur d'envoi : {}", e);
            // Recréditer en cas d'échec réseau
            let mut w = state.wallet.lock().await;
            w.balance += amount;
            save_wallet(my_id, &w);
        }
    }
}

// ────────────────────────────────────────────────────────────
// Traitement des messages entrants
// ────────────────────────────────────────────────────────────
async fn handle_incoming(
    msg: Message,
    my_id: &str,
    my_fingerprint: &str,
    socket: &UdpSocket,
    relay_addr: SocketAddr,
    state: &Arc<AppState>,
) {
    match msg {
        Message::Classic { src_id, txt, time, msg_id, public_key, signature, .. } => {
            if verify(&public_key, &src_id, &txt, time, msg_id, &signature) {
                let fp = fingerprint(&public_key);
                println!("[{} - {}] {}", src_id, fp, txt);
                state.push_message(ChatMessage {
                    from_id: src_id,
                    fingerprint: fp,
                    text: txt,
                    timestamp: time,
                    msg_type: "message".to_string(),
                }).await;
            } else {
                eprintln!("[WARN] Message de '{}' — signature invalide, ignoré", src_id);
            }
        }

        Message::Payment { src_id, dst_id, amount, payment_id } => {
            if dst_id != my_id && dst_id != my_fingerprint {
                return;
            }
            {
                let mut w = state.wallet.lock().await;
                credit(&mut w, amount);
                println!("[Payment #{payment_id}] +{amount} sats reçus de {src_id} (balance: {} sats)", w.balance);
                save_wallet(my_id, &w);
            }
            state.push_message(ChatMessage {
                from_id: src_id.clone(),
                fingerprint: String::new(),
                text: amount.to_string(),
                timestamp: now(),
                msg_type: "payment_received".to_string(),
            }).await;
            state.broadcast_wallet_update().await;

            // Accusé de réception
            let ack = Message::PaymentAck {
                payment_id,
                from_id: my_id.to_string(),
                to_id: src_id,
            };
            let _ = socket.send_msg(&ack, relay_addr).await;
        }

        Message::PaymentAck { payment_id, from_id, to_id } => {
            if to_id != my_id && to_id != my_fingerprint {
                return;
            }
            {
                let w = state.wallet.lock().await;
                save_wallet(my_id, &w);
                println!("[Payment #{payment_id}] Confirmé par {from_id} (balance: {} sats)", w.balance);
            }
            state.push_message(ChatMessage {
                from_id: from_id.clone(),
                fingerprint: String::new(),
                text: String::new(),
                timestamp: now(),
                msg_type: "payment_ack".to_string(),
            }).await;
        }

        _ => {}
    }
}
