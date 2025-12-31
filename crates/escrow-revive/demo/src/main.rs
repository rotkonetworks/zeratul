//! Escrow Demo CLI
//!
//! Generate shares, create calldata, and demonstrate the escrow flow.

use clap::{Parser, Subcommand};
use rand::Rng;
use sha3::{Digest, Keccak256};

/// Binary field element in GF(2^32)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BF32(u32);

impl BF32 {
    fn zero() -> Self { Self(0) }

    fn add(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
    }

    fn mul(self, other: Self) -> Self {
        const IRREDUCIBLE: u32 = 0x8D;
        let mut a = self.0 as u64;
        let mut b = other.0 as u64;
        let mut result: u64 = 0;

        while b != 0 {
            if b & 1 != 0 {
                result ^= a;
            }
            a <<= 1;
            if a & (1 << 32) != 0 {
                a ^= (1 << 32) | (IRREDUCIBLE as u64);
            }
            b >>= 1;
        }
        Self(result as u32)
    }

    fn from_le_bytes(bytes: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(bytes))
    }

    fn to_le_bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }
}

/// Share data
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Share {
    index: usize,
    values: Vec<u32>,
    merkle_proof: Vec<String>,
}

/// Full share set with commitment
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ShareSet {
    commitment: String,
    escrow_pubkey: String,
    shares: Vec<Share>,
}

/// Keccak256 hash
fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Hash share values
fn hash_share_values(values: &[BF32]) -> [u8; 32] {
    let mut data = Vec::with_capacity(values.len() * 4);
    for v in values {
        data.extend_from_slice(&v.to_le_bytes());
    }
    keccak256(&data)
}

/// Evaluate polynomial at point x
fn evaluate_polynomial(coeffs: &[BF32], x: BF32) -> BF32 {
    let mut result = BF32::zero();
    for coeff in coeffs.iter().rev() {
        result = result.mul(x).add(*coeff);
    }
    result
}

/// Build Merkle tree and return (root, proofs)
fn build_merkle_tree(leaves: &[[u8; 32]]) -> ([u8; 32], Vec<Vec<[u8; 32]>>) {
    let n = leaves.len();
    if n == 0 {
        return ([0u8; 32], vec![]);
    }
    if n == 1 {
        return (leaves[0], vec![vec![]]);
    }

    let padded_len = n.next_power_of_two();
    let mut current_level: Vec<[u8; 32]> = leaves.to_vec();
    current_level.resize(padded_len, [0u8; 32]);

    let mut levels = vec![current_level.clone()];

    while current_level.len() > 1 {
        let mut next_level = Vec::with_capacity(current_level.len() / 2);
        for chunk in current_level.chunks(2) {
            let mut combined = [0u8; 64];
            combined[..32].copy_from_slice(&chunk[0]);
            combined[32..].copy_from_slice(&chunk[1]);
            next_level.push(keccak256(&combined));
        }
        levels.push(next_level.clone());
        current_level = next_level;
    }

    let root = current_level[0];

    let mut proofs = Vec::with_capacity(n);
    for i in 0..n {
        let mut proof = Vec::new();
        let mut idx = i;

        for level in &levels[..levels.len() - 1] {
            let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
            if sibling_idx < level.len() {
                proof.push(level[sibling_idx]);
            } else {
                proof.push([0u8; 32]);
            }
            idx /= 2;
        }
        proofs.push(proof);
    }

    (root, proofs)
}

/// Generate VSS shares for a 32-byte secret
fn generate_shares(secret: &[u8; 32], threshold: usize, num_shares: usize) -> ShareSet {
    let mut rng = rand::thread_rng();

    // Convert secret to 8 field elements
    let secret_elems: Vec<BF32> = secret
        .chunks(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().unwrap();
            BF32::from_le_bytes(arr)
        })
        .collect();

    // For each field element, create polynomial and evaluate
    let mut all_share_values: Vec<Vec<BF32>> = vec![Vec::new(); num_shares];

    for secret_elem in &secret_elems {
        let mut coeffs = vec![*secret_elem];
        for _ in 1..threshold {
            coeffs.push(BF32(rng.gen()));
        }

        for i in 0..num_shares {
            let x = BF32((i + 1) as u32);
            let y = evaluate_polynomial(&coeffs, x);
            all_share_values[i].push(y);
        }
    }

    // Build Merkle tree
    let leaf_hashes: Vec<[u8; 32]> = all_share_values
        .iter()
        .map(|values| hash_share_values(values))
        .collect();

    let (root, proofs) = build_merkle_tree(&leaf_hashes);

    // Create shares
    let shares: Vec<Share> = all_share_values
        .into_iter()
        .enumerate()
        .map(|(i, values)| Share {
            index: i,
            values: values.iter().map(|v| v.0).collect(),
            merkle_proof: proofs[i].iter().map(|h| hex::encode(h)).collect(),
        })
        .collect();

    // Derive escrow pubkey from secret (in real impl, this would be curve ops)
    let escrow_pubkey = keccak256(secret);

    ShareSet {
        commitment: hex::encode(root),
        escrow_pubkey: hex::encode(escrow_pubkey),
        shares,
    }
}

