//! Benchmark trial decryption throughput (native)
//!
//! Run with: cargo bench -p zafu-wasm

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use pasta_curves::pallas;
use ff::PrimeField;
use group::GroupEncoding;
use blake2::{Blake2b512, Digest};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use rayon::prelude::*;

/// Simulate trial decryption (same logic as WASM)
fn try_decrypt_action(ivk: &pallas::Scalar, epk_bytes: &[u8; 32], nullifier: &[u8; 32], ciphertext: &[u8; 52]) -> Option<u64> {
    // Parse ephemeral key
    let epk = pallas::Affine::from_bytes(epk_bytes);
    if epk.is_none().into() {
        return None;
    }
    let epk = epk.unwrap();

    // Compute shared secret
    let shared_secret = (epk * ivk).to_bytes();

    // Derive symmetric key
    let mut hasher = Blake2b512::new();
    hasher.update(b"Zcash_OrchardKDF_");
    hasher.update(&shared_secret);
    hasher.update(epk_bytes);
    let hash = hasher.finalize();
    let mut sym_key = [0u8; 32];
    sym_key.copy_from_slice(&hash[..32]);

    // Decrypt
    let cipher = ChaCha20Poly1305::new_from_slice(&sym_key).ok()?;
    let nonce: [u8; 12] = nullifier[..12].try_into().ok()?;
    let plaintext = cipher.decrypt((&nonce).into(), &ciphertext[..]).ok()?;

    if plaintext.len() < 19 {
        return None;
    }

    let value = u64::from_le_bytes(plaintext[11..19].try_into().ok()?);
    Some(value)
}

/// Generate random IVK
fn random_ivk() -> pallas::Scalar {
    use rand::RngCore;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 32];
    rng.fill_bytes(&mut bytes);
    pallas::Scalar::from_repr(bytes).unwrap_or(pallas::Scalar::zero())
}

/// Generate random action data
fn random_actions(count: usize) -> Vec<([u8; 32], [u8; 32], [u8; 32], [u8; 52])> {
    use rand::RngCore;
    let mut rng = rand::thread_rng();

    (0..count)
        .map(|_| {
            let mut nullifier = [0u8; 32];
            let mut cmx = [0u8; 32];
            let mut epk = [0u8; 32];
            let mut ciphertext = [0u8; 52];
            rng.fill_bytes(&mut nullifier);
            rng.fill_bytes(&mut cmx);
            rng.fill_bytes(&mut epk);
            rng.fill_bytes(&mut ciphertext);
            (nullifier, cmx, epk, ciphertext)
        })
        .collect()
}

fn bench_trial_decrypt_sequential(c: &mut Criterion) {
    let ivk = random_ivk();
    let actions = random_actions(10000);

    let mut group = c.benchmark_group("trial_decrypt_sequential");
    group.throughput(Throughput::Elements(actions.len() as u64));

    group.bench_function("10k_actions", |b| {
        b.iter(|| {
            let mut found = 0u64;
            for (nullifier, _cmx, epk, ciphertext) in &actions {
                if let Some(value) = try_decrypt_action(&ivk, epk, nullifier, ciphertext) {
                    found += value;
                }
            }
            black_box(found)
        })
    });

    group.finish();
}

fn bench_trial_decrypt_parallel(c: &mut Criterion) {
    let ivk = random_ivk();
    let actions = random_actions(10000);

    let mut group = c.benchmark_group("trial_decrypt_parallel");
    group.throughput(Throughput::Elements(actions.len() as u64));

    group.bench_function("10k_actions", |b| {
        b.iter(|| {
            let found: u64 = actions
                .par_iter()
                .filter_map(|(nullifier, _cmx, epk, ciphertext)| {
                    try_decrypt_action(&ivk, epk, nullifier, ciphertext)
                })
                .sum();
            black_box(found)
        })
    });

    group.finish();
}

fn bench_scaling(c: &mut Criterion) {
    let ivk = random_ivk();

    let mut group = c.benchmark_group("scaling_parallel");

    for size in [1000, 5000, 10000, 50000, 100000] {
        let actions = random_actions(size);
        group.throughput(Throughput::Elements(size as u64));

        group.bench_function(format!("{}_actions", size), |b| {
            b.iter(|| {
                let found: u64 = actions
                    .par_iter()
                    .filter_map(|(nullifier, _cmx, epk, ciphertext)| {
                        try_decrypt_action(&ivk, epk, nullifier, ciphertext)
                    })
                    .sum();
                black_box(found)
            })
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_trial_decrypt_sequential,
    bench_trial_decrypt_parallel,
    bench_scaling,
);
criterion_main!(benches);
