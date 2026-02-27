//! rendezvous - word-code based peer discovery over mainline DHT
//!
//! adapted from x11q: uses pkarr to publish nodeid under a derived keypair,
//! so both sides can find each other using just a short word code.
//! spake2 PAKE ensures only someone with the code can connect.

use ed25519_dalek::SigningKey;
use iroh::{EndpointId, PublicKey};
use pkarr::dns::{rdata::TXT, Name};
use pkarr::{Client as PkarrClient, Keypair, SignedPacket};
use rand::Rng;
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::time::Duration;
use tokio::time::timeout;

use crate::protocol::TableRules;

const DHT_TIMEOUT: Duration = Duration::from_secs(30);
const CODE_TTL: u32 = 300; // 5 minutes

/// well-known seed for public table registry
const PUBLIC_REGISTRY_SEED: &[u8] = b"poker-public-tables-v1";

/// table visibility for discovery
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableVisibility {
    /// anyone can discover and join
    Public,
    /// only visible to friends (requires friend list check)
    FriendsOnly,
    /// not discoverable, code required
    Private,
}

/// table code wrapper
#[derive(Clone, Debug)]
pub struct TableCode(pub String);

impl TableCode {
    pub fn new(code: String) -> Self {
        Self(code)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TableCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// PGP-style wordlist (256 words)
const WORDLIST: [&str; 256] = [
    "aardvark", "absurd", "accrue", "acme", "adrift", "adult", "afflict", "ahead",
    "aimless", "algol", "allow", "almost", "ammo", "ancient", "apple", "artist",
    "assume", "atlas", "awesome", "axle", "baboon", "backfield", "backward", "banjo",
    "beaming", "bedlamp", "beehive", "beeswax", "befriend", "belfast", "berserk", "billiard",
    "bison", "blackjack", "blockade", "blowtorch", "bluebird", "bombast", "bookshelf", "brackish",
    "breadline", "breakup", "brickyard", "briefcase", "burbank", "button", "buzzard", "cement",
    "chairlift", "chatter", "checkup", "chessman", "chico", "chisel", "choking", "classic",
    "classroom", "cleanup", "clockwork", "cobra", "commence", "concert", "cowbell", "crackdown",
    "cranky", "crayon", "crossbow", "crowfoot", "crucial", "crusade", "cubic", "dashboard",
    "deadbolt", "deckhand", "decode", "detour", "digital", "diploma", "disrupt", "distant",
    "diver", "doorstep", "dosage", "dotted", "dragon", "dreadful", "drifter", "dropout",
    "drumbeat", "drunken", "duplex", "dwelling", "eating", "edict", "egghead", "eightball",
    "endorse", "endow", "enlist", "erase", "escape", "exceed", "eyeglass", "eyetooth",
    "facial", "fallout", "flagpole", "flatfoot", "flytrap", "fracture", "framework", "freedom",
    "frighten", "gazelle", "geiger", "glasgow", "glitter", "glucose", "goggles", "goldfish",
    "gremlin", "guidance", "hamlet", "hamster", "handiwork", "headwaters", "highchair", "hockey",
    "hamburger", "hesitate", "hideaway", "holiness", "hurricane", "hydraulic", "idaho", "implicit",
    "indulge", "inferno", "informant", "insincere", "insurgent", "intestine", "inventive", "japanese",
    "jupiter", "kickoff", "kingfish", "klaxon", "liberty", "maritime", "miracle", "misnomer",
    "molasses", "molecule", "montana", "mosquito", "multiple", "nagasaki", "narrative", "nebula",
    "newsletter", "nominal", "northward", "obscure", "october", "offload", "olive", "openwork",
    "operator", "optic", "orbit", "osmosis", "outfielder", "pacific", "pandemic", "pandora",
    "paperweight", "pedigree", "pegasus", "penetrate", "perceptive", "pharmacy", "phonetic", "photograph",
    "pioneering", "piracy", "playhouse", "populate", "potato", "preclude", "prescribe", "printer",
    "procedure", "puberty", "publisher", "pyramid", "quantity", "racketeer", "rampant", "reactor",
    "recipe", "recover", "renegade", "repellent", "replica", "reproduce", "resistor", "responsive",
    "retina", "retrieval", "revenue", "riverbed", "rosebud", "ruffian", "sailboat", "saturday",
    "savanna", "scavenger", "sensation", "sequence", "shadowbox", "showgirl", "signify", "simplify",
    "simulate", "slowdown", "snapshot", "snowcap", "snowslide", "solitude", "southward", "specimen",
    "speculate", "spellbound", "spheroid", "spigot", "spindle", "steadfast", "steamship", "stockman",
    "stopwatch", "stormy", "strawberry", "stupendous", "supportive", "surrender", "suspense", "sweatband",
    "swelter", "tampico", "telephone", "therapist", "tobacco", "tolerance", "tomorrow", "torpedo",
];

/// generate a random table code: "N-word-word"
pub fn generate_code() -> TableCode {
    let mut rng = rand::thread_rng();
    let n: u8 = rng.gen_range(0..100);
    let w1 = WORDLIST[rng.gen_range(0..256)];
    let w2 = WORDLIST[rng.gen_range(0..256)];
    TableCode(format!("{}-{}-{}", n, w1, w2))
}

/// derive deterministic ed25519 keypair from code
fn derive_keypair(code: &str) -> Keypair {
    let mut hasher = Sha256::new();
    hasher.update(b"poker-table-v1:");
    hasher.update(code.as_bytes());
    let seed: [u8; 32] = hasher.finalize().into();
    let signing_key = SigningKey::from_bytes(&seed);
    Keypair::from_secret_key(&signing_key.to_bytes())
}

/// publish table to DHT
pub async fn publish_table(
    code: &TableCode,
    endpoint_id: EndpointId,
    rules: &TableRules,
) -> Result<(), RendezvousError> {
    let keypair = derive_keypair(code.as_str());
    let client = PkarrClient::builder()
        .build()
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    // encode endpoint id + rules hash as TXT record
    let node_id_hex = hex::encode(endpoint_id.as_bytes());
    let rules_hash = hex::encode(&rules.hash()[..8]);
    let txt_value = format!("{}:{}", node_id_hex, rules_hash);

    let name = Name::new("_poker")
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
    let txt = TXT::new()
        .with_string(&txt_value)
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    let packet = SignedPacket::builder()
        .txt(name, txt, CODE_TTL)
        .sign(&keypair)
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    client
        .publish(&packet, None)
        .await
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    Ok(())
}

/// resolve table from DHT
pub async fn resolve_table(code: &TableCode) -> Result<(EndpointId, [u8; 8]), RendezvousError> {
    let keypair = derive_keypair(code.as_str());
    let public_key = keypair.public_key();
    let client = PkarrClient::builder()
        .build()
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    let packet = timeout(DHT_TIMEOUT, client.resolve(&public_key))
        .await
        .map_err(|_| RendezvousError::Timeout)?
        .ok_or(RendezvousError::NotFound)?;

    // find TXT record
    for record in packet.resource_records("_poker") {
        if let pkarr::dns::rdata::RData::TXT(ref txt) = record.rdata {
            let txt_str: String = txt
                .clone()
                .try_into()
                .map_err(|_| RendezvousError::InvalidRecord)?;

            let parts: Vec<&str> = txt_str.split(':').collect();
            if parts.len() != 2 {
                continue;
            }

            let endpoint_id_bytes = hex::decode(parts[0])
                .map_err(|_| RendezvousError::InvalidRecord)?;
            let endpoint_id_arr: [u8; 32] = endpoint_id_bytes
                .try_into()
                .map_err(|_| RendezvousError::InvalidRecord)?;
            let endpoint_id = PublicKey::from_bytes(&endpoint_id_arr)
                .map_err(|_| RendezvousError::InvalidRecord)?;

            let rules_hash_bytes = hex::decode(parts[1])
                .map_err(|_| RendezvousError::InvalidRecord)?;
            let mut rules_hash = [0u8; 8];
            rules_hash.copy_from_slice(&rules_hash_bytes);

            return Ok((endpoint_id, rules_hash));
        }
    }

    Err(RendezvousError::NotFound)
}

/// SPAKE2 server (table host)
pub struct PakeServer {
    spake: Spake2<Ed25519Group>,
    outbound_msg: Vec<u8>,
}

impl PakeServer {
    pub fn new(code: &TableCode) -> Self {
        let (spake, outbound_msg) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_str().as_bytes()),
            &Identity::new(b"poker-host"),
            &Identity::new(b"poker-client"),
        );
        Self { spake, outbound_msg }
    }