/// Verify a share against commitment
fn verify_share(commitment: &[u8; 32], share: &Share) -> bool {
    let values: Vec<BF32> = share.values.iter().map(|&v| BF32(v)).collect();
    let leaf_hash = hash_share_values(&values);

    let proof: Vec<[u8; 32]> = share.merkle_proof
        .iter()
        .map(|h| {
            let bytes = hex::decode(h).unwrap();
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        })
        .collect();

    let mut current = leaf_hash;
    let mut idx = share.index;

    for sibling in &proof {
        let mut combined = [0u8; 64];
        if idx % 2 == 0 {
            combined[..32].copy_from_slice(&current);
            combined[32..].copy_from_slice(sibling);
        } else {
            combined[..32].copy_from_slice(sibling);
            combined[32..].copy_from_slice(&current);
        }
        current = keccak256(&combined);
        idx /= 2;
    }

    &current == commitment
}

/// Generate calldata for createEscrow
fn create_escrow_calldata(share_set: &ShareSet, share_c_index: usize) -> String {
    // Selector: keccak256("createEscrow(bytes32,bytes32,bytes32)")[:4]
    let selector = "0x7a5c0b3e"; // Pre-computed

    let commitment = &share_set.commitment;
    let escrow_pubkey = &share_set.escrow_pubkey;

    // Share C values as bytes32 (just the raw 32 bytes)
    let share_c = &share_set.shares[share_c_index];
    let mut share_c_bytes = [0u8; 32];
    for (i, &v) in share_c.values.iter().enumerate() {
        share_c_bytes[i * 4..(i + 1) * 4].copy_from_slice(&v.to_le_bytes());
    }

    format!("{}{}{}{}", selector, commitment, escrow_pubkey, hex::encode(share_c_bytes))
}

fn hex_encode_padded(data: &[u8]) -> String {
    let mut s = String::with_capacity(64);
    for _ in 0..(32 - data.len()) {
        s.push_str("00");
    }
    s.push_str(&hex::encode(data));
    s
}

