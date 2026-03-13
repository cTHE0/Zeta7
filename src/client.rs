use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::{Opts, Message, UdpSocketExt, get_public_ip};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

pub async fn main_client(opts: Opts) {
    let relay_addr: SocketAddr = opts.relay_addr.expect("--relay-addr required")
        .parse().expect("Wrong address format");
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await.expect("Failed to bind"));
    let public_addr: SocketAddr = get_public_ip(&socket).await.expect("Public IP not obtained.");
    let peer_id = opts.peer_id;

    // Enregistrement auprès du relai
    let msg = Message::Register {
        src_addr: public_addr,
        src_id: peer_id.clone(),
        dst_addr: relay_addr,
        dst_id: "relay".to_string(),
        time: now(),
    };
    socket.send_msg(&msg, relay_addr).await.unwrap();
    println!("Connected as '{}' on {}", peer_id, public_addr);

    // Heartbeat : re-enregistrement toutes les 30s pour rester dans la liste du relai
    let socket_hb = Arc::clone(&socket);
    let peer_id_hb = peer_id.clone();
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(30)).await;
            let msg = Message::Register {
                src_addr: public_addr,
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
    tokio::spawn(async move {
        let mut buf = vec![0; 4096];
        loop {
            if let Ok((size, _)) = socket_rx.recv_from(&mut buf).await {
                if let Ok(msg) = bincode::deserialize::<Message>(&buf[..size]) {
                    if let Message::Classic { src_id, txt, .. } = msg {
                        println!("[{}] {}", src_id, txt);
                    }
                }
            }
        }
    });

    // Boucle d'envoi depuis stdin
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let msg = Message::Classic {
            src_addr: public_addr,
            src_id: peer_id.clone(),
            dst_addr: relay_addr,
            dst_id: "all".to_string(),
            txt: line,
            time: now(),
            msg_id: rand::random::<u64>(),
            ttl: 16,
        };
        let _ = socket.send_msg(&msg, relay_addr).await;
    }
}