    pub fn message(&self) -> &[u8] {
        &self.outbound_msg
    }

    pub fn finish(self, client_msg: &[u8]) -> Result<[u8; 32], RendezvousError> {
        let key = self
            .spake
            .finish(client_msg)
            .map_err(|_| RendezvousError::AuthFailed)?;
        Ok(key.try_into().expect("spake2 produces 32 byte key"))
    }
}

/// SPAKE2 client (table joiner)
pub struct PakeClient {
    spake: Spake2<Ed25519Group>,
    outbound_msg: Vec<u8>,
}

impl PakeClient {
    pub fn new(code: &TableCode) -> Self {
        let (spake, outbound_msg) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.as_str().as_bytes()),
            &Identity::new(b"poker-host"),
            &Identity::new(b"poker-client"),
        );
        Self { spake, outbound_msg }
    }

    pub fn message(&self) -> &[u8] {
        &self.outbound_msg
    }

    pub fn finish(self, server_msg: &[u8]) -> Result<[u8; 32], RendezvousError> {
        let key = self
            .spake
            .finish(server_msg)
            .map_err(|_| RendezvousError::AuthFailed)?;
        Ok(key.try_into().expect("spake2 produces 32 byte key"))
    }
}

/// public table listing entry
#[derive(Clone, Debug)]
pub struct PublicTableEntry {
    /// table code
    pub code: String,
    /// host endpoint id
    pub endpoint_id: EndpointId,
    /// host display name
    pub host_name: String,
    /// stakes string (e.g. "1/2")
    pub stakes: String,
    /// current/max players
    pub players: (u8, u8),
    /// visibility
    pub visibility: TableVisibility,
    /// host's public key (for friend matching)
    pub host_pubkey: [u8; 32],
    /// timestamp when registered
    pub registered_at: u64,
}