#[derive(Parser)]
#[command(name = "escrow-demo")]
#[command(about = "P2P Escrow Demo CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate VSS shares for a secret
    GenerateShares {
        /// Secret as hex (32 bytes)
        #[arg(short, long)]
        secret: Option<String>,
        /// Threshold (default: 2)
        #[arg(short, long, default_value = "2")]
        threshold: usize,
        /// Number of shares (default: 3)
        #[arg(short, long, default_value = "3")]
        num_shares: usize,
    },
    /// Verify a share against commitment
    VerifyShare {
        /// Commitment as hex
        #[arg(short, long)]
        commitment: String,
        /// Share JSON
        #[arg(short, long)]
        share: String,
    },
    /// Generate calldata for contract interaction
    Calldata {
        /// Share set JSON file
        #[arg(short, long)]
        shares_file: String,
    },
    /// Run full demo flow
    Demo,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenerateShares { secret, threshold, num_shares } => {
            let secret_bytes: [u8; 32] = if let Some(hex_str) = secret {
                let bytes = hex::decode(hex_str.trim_start_matches("0x")).expect("Invalid hex");
                bytes.try_into().expect("Secret must be 32 bytes")
            } else {
                let mut rng = rand::thread_rng();
                let mut s = [0u8; 32];
                rng.fill(&mut s);
                s
            };

            println!("Secret: 0x{}", hex::encode(secret_bytes));
            println!("Threshold: {}-of-{}", threshold, num_shares);
            println!();

            let share_set = generate_shares(&secret_bytes, threshold, num_shares);

            println!("Commitment (Merkle Root): 0x{}", share_set.commitment);
            println!("Escrow Pubkey: 0x{}", share_set.escrow_pubkey);
            println!();

            for share in &share_set.shares {
                println!("Share {}: {:?}", share.index, share.values);
                println!("  Proof: {} elements", share.merkle_proof.len());
            }
            println!();

            // Output JSON
            let json = serde_json::to_string_pretty(&share_set).unwrap();
            println!("Full Share Set JSON:");
            println!("{}", json);
        }

        Commands::VerifyShare { commitment, share } => {
            let commitment_bytes: [u8; 32] = hex::decode(commitment.trim_start_matches("0x"))
                .expect("Invalid commitment hex")
                .try_into()
                .expect("Commitment must be 32 bytes");

            let share: Share = serde_json::from_str(&share).expect("Invalid share JSON");

            let valid = verify_share(&commitment_bytes, &share);
            println!("Share {} verification: {}", share.index, if valid { "VALID ✓" } else { "INVALID ✗" });
        }

        Commands::Calldata { shares_file } => {
            let content = std::fs::read_to_string(&shares_file).expect("Cannot read file");
            let share_set: ShareSet = serde_json::from_str(&content).expect("Invalid JSON");

            let calldata = create_escrow_calldata(&share_set, 2); // Share C is index 2
            println!("createEscrow calldata:");
            println!("{}", calldata);
        }

        Commands::Demo => {
            println!("╔═══════════════════════════════════════════════════════════════╗");
            println!("║           P2P ESCROW WITH VSS - DEMO FLOW                     ║");
            println!("╚═══════════════════════════════════════════════════════════════╝");
            println!();

            // Step 1: Seller generates secret (escrow spending key)
            println!("1. SELLER generates escrow spending key (secret)");
            println!("   ─────────────────────────────────────────────");
            let mut rng = rand::thread_rng();
            let mut secret = [0u8; 32];
            rng.fill(&mut secret);
            println!("   Secret: 0x{}", hex::encode(secret));
            println!();

            // Step 2: Seller creates VSS shares
            println!("2. SELLER creates 2-of-3 VSS shares");
            println!("   ─────────────────────────────────────────────");
            let share_set = generate_shares(&secret, 2, 3);
            println!("   Commitment: 0x{}", share_set.commitment);
            println!("   Escrow Address: 0x{}", share_set.escrow_pubkey);
            println!();
            println!("   Share distribution:");
            println!("   • Share 0 (Buyer):  {:?}", share_set.shares[0].values);
            println!("   • Share 1 (Seller): {:?}", share_set.shares[1].values);
            println!("   • Share 2 (Chain):  {:?}", share_set.shares[2].values);
            println!();

            // Step 3: Buyer verifies their share
            println!("3. BUYER verifies their share against commitment");
            println!("   ─────────────────────────────────────────────");
            let commitment_bytes: [u8; 32] = hex::decode(&share_set.commitment).unwrap().try_into().unwrap();
            let valid = verify_share(&commitment_bytes, &share_set.shares[0]);
            println!("   Verification result: {}", if valid { "VALID ✓" } else { "INVALID ✗" });
            println!();

            // Step 4: Create contract calldata
            println!("4. CONTRACT INTERACTION calldata");
            println!("   ─────────────────────────────────────────────");

            // createEscrow selector
            let create_selector = &keccak256(b"createEscrow(bytes32,bytes32,bytes32)")[..4];
            println!("   createEscrow selector: 0x{}", hex::encode(create_selector));

            // Share C as bytes
            let share_c = &share_set.shares[2];
            let mut share_c_bytes = [0u8; 32];
            for (i, &v) in share_c.values.iter().enumerate() {
                share_c_bytes[i * 4..(i + 1) * 4].copy_from_slice(&v.to_le_bytes());
            }

            println!();
            println!("   Full createEscrow calldata:");
            println!("   0x{}{}{}{}",
                hex::encode(create_selector),
                share_set.commitment,
                share_set.escrow_pubkey,
                hex::encode(share_c_bytes)
            );
            println!();

            // Step 5: Show trade flow
            println!("5. TRADE FLOW");
            println!("   ─────────────────────────────────────────────");
            println!("   ");
            println!("   HAPPY PATH:");
            println!("   a) Seller funds escrow address on Zcash/Penumbra");
            println!("   b) Buyer sends fiat payment");
            println!("   c) Seller confirms → sends Share 1 to Buyer");
            println!("   d) Buyer: Share 0 + Share 1 → reconstruct secret");
            println!("   e) Buyer sweeps escrow on target chain");
            println!("   ");
            println!("   DISPUTE PATH:");
            println!("   a) Either party raises dispute");
            println!("   b) Arbitrators vote");
            println!("   c) Contract emits ShareRevealed event with Share 2");
            println!("   d) Winner: their share + Share 2 → reconstruct secret");
            println!("   e) Winner sweeps escrow on target chain");
            println!();

            // Output JSON for testing
            println!("═══════════════════════════════════════════════════════════════");
            println!("SHARE SET JSON (save to file for testing):");
            println!("═══════════════════════════════════════════════════════════════");
            let json = serde_json::to_string_pretty(&share_set).unwrap();
            println!("{}", json);
        }
    }
}
