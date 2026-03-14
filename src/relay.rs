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

type PeersMap = Arc<Mutex<HashMap<SocketAddr, (String, u64)>>>; // addr → (peer_id, last_seen)

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

// Mémorise les msg_id déjà traités pour éviter de diffuser deux fois le même message.
// Borné à `max` entrées : les plus ancien sont évincés en premier.
struct SeenIds {
    set: HashSet<u64>,
    queue: VecDeque<u64>,
    max: usize,
}

impl SeenIds {
    fn new(max: usize) -> Self {
        SeenIds { set: HashSet::new(), queue: VecDeque::new(), max }
    }
    // Retourne true si le msg_id est nouveau (et l'enregistre), false s'il était déjà vu.
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

pub async fn main_relay(opts: Opts) {
    let my_id = opts.peer_id.clone();
    let relay_id = format!("relay:{}", my_id); // préfixe pour se distinguer des clients

    // Adresses des autres relays avec lesquels se fédérer
    let peer_relay_addrs: Vec<SocketAddr> = opts.peer_relays
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse().expect("Invalid peer relay address"))
        .collect();

    let port_relay = 12345;
    let socket = Arc::new(UdpSocket::bind(format!("0.0.0.0:{}", port_relay)).await.expect("Failed to bind"));
    println!("Relay '{}' listening on port {}...", my_id, port_relay);

    let signing_key = load_or_generate_keypair(&my_id);
    let public_key = signing_key.verifying_key().to_bytes().to_vec();

    let peers_list: PeersMap = Arc::new(Mutex::new(HashMap::new()));
    let seen_ids = Arc::new(Mutex::new(SeenIds::new(10_000)));

    // Enregistrement auprès des autres relays
    for peer_relay_addr in &peer_relay_addrs {
        let msg = Message::Register {
            src_addr: format!("0.0.0.0:{}", port_relay).parse().unwrap(),
            src_id: relay_id.clone(),
            dst_addr: *peer_relay_addr,
            dst_id: "relay".to_string(),
            time: now(),
        };
        socket.send_msg(&msg, *peer_relay_addr).await.unwrap();
        println!("Registered with peer relay {}", peer_relay_addr);
    }

    // Heartbeat : re-enregistrement toutes les 30s auprès des autres relays
    let socket_hb = Arc::clone(&socket);
    let relay_id_hb = relay_id.clone();
    let peer_relay_addrs_hb = peer_relay_addrs.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            for peer_relay_addr in &peer_relay_addrs_hb {
                let msg = Message::Register {
                    src_addr: format!("0.0.0.0:{}", port_relay).parse().unwrap(),
                    src_id: relay_id_hb.clone(),
                    dst_addr: *peer_relay_addr,
                    dst_id: "relay".to_string(),
                    time: now(),
                };
                let _ = socket_hb.send_msg(&msg, *peer_relay_addr).await;
            }
        }
    });

    // Nettoyage automatique des peers inactifs
    let peers_cleanup = Arc::clone(&peers_list);
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            delete_disconnected_peers(&peers_cleanup).await;
        }
    });

    // Réception des messages
    let socket_rx = Arc::clone(&socket);
    let peers_rx = Arc::clone(&peers_list);
    let seen_rx = Arc::clone(&seen_ids);
    tokio::spawn(async move {
        let mut buf = vec![0; 4096];
        loop {
            match socket_rx.recv_from(&mut buf).await {
                Ok((size, sender_addr)) => {
                    let msg: Message = match bincode::deserialize(&buf[..size]) {
                        Ok(m) => m,
                        Err(e) => { eprintln!("[ERROR] Deserialization failed: {}", e); continue; }
                    };

                    if let Message::Register { src_id, time, .. } = &msg {
                        peers_rx.lock().await
                            .entry(sender_addr)
                            .and_modify(|(_, t)| *t = *time)
                            .or_insert((src_id.clone(), *time));
                        println!("[+] '{}' registered ({})", src_id, sender_addr);
                    }

                    if let Message::Classic { src_id, txt, time, msg_id, ttl, public_key, signature, .. } = &msg {
                        // Déduplication : ignorer les messages déjà vus
                        if !seen_rx.lock().await.register(*msg_id) { continue; }

                        // Vérification de la signature
                        if verify(public_key, src_id, txt, *time, *msg_id, signature) {
                            println!("[{} - {}] {}", src_id, fingerprint(public_key), txt);
                        } else {
                            eprintln!("[WARN] Message de '{}' — signature invalide, ignoré", src_id);
                            continue;
                        }

                        // Continuer à propager si le TTL n'est pas épuisé
                        if *ttl > 0 {
                            let mut fwd = msg.clone();
                            if let Message::Classic { ttl: ref mut t, .. } = fwd { *t -= 1; }
                            relay_message(&peers_rx, sender_addr, fwd, &socket_rx).await;
                        }
                    }

                    if let Message::AskForAddr { src_addr, peer_id, .. } = &msg {
                        let map = peers_rx.lock().await;
                        if let Some((found_addr, _)) = map.iter().find(|(_, (id, _))| id == peer_id) {
                            let reply = Message::PeerInfo { peer_addr: *found_addr, peer_id: peer_id.clone() };
                            drop(map);
                            let _ = socket_rx.send_msg(&reply, *src_addr).await;
                        } else {
                            eprintln!("Peer {} not found", peer_id);
                        }
                    }
                }
                Err(e) => eprintln!("[ERROR]: {}", e),
            }
        }
    });

    // Boucle stdin : le relay peut aussi écrire dans le chat
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let msg_id: u64 = rand::random();
        let time_val = now();
        let sig = sign(&signing_key, &my_id, &line, time_val, msg_id);
        let msg = Message::Classic {
            src_addr: format!("0.0.0.0:{}", port_relay).parse().unwrap(),
            src_id: my_id.clone(),
            dst_addr: "0.0.0.0:0".parse().unwrap(),
            dst_id: "all".to_string(),
            txt: line,
            time: time_val,
            msg_id,
            ttl: 16,
            public_key: public_key.clone(),
            signature: sig,
        };
        // Enregistrer ce msg_id pour ignorer les éventuels échos en retour
        seen_ids.lock().await.register(msg_id);
        // dummy sender_addr absent de peers_list → broadcast à tous
        let dummy: SocketAddr = "0.0.0.0:0".parse().unwrap();
        relay_message(&peers_list, dummy, msg, &socket).await;
    }
}

// Diffuse un message à tous les peers sauf l'expéditeur.
// La prévention de boucles est assurée par la déduplication via SeenIds.
async fn relay_message(peers: &PeersMap, sender_addr: SocketAddr, msg: Message, socket: &UdpSocket) {
    let peers_map = peers.lock().await;
    for (other_addr, _) in peers_map.iter() {
        if other_addr == &sender_addr { continue; }
        if let Err(e) = socket.send_msg(&msg, *other_addr).await {
            eprintln!("Failed to send to {}: {}", other_addr, e);
        }
    }
}

async fn delete_disconnected_peers(peers: &PeersMap) {
    let mut peers_map = peers.lock().await;
    let now = now();
    peers_map.retain(|addr, (_, last_seen)| {
        let active = now - *last_seen < 60;
        if !active { println!("[-] Peer {} disconnected (timeout)", addr); }
        active
    });
}