/// derive the public registry keypair
fn derive_registry_keypair() -> Keypair {
    let mut hasher = Sha256::new();
    hasher.update(PUBLIC_REGISTRY_SEED);
    let seed: [u8; 32] = hasher.finalize().into();
    let signing_key = SigningKey::from_bytes(&seed);
    Keypair::from_secret_key(&signing_key.to_bytes())
}

/// register a public table in the discovery registry
pub async fn register_public_table(
    code: &TableCode,
    endpoint_id: EndpointId,
    host_name: &str,
    stakes: &str,
    max_players: u8,
    visibility: TableVisibility,
    host_pubkey: &[u8; 32],
) -> Result<(), RendezvousError> {
    // only register public/friends-only tables
    if visibility == TableVisibility::Private {
        return Ok(());
    }

    let keypair = derive_registry_keypair();
    let client = PkarrClient::builder()
        .build()
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    // first try to get existing entries
    let mut entries = list_public_tables_internal(&client).await.unwrap_or_default();

    // remove stale entries (older than 5 min) and our own if exists
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    entries.retain(|e| e.code != code.as_str() && now - e.registered_at < 300);

    // add our entry
    let vis_byte = match visibility {
        TableVisibility::Public => b'P',
        TableVisibility::FriendsOnly => b'F',
        TableVisibility::Private => b'X',
    };

    // encode: code|endpoint|name|stakes|max|vis|pubkey|time
    let entry_str = format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        code.as_str(),
        hex::encode(endpoint_id.as_bytes()),
        host_name,
        stakes,
        max_players,
        vis_byte as char,
        hex::encode(host_pubkey),
        now
    );

    // build TXT records for all entries (max ~10 to stay within packet limits)
    let mut builder = SignedPacket::builder();

    // add existing entries
    for (i, existing) in entries.iter().take(9).enumerate() {
        let vis_byte = match existing.visibility {
            TableVisibility::Public => 'P',
            TableVisibility::FriendsOnly => 'F',
            TableVisibility::Private => 'X',
        };
        let txt_val = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            existing.code,
            hex::encode(existing.endpoint_id.as_bytes()),
            existing.host_name,
            existing.stakes,
            existing.players.1,
            vis_byte,
            hex::encode(existing.host_pubkey),
            existing.registered_at
        );
        let name_str = format!("_t{}", i);
        let name = Name::new(&name_str)
            .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
        let txt = TXT::new()
            .with_string(&txt_val)
            .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
        builder = builder.txt(name, txt, CODE_TTL);
    }

    // add our new entry
    let idx = entries.len().min(9);
    let name_str = format!("_t{}", idx);
    let name = Name::new(&name_str)
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
    let txt = TXT::new()
        .with_string(&entry_str)
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
    builder = builder.txt(name, txt, CODE_TTL);

    let packet = builder
        .sign(&keypair)
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    client
        .publish(&packet, None)
        .await
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    Ok(())
}

/// list all public tables from the registry
pub async fn list_public_tables() -> Result<Vec<PublicTableEntry>, RendezvousError> {
    let client = PkarrClient::builder()
        .build()
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
    list_public_tables_internal(&client).await
}

async fn list_public_tables_internal(client: &PkarrClient) -> Result<Vec<PublicTableEntry>, RendezvousError> {
    let keypair = derive_registry_keypair();
    let public_key = keypair.public_key();

    let packet = timeout(DHT_TIMEOUT, client.resolve(&public_key))
        .await
        .map_err(|_| RendezvousError::Timeout)?
        .ok_or(RendezvousError::NotFound)?;

    let mut entries = Vec::new();

    // scan for _t0, _t1, ... records
    for i in 0..10 {
        let name = format!("_t{}", i);
        for record in packet.resource_records(&name) {
            if let pkarr::dns::rdata::RData::TXT(ref txt) = record.rdata {
                let txt_str: String = txt
                    .clone()
                    .try_into()
                    .map_err(|_| RendezvousError::InvalidRecord)?;

                if let Some(entry) = parse_table_entry(&txt_str) {
                    entries.push(entry);
                }
            }
        }
    }

    Ok(entries)
}

