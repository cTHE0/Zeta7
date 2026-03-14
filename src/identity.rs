use ed25519_dalek::{SigningKey, VerifyingKey, Signer, Verifier, Signature};
use rand::rngs::OsRng;

// Retourne les 8 premiers bytes de la clé publique en hexadécimal (fingerprint court).
pub fn fingerprint(public_key: &[u8]) -> String {
    public_key.iter().take(8).map(|b| format!("{:02x}", b)).collect()
}

// Charge la clé depuis ./{peer_id}_identity ou en génère une nouvelle.
pub fn load_or_generate_keypair(peer_id: &str) -> SigningKey {
    let path = format!("./{}_identity", peer_id);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(seed) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return SigningKey::from_bytes(&seed);
        }
    }
    let key = SigningKey::generate(&mut OsRng);
    std::fs::write(&path, key.to_bytes()).expect("Cannot save identity");
    println!("New identity generated → {}", path);
    key
}

// Signe le contenu immuable du message (hors ttl qui change à chaque saut).
pub fn sign(key: &SigningKey, src_id: &str, txt: &str, time: u64, msg_id: u64) -> Vec<u8> {
    let data = build_signed_data(src_id, txt, time, msg_id);
    key.sign(&data).to_bytes().to_vec()
}

// Retourne true si la signature est valide.
pub fn verify(public_key: &[u8], src_id: &str, txt: &str, time: u64, msg_id: u64, signature: &[u8]) -> bool {
    let Ok(pk_bytes) = <[u8; 32]>::try_from(public_key) else { return false; };
    let Ok(sig_bytes) = <[u8; 64]>::try_from(signature) else { return false; };
    let Ok(vk) = VerifyingKey::from_bytes(&pk_bytes) else { return false; };
    let sig = Signature::from_bytes(&sig_bytes);
    let data = build_signed_data(src_id, txt, time, msg_id);
    vk.verify(&data, &sig).is_ok()
}

fn build_signed_data(src_id: &str, txt: &str, time: u64, msg_id: u64) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(src_id.as_bytes());
    data.extend_from_slice(txt.as_bytes());
    data.extend_from_slice(&time.to_le_bytes());
    data.extend_from_slice(&msg_id.to_le_bytes());
    data
}
