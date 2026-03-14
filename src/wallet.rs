use k256::{SecretKey, PublicKey};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use bitcoin_hashes::{Hash, hash160, sha256d};
use rand::rngs::OsRng;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Wallet {
    pub balance: u64,
    pub address: String,
}

// Charge ou génère une clé secp256k1 (convention Bitcoin).
// Stockée brute (32 bytes) dans ./{peer_id}_btc_key
pub fn load_or_generate_btc_key(peer_id: &str) -> SecretKey {
    let path = format!("./{}_btc_key", peer_id);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(key) = SecretKey::from_slice(&bytes) {
            return key;
        }
    }
    let key = SecretKey::random(&mut OsRng);
    std::fs::write(&path, key.to_bytes().as_slice()).expect("Cannot save btc key");
    println!("New Bitcoin keypair generated → {}", path);
    key
}

// Dérive une adresse P2PKH Bitcoin à partir d'une clé publique secp256k1.
// Algorithme : Base58Check( 0x00 || hash160(pubkey_compressée) )
// hash160 = RIPEMD160(SHA256(pubkey))
pub fn btc_address(pubkey: &PublicKey) -> String {
    let encoded = pubkey.to_encoded_point(true);
    let compressed_bytes = encoded.as_bytes();

    // hash160 : RIPEMD160(SHA256(pubkey))
    let h160 = hash160::Hash::hash(compressed_bytes);
    let h160_bytes = h160.to_byte_array();

    // Payload : version byte mainnet (0x00) + hash160
    let mut payload = Vec::with_capacity(25);
    payload.push(0x00u8);
    payload.extend_from_slice(&h160_bytes);

    // Checksum : 4 premiers bytes de SHA256(SHA256(payload))
    let checksum = sha256d::Hash::hash(&payload);
    payload.extend_from_slice(&checksum.to_byte_array()[..4]);

    bs58::encode(payload).into_string()
}

// Charge le portefeuille depuis ./{peer_id}_wallet.json ou en crée un vide.
pub fn load_or_create_wallet(peer_id: &str, address: &str) -> Wallet {
    let path = format!("./{}_wallet.json", peer_id);
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(wallet) = serde_json::from_str::<Wallet>(&data) {
            return wallet;
        }
    }
    let wallet = Wallet { balance: 0, address: address.to_string() };
    save_wallet(peer_id, &wallet);
    println!("New wallet created → {} (balance: 0)", address);
    wallet
}

// Persiste le portefeuille sur disque.
pub fn save_wallet(peer_id: &str, wallet: &Wallet) {
    let path = format!("./{}_wallet.json", peer_id);
    let data = serde_json::to_string_pretty(wallet).expect("Cannot serialize wallet");
    std::fs::write(&path, data).expect("Cannot save wallet");
}

// Débite le portefeuille. Retourne false si solde insuffisant.
pub fn debit(wallet: &mut Wallet, amount: u64) -> bool {
    if wallet.balance < amount {
        return false;
    }
    wallet.balance -= amount;
    true
}

// Crédite le portefeuille.
pub fn credit(wallet: &mut Wallet, amount: u64) {
    wallet.balance += amount;
}
