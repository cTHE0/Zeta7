use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::sync::Arc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Message, UdpSocketExt, Opts};
use crate::identity::{load_or_generate_keypair, sign, verify, fingerprint};

/// Peer info: (peer_id, last_seen, btc_address)
type PeerInfo = (String, u64, Option<String>);
type PeersMap = Arc<Mutex<HashMap<SocketAddr, PeerInfo>>>;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Cache des msg_id déjà traités (évite les doublons)
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

pub async fn main_relay(opts: Opts) {
    let my_id = opts.peer_id.clone();
    let relay_id = format!("relay:{}", my_id);
    let port: u16 = 12345;

    let peer_relays: Vec<SocketAddr> = opts.peer_relays
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse().expect("Invalid relay address"))
        .collect();

    let socket = Arc::new(UdpSocket::bind(format!("0.0.0.0:{}", port)).await.expect("Bind failed"));
    println!("Relay '{}' listening on port {}...", my_id, port);

    let signing_key = load_or_generate_keypair(&my_id);
    let public_key = signing_key.verifying_key().to_bytes().to_vec();

    let peers: PeersMap = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::new(Mutex::new(SeenIds::new(10_000)));

    // Enregistrement initial auprès des relais pairs
    register_with_peers(&socket, &relay_id, port, &peer_relays).await;

    // Heartbeat toutes les 30s
    spawn_heartbeat(Arc::clone(&socket), relay_id.clone(), port, peer_relays.clone());

    // Nettoyage des peers inactifs
    spawn_cleanup(Arc::clone(&peers));

    // Boucle de réception
    let socket_rx = Arc::clone(&socket);
    let peers_rx = Arc::clone(&peers);
    let seen_rx = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let (size, sender) = match socket_rx.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(e) => { eprintln!("[ERROR] recv: {}", e); continue; }
            };

            let msg: Message = match bincode::deserialize(&buf[..size]) {
                Ok(m) => m,
                Err(e) => { eprintln!("[ERROR] Deserialize: {}", e); continue; }
            };

            handle_message(msg, sender, &peers_rx, &seen_rx, &socket_rx).await;
        }
    });

    // Console stdin pour envoyer des messages
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let msg_id: u64 = rand::random();
        let time = now();
        let sig = sign(&signing_key, &my_id, &line, time, msg_id);

        let msg = Message::Classic {
            src_addr: format!("0.0.0.0:{}", port).parse().unwrap(),
            src_id: my_id.clone(),
            dst_addr: "0.0.0.0:0".parse().unwrap(),
            dst_id: "all".to_string(),
            txt: line,
            time,
            msg_id,
            ttl: 16,
            public_key: public_key.clone(),
            signature: sig,
        };

        seen.lock().await.is_new(msg_id);
        broadcast(&peers, "0.0.0.0:0".parse().unwrap(), &msg, &socket).await;
    }
}

async fn handle_message(
    msg: Message,
    sender: SocketAddr,
    peers: &PeersMap,
    seen: &Mutex<SeenIds>,
    socket: &UdpSocket,
) {
    match &msg {
        Message::Register { src_id, time, btc_address, .. } => {
            let mut map = peers.lock().await;
            map.insert(sender, (src_id.clone(), *time, btc_address.clone()));
            if let Some(btc) = btc_address {
                println!("[+] '{}' registered ({}) [BTC: {}]", src_id, sender, btc);
            } else {
                println!("[+] '{}' registered ({})", src_id, sender);
            }
        }

        Message::Classic { src_id, txt, time, msg_id, ttl, public_key, signature, .. } => {
            if !seen.lock().await.is_new(*msg_id) { return; }

            if !verify(public_key, src_id, txt, *time, *msg_id, signature) {
                eprintln!("[WARN] Invalid signature from '{}'", src_id);
                return;
            }

            println!("[{} - {}] {}", src_id, fingerprint(public_key), txt);

            if *ttl > 0 {
                let mut fwd = msg.clone();
                if let Message::Classic { ttl: ref mut t, .. } = fwd { *t -= 1; }
                broadcast(peers, sender, &fwd, socket).await;
            }
        }

        Message::Payment { src_id, dst_id, amount, payment_id } => {
            println!("[Payment #{}] {} → {} : {} sats", payment_id, src_id, dst_id, amount);

            // Trouver le destinataire par btc_address ou peer_id
            let target = find_peer(peers, dst_id).await;

            if let Some(addr) = target {
                let _ = socket.send_msg(&msg, addr).await;
            } else {
                // Relayer à tous les autres (y compris relais fédérés)
                broadcast(peers, sender, &msg, socket).await;
            }
        }

        Message::PaymentAck { to_id, from_id, payment_id } => {
            println!("[PaymentAck #{}] {} → {}", payment_id, from_id, to_id);

            // Trouver l'émetteur original par btc_address ou peer_id
            let target = find_peer(peers, to_id).await;

            if let Some(addr) = target {
                let _ = socket.send_msg(&msg, addr).await;
            } else {
                broadcast(peers, sender, &msg, socket).await;
            }
        }

        Message::AskForAddr { src_addr, peer_id, .. } => {
            let map = peers.lock().await;
            if let Some((addr, _)) = map.iter().find(|(_, (id, _, _))| id == peer_id) {
                let reply = Message::PeerInfo { peer_addr: *addr, peer_id: peer_id.clone() };
                drop(map);
                let _ = socket.send_msg(&reply, *src_addr).await;
            }
        }

        _ => {}
    }
}

/// Trouve un peer par peer_id ou btc_address
async fn find_peer(peers: &PeersMap, id: &str) -> Option<SocketAddr> {
    let map = peers.lock().await;
    for (addr, (peer_id, _, btc)) in map.iter() {
        if peer_id == id { return Some(*addr); }
        if let Some(btc_addr) = btc {
            if btc_addr == id { return Some(*addr); }
        }
    }
    None
}

/// Broadcast à tous les peers sauf l'expéditeur
async fn broadcast(peers: &PeersMap, exclude: SocketAddr, msg: &Message, socket: &UdpSocket) {
    let map = peers.lock().await;
    for addr in map.keys() {
        if *addr != exclude {
            let _ = socket.send_msg(msg, *addr).await;
        }
    }
}

async fn register_with_peers(socket: &UdpSocket, relay_id: &str, port: u16, peer_relays: &[SocketAddr]) {
    for &addr in peer_relays {
        let msg = Message::Register {
            src_addr: format!("0.0.0.0:{}", port).parse().unwrap(),
            src_id: relay_id.to_string(),
            dst_addr: addr,
            dst_id: "relay".to_string(),
            time: now(),
            btc_address: None,
        };
        let _ = socket.send_msg(&msg, addr).await;
        println!("Registered with peer relay {}", addr);
    }
}

fn spawn_heartbeat(socket: Arc<UdpSocket>, relay_id: String, port: u16, peer_relays: Vec<SocketAddr>) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            for &addr in &peer_relays {
                let msg = Message::Register {
                    src_addr: format!("0.0.0.0:{}", port).parse().unwrap(),
                    src_id: relay_id.clone(),
                    dst_addr: addr,
                    dst_id: "relay".to_string(),
                    time: now(),
                    btc_address: None,
                };
                let _ = socket.send_msg(&msg, addr).await;
            }
        }
    });
}

fn spawn_cleanup(peers: PeersMap) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            let mut map = peers.lock().await;
            let now_ts = now();
            map.retain(|addr, (_, last_seen, _)| {
                let active = now_ts - *last_seen < 60;
                if !active { println!("[-] {} disconnected (timeout)", addr); }
                active
            });
        }
    });
}
