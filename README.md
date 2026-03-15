# Zeta — Réseau social P2P décentralisé

Zeta est un réseau de messagerie pair-à-pair avec portefeuille Bitcoin intégré. Chaque nœud se connecte via un relai UDP, signe ses messages avec Ed25519, et peut envoyer des sats à d'autres pairs. Une interface web s'ouvre dans le navigateur au démarrage.

---

## Prérequis

**Rust** (édition 2021) — installation en une commande :

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Aucune autre dépendance système requise.

---

## Installation

```bash
git clone https://github.com/<ton-compte>/zeta7.git
cd zeta7
cargo build --release
```

---

## Lancement

### Mode client (usage normal)

```bash
cargo run --release -- --mode client --peer-id <ton_nom> --relay-addr <ip_relai>:12345
```

**Exemple (un seul relai) :**

```bash
cargo run --release -- --mode client --peer-id alice --relay-addr 51.159.0.1:12345
```

**Exemple (plusieurs relais, comma-séparés) :**

```bash
cargo run --release -- --mode client --peer-id alice --relay-addr 51.159.0.1:12345,51.159.0.2:12345
```

Connecter un client à plusieurs relais augmente la portée et la redondance : les messages envoyés sont diffusés sur tous les relais simultanément, et les messages reçus en double (via plusieurs relais) sont automatiquement dédupliqués.

L'interface web démarre automatiquement sur **http://127.0.0.1:8080** — ouvre-la dans ton navigateur.

> Lors du premier lancement, trois fichiers sont créés dans le répertoire courant :
> `alice_identity`, `alice_btc_key`, `alice_wallet.json`.
> Ils constituent ton identité et ton portefeuille. Garde-les pour retrouver le même nœud.

---

### Mode relai (pour héberger un point de connexion)

```bash
cargo run --release -- --mode relay --peer-id relay1
```

Écoute sur le port UDP **12345**. Plusieurs relais peuvent se fédérer :

```bash
cargo run --release -- --mode relay --peer-id relay1 --peer-relays 5.6.7.8:12345
```

---

## Interface web

L'interface s'ouvre sur `http://127.0.0.1:8080`.

| Section | Description |
|---|---|
| **Identité** | Peer ID et empreinte Ed25519 (16 caractères hex) |
| **Portefeuille** | Solde en satoshis et adresse Bitcoin P2PKH |
| **Envoyer des sats** | Paiement vers un pair (Peer ID ou empreinte) |
| **Fil de messages** | Messages signés reçus du réseau en temps réel |
| **Barre d'envoi** | Diffuser un message signé à tous les pairs |

Les événements arrivent sans rechargement via **Server-Sent Events**.

---

## Commandes CLI (optionnel)

Les mêmes actions sont disponibles dans le terminal :

```
Bonjour tout le monde   → diffuse un message signé
/balance                → affiche le solde et l'adresse Bitcoin
/pay alice 500          → envoie 500 sats au pair "alice"
```

---

## Architecture

```
[Alice] ──── Register / Classic / Payment ────→ [Relai A] ─── fédération ───→ [Relai B]
   │                                                  │                              │
   └──── Register / Classic / Payment ───────────────┘                          [Bob]
                                              Broadcast (TTL 16)
                                              Vérification signature
```

Un client peut se connecter à plusieurs relais simultanément (option `--relay-addr` comma-séparée).
Les relais peuvent eux-mêmes se fédérer entre eux (`--peer-relays`).

- Messages signés **Ed25519** — le relai vérifie avant de retransmettre.
- Paiements relayés en UDP ; le destinataire répond par un `PaymentAck`.
- Portefeuille **local** (JSON) — pas de blockchain, chaque nœud gère son solde.
- Détection du type NAT intégrée (STUN, RFC 3489) pour le futur hole-punching direct.

---

## Fichiers générés

| Fichier | Contenu |
|---|---|
| `<id>_identity` | Seed Ed25519 (32 octets) |
| `<id>_btc_key` | Clé privée secp256k1 (32 octets) |
| `<id>_wallet.json` | Solde et adresse Bitcoin |
