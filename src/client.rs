use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::{Opts, Message, UdpSocketExt};
use crate::identity::{load_or_generate_keypair, sign, verify, fingerprint};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub async fn main_client(opts: Opts) {
    let relay_addr: SocketAddr = opts.relay_addr.expect("--relay-addr required")
        .parse().expect("Wrong address format");
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await.expect("Failed to bind"));
    let local_addr = socket.local_addr().unwrap();
    let peer_id = opts.peer_id;

    // Chargement ou génération du couple de clés
    let signing_key = load_or_generate_keypair(&peer_id);
    let public_key = signing_key.verifying_key().to_bytes().to_vec();

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

    // Réception et vérification des messages en arrière-plan
    let socket_rx = Arc::clone(&socket);
    tokio::spawn(async move {
        let mut buf = vec![0; 4096];
        loop {
            if let Ok((size, _)) = socket_rx.recv_from(&mut buf).await {
                if let Ok(msg) = bincode::deserialize::<Message>(&buf[..size]) {
                    if let Message::Classic { src_id, txt, time, msg_id, public_key, signature, .. } = msg {
                        if verify(&public_key, &src_id, &txt, time, msg_id, &signature) {
                            println!("[{} - {}] {}", src_id, fingerprint(&public_key), txt);
                        } else {
                            eprintln!("[WARN] Message de '{}' — signature invalide, ignoré", src_id);
                        }
                    }
                }
            }
        }
    });

    // Boucle d'envoi depuis stdin
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
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
