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

/// Cache de déduplication des messages
struct SeenIds {
    set: HashSet<u64>,
    queue: VecDeque<u64>,
    max: usize,
}

impl SeenIds {
    fn new(max: usize) -> Self {
        Self { set: HashSet::new(), queue: VecDeque::new(), max }
    }

    fn is_new(&mut self, id: u64) -> bool {
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
    let relay_addrs: Vec<SocketAddr> = opts.relay_addr
        .expect("--relay-addr required")
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse().expect("Invalid address"))
        .collect();

    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await.expect("Bind failed"));
    let local_addr = socket.local_addr().unwrap();
    let peer_id = opts.peer_id;

    // Clés cryptographiques
    let signing_key = load_or_generate_keypair(&peer_id);
    let public_key = signing_key.verifying_key().to_bytes().to_vec();
    let my_fp = fingerprint(&public_key);

    let btc_key = load_or_generate_btc_key(&peer_id);
    let my_btc = btc_address(&btc_key.public_key());

    let wallet = Arc::new(Mutex::new(load_or_create_wallet(&peer_id, &my_btc)));

    println!("Bitcoin address : {}", my_btc);
    println!("Balance         : {} sats", wallet.lock().await.balance);

    // État partagé avec l'interface web
    let (events_tx, _) = broadcast::channel::<SseEvent>(256);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<UiCommand>(64);

    let state = Arc::new(AppState {
        node_id: peer_id.clone(),
        fingerprint: my_fp.clone(),
        btc_address: my_btc.clone(),
        messages: RwLock::new(Vec::new()),
        wallet: Arc::clone(&wallet),
        events_tx,
        cmd_tx,
    });

    // Serveur web
    let state_web = Arc::clone(&state);
    tokio::spawn(async move {
        crate::web::start_server(state_web, 8080).await;
    });

    // Enregistrement auprès des relais
    register(&socket, &peer_id, &my_btc, local_addr, &relay_addrs).await;
    println!("Connected as '{}' (fp: {}) to {} relay(s)", peer_id, my_fp, relay_addrs.len());

    // Heartbeat
    spawn_heartbeat(Arc::clone(&socket), peer_id.clone(), my_btc.clone(), local_addr, relay_addrs.clone());

    let seen = Arc::new(Mutex::new(SeenIds::new(10_000)));

    // Réception des messages
    let ctx = ClientContext {
        peer_id: peer_id.clone(),
        my_fp: my_fp.clone(),
        my_btc: my_btc.clone(),
        socket: Arc::clone(&socket),
        relay_addrs: relay_addrs.clone(),
        seen: Arc::clone(&seen),
        state: Arc::clone(&state),
    };
    tokio::spawn(recv_loop(ctx));

    // Boucle principale: stdin + commandes web
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) if line.starts_with("/balance") => {
                        let w = state.wallet.lock().await;
                        println!("Balance: {} sats ({})", w.balance, w.address);
                    }
                    Ok(Some(line)) if line.starts_with("/pay ") => {
                        if let Some(args) = line.strip_prefix("/pay ") {
                            do_pay(args, &peer_id, &socket, &relay_addrs, &state).await;
                        }
                    }
                    Ok(Some(line)) => {
                        send_msg(&line, &peer_id, &socket, &relay_addrs, local_addr, &signing_key, &public_key, &state).await;
                    }
                    _ => break,
                }
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(UiCommand::SendMessage(text)) => {
                        send_msg(&text, &peer_id, &socket, &relay_addrs, local_addr, &signing_key, &public_key, &state).await;
                    }
                    Some(UiCommand::Pay { dst_id, amount }) => {
                        let args = format!("{} {}", dst_id, amount);
                        do_pay(&args, &peer_id, &socket, &relay_addrs, &state).await;
                    }
                    None => break,
                }
            }
        }
    }
}

struct ClientContext {
    peer_id: String,
    my_fp: String,
    my_btc: String,
    socket: Arc<UdpSocket>,
    relay_addrs: Vec<SocketAddr>,
    seen: Arc<Mutex<SeenIds>>,
    state: Arc<AppState>,
}

async fn recv_loop(ctx: ClientContext) {
    let mut buf = vec![0u8; 65535];
    loop {
        let (size, _) = match ctx.socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => { eprintln!("[ERROR] recv: {}", e); continue; }
        };

        let msg: Message = match bincode::deserialize(&buf[..size]) {
            Ok(m) => m,
            Err(e) => { eprintln!("[ERROR] Deserialize: {}", e); continue; }
        };

        handle_msg(msg, &ctx).await;
    }
}

