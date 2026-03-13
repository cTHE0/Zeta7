use simple_logger::SimpleLogger;
use std::net::SocketAddr;
use log::LevelFilter;
use rand::Rng;
use crate::nat_detector::util::nat_detect_with_servers;
use crate::nat_detector::util::NatType;

mod util;

pub async fn nat_detector () -> std::io::Result<(NatType, SocketAddr)> {
	// Paramètres modifiables
    let stun_servers: Option<Vec<String>> = None;
    let stun_servers_count: usize = 20;
    let verbose: bool = false;

    let mut logger = SimpleLogger::new();
    if verbose {
        logger = logger.with_level(LevelFilter::Debug);
    } else {
        logger = logger.with_level(LevelFilter::Info);
    }
    logger.init().unwrap();
    let vec = stun_servers.unwrap_or_else(|| {
        let vec: Vec<String> = include_str!("valid_ipv4s.txt").lines().map(|e|e.trim().to_string()).collect();
        // select server randomly
        let mut rng = rand::thread_rng();
        let mut new_vec = Vec::new();
        for _ in 0..stun_servers_count {
            let stun_server = vec[rng.gen_range(0..vec.len())].to_string();
            new_vec.push(stun_server);
        }
        new_vec
    });
    let stun_servers = vec.iter().map(|e| e.as_str()).collect::<Vec<&str>>();

	nat_detect_with_servers(stun_servers.as_slice()).await.map_err(|e| {
	    eprintln!("[ERROR] Cannot detect the NAT type: {}", e);
	    e
	})
}