fn parse_table_entry(s: &str) -> Option<PublicTableEntry> {
    let parts: Vec<&str> = s.split('|').collect();
    if parts.len() != 8 {
        return None;
    }

    let code = parts[0].to_string();
    let endpoint_bytes = hex::decode(parts[1]).ok()?;
    let endpoint_arr: [u8; 32] = endpoint_bytes.try_into().ok()?;
    let endpoint_id = PublicKey::from_bytes(&endpoint_arr).ok()?;
    let host_name = parts[2].to_string();
    let stakes = parts[3].to_string();
    let max_players: u8 = parts[4].parse().ok()?;
    let visibility = match parts[5].chars().next()? {
        'P' => TableVisibility::Public,
        'F' => TableVisibility::FriendsOnly,
        _ => return None,
    };
    let pubkey_bytes = hex::decode(parts[6]).ok()?;
    let host_pubkey: [u8; 32] = pubkey_bytes.try_into().ok()?;
    let registered_at: u64 = parts[7].parse().ok()?;

    Some(PublicTableEntry {
        code,
        endpoint_id,
        host_name,
        stakes,
        players: (0, max_players), // current players unknown from registry
        visibility,
        host_pubkey,
        registered_at,
    })
}

/// unregister a table from the public registry
pub async fn unregister_public_table(code: &TableCode) -> Result<(), RendezvousError> {
    let keypair = derive_registry_keypair();
    let client = PkarrClient::builder()
        .build()
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    // get existing entries and remove ours
    let mut entries = list_public_tables_internal(&client).await.unwrap_or_default();
    entries.retain(|e| e.code != code.as_str());

    if entries.is_empty() {
        // nothing to publish
        return Ok(());
    }

    // rebuild packet without our entry
    let mut builder = SignedPacket::builder();
    for (i, existing) in entries.iter().enumerate() {
        let vis_byte = match existing.visibility {
            TableVisibility::Public => 'P',
            TableVisibility::FriendsOnly => 'F',
            TableVisibility::Private => 'X',
        };
        let txt_val = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            existing.code,
            hex::encode(existing.endpoint_id.as_bytes()),
            existing.host_name,
            existing.stakes,
            existing.players.1,
            vis_byte,
            hex::encode(existing.host_pubkey),
            existing.registered_at
        );
        let name_str = format!("_t{}", i);
        let name = Name::new(&name_str)
            .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
        let txt = TXT::new()
            .with_string(&txt_val)
            .map_err(|e| RendezvousError::DhtError(e.to_string()))?;
        builder = builder.txt(name, txt, CODE_TTL);
    }

    let packet = builder
        .sign(&keypair)
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    client
        .publish(&packet, None)
        .await
        .map_err(|e| RendezvousError::DhtError(e.to_string()))?;

    Ok(())
}

/// rendezvous errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum RendezvousError {
    #[error("DHT operation failed: {0}")]
    DhtError(String),
    #[error("table not found")]
    NotFound,
    #[error("DHT lookup timed out")]
    Timeout,
    #[error("invalid DHT record")]
    InvalidRecord,
    #[error("authentication failed - wrong code?")]
    AuthFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_generation() {
        let code = generate_code();
        let parts: Vec<&str> = code.as_str().split('-').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0].parse::<u8>().unwrap() < 100);
    }

    #[test]
    fn test_keypair_deterministic() {
        let k1 = derive_keypair("7-tiger-lamp");
        let k2 = derive_keypair("7-tiger-lamp");
        assert_eq!(k1.public_key().to_z32(), k2.public_key().to_z32());
    }

    #[test]
    fn test_pake_success() {
        let code = TableCode::new("7-tiger-lamp".to_string());

        let server = PakeServer::new(&code);
        let client = PakeClient::new(&code);

        // capture messages before consuming
        let server_msg = server.message().to_vec();
        let client_msg = client.message().to_vec();

        let sk = server.finish(&client_msg).unwrap();
        let ck = client.finish(&server_msg).unwrap();
        assert_eq!(sk, ck);
    }

    #[test]
    fn test_pake_wrong_code() {
        let server = PakeServer::new(&TableCode::new("7-tiger-lamp".to_string()));
        let client = PakeClient::new(&TableCode::new("8-wrong-code".to_string()));

        let server_msg = server.message().to_vec();
        let client_msg = client.message().to_vec();

        // spake2 doesn't error on wrong password, but produces different keys
        let sk = server.finish(&client_msg).unwrap();
        let ck = client.finish(&server_msg).unwrap();

        // keys should NOT match when passwords differ
        assert_ne!(sk, ck, "keys should not match with wrong code");
    }
}
