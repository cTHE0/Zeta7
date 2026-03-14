use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use serde::{Serialize, Deserialize};
use crate::wallet::Wallet;

/// Un message affiché dans le fil de discussion de l'interface web.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub from_id: String,
    pub fingerprint: String,
    pub text: String,
    pub timestamp: u64,
    /// "message" | "payment_received" | "payment_sent" | "payment_ack"
    pub msg_type: String,
}

/// Événement envoyé via SSE aux clients web connectés.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SseEvent {
    pub event_type: String, // "message" | "payment" | "wallet_update"
    pub data: String,       // payload JSON sérialisé
}

/// Commandes envoyées depuis l'UI web vers la boucle client UDP.
pub enum UiCommand {
    SendMessage(String),
    Pay { dst_id: String, amount: u64 },
}

/// État global partagé entre le serveur HTTP et la boucle P2P.
pub struct AppState {
    pub node_id: String,
    pub fingerprint: String,
    pub btc_address: String,
    pub messages: RwLock<Vec<ChatMessage>>,
    pub wallet: Arc<Mutex<Wallet>>,
    pub events_tx: broadcast::Sender<SseEvent>,
    pub cmd_tx: mpsc::Sender<UiCommand>,
}

impl AppState {
    /// Ajoute un message au fil et notifie les clients SSE.
    pub async fn push_message(&self, msg: ChatMessage) {
        let ev_type = if msg.msg_type == "message" { "message" } else { "payment" };
        let data = serde_json::to_string(&msg).unwrap_or_default();
        {
            let mut msgs = self.messages.write().await;
            msgs.push(msg);
            // On conserve au maximum 500 messages en mémoire
            if msgs.len() > 500 {
                let overflow = msgs.len() - 500;
                msgs.drain(0..overflow);
            }
        }
        let _ = self.events_tx.send(SseEvent {
            event_type: ev_type.to_string(),
            data,
        });
    }

    /// Notifie les clients SSE de la mise à jour du solde.
    pub async fn broadcast_wallet_update(&self) {
        let w = self.wallet.lock().await;
        if let Ok(data) = serde_json::to_string(&*w) {
            let _ = self.events_tx.send(SseEvent {
                event_type: "wallet_update".to_string(),
                data,
            });
        }
    }
}