async fn handle_msg(msg: Message, ctx: &ClientContext) {
    match msg {
        Message::Classic { src_id, txt, time, msg_id, public_key, signature, .. } => {
            if !ctx.seen.lock().await.is_new(msg_id) { return; }

            if !verify(&public_key, &src_id, &txt, time, msg_id, &signature) {
                eprintln!("[WARN] Invalid signature from '{}'", src_id);
                return;
            }

            let fp = fingerprint(&public_key);
            println!("[{} - {}] {}", src_id, fp, txt);

            ctx.state.push_message(ChatMessage {
                from_id: src_id,
                fingerprint: fp,
                text: txt,
                timestamp: time,
                msg_type: "message".to_string(),
            }).await;
        }

        Message::Payment { src_id, dst_id, amount, payment_id } => {
            // Suis-je le destinataire ?
            if dst_id != ctx.peer_id && dst_id != ctx.my_fp && dst_id != ctx.my_btc {
                return;
            }

            // Déduplication (utilise un offset pour éviter collision avec msg_id)
            if !ctx.seen.lock().await.is_new(payment_id ^ 0xFFFF_FFFF_0000_0000) { return; }

            {
                let mut w = ctx.state.wallet.lock().await;
                credit(&mut w, amount);
                println!("[Payment #{}] +{} sats from {} (balance: {})", payment_id, amount, src_id, w.balance);
                save_wallet(&ctx.peer_id, &w);
            }

            ctx.state.push_message(ChatMessage {
                from_id: src_id.clone(),
                fingerprint: String::new(),
                text: amount.to_string(),
                timestamp: now(),
                msg_type: "payment_received".to_string(),
            }).await;
            ctx.state.broadcast_wallet_update().await;

            // Envoyer l'accusé de réception
            let ack = Message::PaymentAck {
                payment_id,
                from_id: ctx.peer_id.clone(),
                to_id: src_id,
            };
            send_to_relays(&ctx.socket, &ack, &ctx.relay_addrs).await;
        }

        Message::PaymentAck { payment_id, from_id, to_id } => {
            // Suis-je l'émetteur du paiement ?
            if to_id != ctx.peer_id && to_id != ctx.my_fp && to_id != ctx.my_btc {
                return;
            }

            // Déduplication
            if !ctx.seen.lock().await.is_new(payment_id ^ 0xFFFF_0000_FFFF_0000) { return; }

            println!("[PaymentAck #{}] Confirmed by {}", payment_id, from_id);

            ctx.state.push_message(ChatMessage {
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

async fn send_to_relays(socket: &UdpSocket, msg: &Message, relays: &[SocketAddr]) {
    for &addr in relays {
        let _ = socket.send_msg(msg, addr).await;
    }
}

async fn send_msg(
    text: &str,
    peer_id: &str,
    socket: &UdpSocket,
    relays: &[SocketAddr],
    local_addr: SocketAddr,
    signing_key: &SigningKey,
    public_key: &[u8],
    state: &Arc<AppState>,
) {
    let time = now();
    let msg_id: u64 = rand::random();
    let sig = sign(signing_key, peer_id, text, time, msg_id);

    let msg = Message::Classic {
        src_addr: local_addr,
        src_id: peer_id.to_string(),
        dst_addr: relays.first().copied().unwrap_or("0.0.0.0:0".parse().unwrap()),
        dst_id: "all".to_string(),
        txt: text.to_string(),
        time,
        msg_id,
        ttl: 16,
        public_key: public_key.to_vec(),
        signature: sig,
    };

    send_to_relays(socket, &msg, relays).await;

    let fp = fingerprint(public_key);
    state.push_message(ChatMessage {
        from_id: format!("{} (vous)", peer_id),
        fingerprint: fp,
        text: text.to_string(),
        timestamp: time,
        msg_type: "message".to_string(),
    }).await;
}

async fn do_pay(args: &str, my_id: &str, socket: &UdpSocket, relays: &[SocketAddr], state: &Arc<AppState>) {
    let parts: Vec<&str> = args.splitn(2, ' ').collect();
    if parts.len() != 2 {
        eprintln!("Usage: /pay <address> <amount>");
        return;
    }

    let dst = parts[0].trim();
    let amount: u64 = match parts[1].trim().parse() {
        Ok(n) if n > 0 => n,
        _ => { eprintln!("Invalid amount"); return; }
    };

    {
        let mut w = state.wallet.lock().await;
        if !debit(&mut w, amount) {
            eprintln!("Insufficient balance ({} sats)", w.balance);
            return;
        }
        save_wallet(my_id, &w);
    }

    let payment_id: u64 = rand::random();
    let msg = Message::Payment {
        src_id: my_id.to_string(),
        dst_id: dst.to_string(),
        amount,
        payment_id,
    };

    send_to_relays(socket, &msg, relays).await;

    println!("[Payment #{}] {} sats sent to {}", payment_id, amount, dst);

    state.push_message(ChatMessage {
        from_id: format!("{} (vous)", my_id),
        fingerprint: String::new(),
        text: format!("{} → {}", amount, dst),
        timestamp: now(),
        msg_type: "payment_sent".to_string(),
    }).await;
    state.broadcast_wallet_update().await;
}

async fn register(socket: &UdpSocket, peer_id: &str, btc: &str, local: SocketAddr, relays: &[SocketAddr]) {
    for &relay in relays {
        let msg = Message::Register {
            src_addr: local,
            src_id: peer_id.to_string(),
            dst_addr: relay,
            dst_id: "relay".to_string(),
            time: now(),
            btc_address: Some(btc.to_string()),
        };
        let _ = socket.send_msg(&msg, relay).await;
    }
}

fn spawn_heartbeat(socket: Arc<UdpSocket>, peer_id: String, btc: String, local: SocketAddr, relays: Vec<SocketAddr>) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            for &relay in &relays {
                let msg = Message::Register {
                    src_addr: local,
                    src_id: peer_id.clone(),
                    dst_addr: relay,
                    dst_id: "relay".to_string(),
                    time: now(),
                    btc_address: Some(btc.clone()),
                };
                let _ = socket.send_msg(&msg, relay).await;
            }
        }
    });
}
