use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::{HashSet, VecDeque};
use ed25519_dalek::SigningKey;
use crate::{Opts, Message, UdpSocketExt};
use crate::identity::{load_or_generate_keypair, sign, verify, fingerprint};
use crate::wallet::{load_or_generate_btc_key, btc_address, load_or_create_wallet, save_wallet, debit, credit};
use crate::app_state::{AppState, ChatMessage, SseEvent, UiCommand};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

// Mémorise les identifiants déjà traités pour dédupliquer les messages
// reçus en double quand le client est connecté à plusieurs relais.
struct SeenIds {
    set: HashSet<u64>,
    queue: VecDeque<u64>,
    max: usize,
}

impl SeenIds {
    fn new(max: usize) -> Self {
        SeenIds { set: HashSet::new(), queue: VecDeque::new(), max }
    }
    // Retourne true si l'id est nouveau (et l'enregistre), false s'il était déjà vu.
    fn register(&mut self, id: u64) -> bool {
        if self.set.contains(&id) { return false; }
        if self.queue.len() >= self.max {
            if let Some(old) = self.queue.pop_front() { self.set.remove(&old); }
        }
        self.set.insert(id);
        self.queue.push_back(id);
        true
    }
}

pub async fn main_client(opts: Opts) {
    // Accepte une liste d'adresses comma-séparées : "1.2.3.4:5678,9.10.11.12:5678"
    let relay_addrs: Vec<SocketAddr> = opts.relay_addr.expect("--relay-addr required")
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse().expect("Wrong address format"))
        .collect();

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

    // Enregistrement auprès de tous les relais
    for &relay_addr in &relay_addrs {
        let msg = Message::Register {
            src_addr: local_addr,
            src_id: peer_id.clone(),
            dst_addr: relay_addr,
            dst_id: "relay".to_string(),
            time: now(),
        };
        socket.send_msg(&msg, relay_addr).await.unwrap();
    }
    println!("Connecté en tant que '{}' (empreinte: {}) auprès de {} relai(s)",
             peer_id, my_fingerprint, relay_addrs.len());
    println!("Commandes CLI : /pay <peer_id> <montant>  |  /balance");

    // Heartbeat : ré-enregistrement toutes les 30 s auprès de tous les relais
    let socket_hb = Arc::clone(&socket);
    let peer_id_hb = peer_id.clone();
    let relay_addrs_hb = relay_addrs.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            for &relay_addr in &relay_addrs_hb {
                let msg = Message::Register {
                    src_addr: local_addr,
                    src_id: peer_id_hb.clone(),
                    dst_addr: relay_addr,
                    dst_id: "relay".to_string(),
                    time: now(),
                };
                let _ = socket_hb.send_msg(&msg, relay_addr).await;
            }
        }
    });

    // Déduplication des messages reçus (un même message peut arriver de plusieurs relais)
    let seen_ids = Arc::new(Mutex::new(SeenIds::new(10_000)));

    // Réception des messages en arrière-plan
    let socket_rx = Arc::clone(&socket);
    let peer_id_rx = peer_id.clone();
    let fingerprint_rx = my_fingerprint.clone();
    let state_rx = Arc::clone(&state);
    let relay_addrs_rx = relay_addrs.clone();
    let seen_rx = Arc::clone(&seen_ids);
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
                        &relay_addrs_rx,
                        &seen_rx,
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
                                &relay_addrs, local_addr, &state,
                            ).await;
                        }
                    }
                    Ok(Some(line)) => {
                        send_classic_message(
                            &line, &peer_id, &socket, &relay_addrs,
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
                            &text, &peer_id, &socket, &relay_addrs,
                            local_addr, &signing_key, &public_key,
                        ).await;
                    }
                    Some(UiCommand::Pay { dst_id, amount }) => {
                        let args = format!("{} {}", dst_id, amount);
                        handle_pay_command(
                            &args, &peer_id, &socket,
                            &relay_addrs, local_addr, &state,
                        ).await;
                    }
                    None => break,
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────
// Envoi d'un message à tous les relais
// ────────────────────────────────────────────────────────────
async fn send_to_all_relays(socket: &UdpSocket, msg: &Message, relay_addrs: &[SocketAddr]) {
    for &addr in relay_addrs {
        let _ = socket.send_msg(msg, addr).await;
    }
}

// ────────────────────────────────────────────────────────────
// Envoi d'un message Classic signé vers tous les relais
// ────────────────────────────────────────────────────────────
async fn send_classic_message(
    text: &str,
    peer_id: &str,
    socket: &UdpSocket,
    relay_addrs: &[SocketAddr],
    local_addr: SocketAddr,
    signing_key: &SigningKey,
    public_key: &[u8],
) {
    let dst_addr = relay_addrs.first().copied().unwrap_or_else(|| "0.0.0.0:0".parse().unwrap());
    let time_val = now();
    let msg_id: u64 = rand::random();
    let sig = sign(signing_key, peer_id, text, time_val, msg_id);
    let msg = Message::Classic {
        src_addr: local_addr,
        src_id: peer_id.to_string(),
        dst_addr,
        dst_id: "all".to_string(),
        txt: text.to_string(),
        time: time_val,
        msg_id,
        ttl: 16,
        public_key: public_key.to_vec(),
        signature: sig,
    };
    send_to_all_relays(socket, &msg, relay_addrs).await;
}

// ────────────────────────────────────────────────────────────
// Envoi d'un paiement vers tous les relais
// ────────────────────────────────────────────────────────────
async fn handle_pay_command(
    args: &str,
    my_id: &str,
    socket: &UdpSocket,
    relay_addrs: &[SocketAddr],
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

    // Envoi vers tous les relais ; on crédite en retour seulement si tous échouent
    let mut any_success = false;
    for &relay_addr in relay_addrs {
        match socket.send_msg(&msg, relay_addr).await {
            Ok(_) => any_success = true,
            Err(e) => eprintln!("Erreur d'envoi vers {} : {}", relay_addr, e),
        }
    }

    if any_success {
        println!("[Payment #{payment_id}] {amount} sats envoyés à {dst_id} (en attente de confirmation)");
        state.push_message(ChatMessage {
            from_id: dst_id.to_string(),
            fingerprint: String::new(),
            text: amount.to_string(),
            timestamp: now(),
            msg_type: "payment_sent".to_string(),
        }).await;
        state.broadcast_wallet_update().await;
    } else {
        eprintln!("Échec d'envoi du paiement sur tous les relais");
        let mut w = state.wallet.lock().await;
        w.balance += amount;
        save_wallet(my_id, &w);
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
    relay_addrs: &[SocketAddr],
    seen_ids: &Mutex<SeenIds>,
    state: &Arc<AppState>,
) {
    match msg {
        Message::Classic { src_id, txt, time, msg_id, public_key, signature, .. } => {
            // Déduplication : ignorer si déjà reçu via un autre relai
            if !seen_ids.lock().await.register(msg_id) { return; }

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
            // Déduplication : un même paiement ne doit être crédité qu'une fois
            // On utilise payment_id ^ u64::MAX pour éviter toute collision avec les msg_id Classic
            if !seen_ids.lock().await.register(payment_id ^ u64::MAX) { return; }

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

            // Accusé de réception propagé sur tous les relais
            let ack = Message::PaymentAck {
                payment_id,
                from_id: my_id.to_string(),
                to_id: src_id,
            };
            send_to_all_relays(socket, &ack, relay_addrs).await;
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
