# Zeta — Réseau social P2P décentralisé

<p align="center">
  <img src="assets/logo.png" alt="Zeta Logo" width="120" height="120">
</p>

<p align="center">
  <a href="https://github.com/CTHE0/zeta7"><img src="https://img.shields.io/badge/Code-Repository-blue?logo=github" alt="Code"></a>
  <a href="https://github.com/CTHE0/zeta7/actions"><img src="https://img.shields.io/badge/Analysis-CI/CD-green?logo=githubactions" alt="Analysis"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow?logo=opensourceinitiative" alt="License"></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust" alt="Built with Rust"></a>
</p>

<p align="center">
  <strong>Messagerie pair-à-pair sécurisée avec portefeuille Bitcoin intégré</strong>
</p>

---

## Présentation

Zeta est un protocole de messagerie décentralisé fonctionnant en pair-à-pair, intégrant nativement un portefeuille Bitcoin léger. Chaque nœud s'authentifie via une signature Ed25519, communique via des relais UDP et peut effectuer des micro-transactions en satoshis vers d'autres pairs. Une interface web embarquée est automatiquement exposée au démarrage pour une interaction utilisateur simplifiée.

> **Objectif** : Fournir une infrastructure de communication résiliente, respectueuse de la vie privée, et compatible avec les protocoles financiers décentralisés.

---

## Prérequis

| Dépendance | Version minimale | Installation |
|------------|-----------------|--------------|
| **Rust** | Edition 2021 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| **Cargo** | Inclus avec Rust | Vérifier avec `cargo --version` |

Aucune dépendance système supplémentaire n'est requise.

---

## Installation

```bash
# Clonage du dépôt
git clone https://github.com/CTHE0/zeta7.git
cd zeta7

# Compilation en mode production
cargo build --release
```

---

## Démarrage

### Mode client (utilisation standard)

```bash
cargo run --release -- --mode client --peer-id <identifiant> --relay-addr <ip_relai>:12345
```

**Exemple — connexion à un relai unique :**

```bash
cargo run --release -- --mode client --peer-id alice --relay-addr 51.159.0.1:12345
```

**Exemple — connexion à plusieurs relais (redondance) :**

```bash
cargo run --release -- --mode client --peer-id alice --relay-addr 51.159.0.1:12345,51.159.0.2:12345
```

> La connexion simultanée à plusieurs relais améliore la portée réseau et la tolérance aux pannes. Les messages dupliqués sont automatiquement dédupliqués au niveau du client.

L'interface web est accessible à l'adresse : **http://127.0.0.1:8080**

> **Premier lancement** : Trois fichiers sont générés dans le répertoire courant :
> - `<id>_identity` : graine Ed25519 (identité cryptographique)
> - `<id>_btc_key` : clé privée secp256k1 (portefeuille Bitcoin)
> - `<id>_wallet.json` : état local du portefeuille
>
> ⚠️ Ces fichiers constituent l'identité et les actifs du nœud. Il est recommandé de les sauvegarder dans un emplacement sécurisé.

### Mode relai (hébergement d'un point de connexion)

```bash
# Lancement d'un relai simple
cargo run --release -- --mode relay --peer-id relay1

# Fédération avec d'autres relais
cargo run --release -- --mode relay --peer-id relay1 --peer-relays 5.6.7.8:12345
```

Le relai écoute par défaut sur le port UDP **12345**.

---

## Interface utilisateur

L'interface web, accessible via `http://127.0.0.1:8080`, propose les fonctionnalités suivantes :

| Section | Fonctionnalité |
|---------|---------------|
| **Identité** | Affichage du Peer ID et de l'empreinte Ed25519 (hexadécimal, 16 caractères) |
| **Portefeuille** | Solde en satoshis et adresse Bitcoin P2PKH associée |
| **Transfert de fonds** | Envoi de satoshis vers un pair (via Peer ID ou empreinte) |
| **Fil de messages** | Réception en temps réel des messages signés du réseau |
| **Publication** | Diffusion d'un message signé à l'ensemble des pairs connectés |

Les mises à jour sont transmises en temps réel via **Server-Sent Events (SSE)**, sans rechargement de page.

---

## Interface en ligne de commande (optionnelle)

Les opérations principales sont également accessibles via la CLI :

```bash
# Diffusion d'un message signé
Bonjour tout le monde

# Consultation du solde
/balance

# Envoi de 500 satoshis au pair "alice"
/pay alice 500
```

---

## Architecture technique

```
[Alice] ──── Register / Classic / Payment ────→ [Relai A] ─── fédération ───→ [Relai B]
   │                                                  │                              │
   └──── Register / Classic / Payment ───────────────┘                          [Bob]
                                              Broadcast (TTL 16)
                                              Vérification signature Ed25519
```

### Principes de conception

- **Authentification** : Tous les messages sont signés via Ed25519 ; les relais valident les signatures avant retransmission.
- **Transport** : Communication UDP avec mécanisme d'accusé de réception (`PaymentAck`) pour les transactions.
- **Gestion des portefeuilles** : Stockage local au format JSON ; aucun consensus blockchain requis pour les micro-paiements internes.
- **NAT Traversal** : Détection intégrée du type de NAT (STUN, RFC 3489) en prévision du hole-punching direct.
- **Redondance** : Support natif de la connexion multi-relais côté client et de la fédération côté relai.

---

## Fichiers générés

| Fichier | Description | Sensibilité |
|---------|-------------|-------------|
| `<id>_identity` | Graine Ed25519 (32 octets) — identité cryptographique | 🔐 Critique |
| `<id>_btc_key` | Clé privée secp256k1 (32 octets) — accès aux fonds | 🔐 Critique |
| `<id>_wallet.json` | État local du portefeuille (solde, adresse) | ⚠️ Important |

> **Recommandation de sécurité** : Ne jamais partager ces fichiers. Leur compromission entraîne la perte irréversible de l'identité et des actifs associés.

---

## Licence

Ce projet est distribué sous licence **MIT**. Consultez le fichier [LICENSE](LICENSE) pour plus de détails.

---

## Contribution

Les contributions sont les bienvenues. Pour proposer une modification :

1. Forkez le dépôt
2. Créez une branche dédiée (`git checkout -b feature/nouvelle-fonctionnalite`)
3. Soumettez une pull request avec une description détaillée des changements

Pour toute question ou signalement de problème, veuillez ouvrir une [issue](https://github.com/CTHE0/zeta7/issues).

---

<p align="center">
  <sub>Projet Zeta — Construit avec Rust · Réseau décentralisé · Vie privée par conception</sub>
</p>
```