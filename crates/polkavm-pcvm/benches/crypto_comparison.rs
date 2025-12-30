//! Benchmarks for secure 128-bit cryptographic primitives
//!
//! Measures real-world performance of Rescue-Prime hash, 128-bit merkle trees,
//! and unified memory operations.

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

use polkavm_pcvm::rescue::RescueHash;
use polkavm_pcvm::merkle128::MerkleTree128;
use polkavm_pcvm::unified_memory128::UnifiedMemory128;

use ligerito_binary_fields::{BinaryElem128, BinaryFieldElement};

fn bench_rescue_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("rescue_hash");

    // Single element hash
    group.bench_function("single_element", |b| {
        let input = vec![BinaryElem128::from(0x123456789abcdef0u128)];
        b.iter(|| {
            black_box(RescueHash::hash_elements(&input))
        })
    });

    // Batch hash (typical merkle leaf)
    for size in [2, 4, 8, 16].iter() {
        group.bench_with_input(BenchmarkId::new("batch", size), size, |b, &size| {
            let input: Vec<BinaryElem128> = (0..size as u128)
                .map(|i| BinaryElem128::from(i * 0x123456789abcdef0))
                .collect();
            b.iter(|| {
                black_box(RescueHash::hash_elements(&input))
            })
        });
    }

    group.finish();
}

fn bench_merkle_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("merkle_tree_128");

    // Tree creation
    for size in [16, 64, 256, 1024].iter() {
        group.bench_with_input(BenchmarkId::new("create", size), size, |b, &size| {
            let leaves: Vec<u128> = (0..size as u128).collect();
            b.iter(|| {
                black_box(MerkleTree128::new(leaves.clone()).unwrap())
            })
        });
    }

    group.finish();
}

fn bench_merkle_proof(c: &mut Criterion) {
    let mut group = c.benchmark_group("merkle_proof_128");

    for size in [64, 256, 1024].iter() {
        let leaves: Vec<u128> = (0..*size as u128).collect();
        let tree = MerkleTree128::new(leaves).unwrap();

        group.bench_with_input(BenchmarkId::new("prove", size), size, |b, &size| {
            b.iter(|| {
                black_box(tree.prove(size / 2).unwrap())
            })
        });

        let proof = tree.prove(*size / 2).unwrap();
        group.bench_with_input(BenchmarkId::new("verify", size), size, |b, _| {
            b.iter(|| {
                black_box(proof.verify())
            })
        });
    }

    group.finish();
}

fn bench_unified_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("unified_memory_128");

    let program: Vec<u8> = (0u8..=255).collect();

    // Creation
    for word_count in [256, 1024, 4096].iter() {
        group.bench_with_input(BenchmarkId::new("create", word_count), word_count, |b, &wc| {
            b.iter(|| {
                black_box(UnifiedMemory128::with_program(&program, wc).unwrap())
            })
        });
    }

    // Instruction fetch
    let mem = UnifiedMemory128::with_program(&program, 1024).unwrap();
    group.bench_function("fetch_instruction", |b| {
        b.iter(|| {
            black_box(mem.fetch_instruction(128).unwrap())
        })
    });

    group.finish();
}

fn bench_field_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("field_128bit");

    let x = BinaryElem128::from(0x123456789abcdef0123456789abcdef0u128);
    let y = BinaryElem128::from(0xfedcba9876543210fedcba9876543210u128);

    group.bench_function("multiply", |b| {
        b.iter(|| black_box(x.mul(&y)))
    });

    group.bench_function("inverse", |b| {
        b.iter(|| black_box(x.inv()))
    });

    group.bench_function("add", |b| {
        b.iter(|| black_box(x.add(&y)))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_rescue_hash,
    bench_merkle_tree,
    bench_merkle_proof,
    bench_unified_memory,
    bench_field_ops,
);

criterion_main!(benches);
