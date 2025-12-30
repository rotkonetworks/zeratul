//! Ligerito benchmarks
//!
//! Usage:
//!   cargo bench -p ligerito              # run all benchmarks
//!   cargo bench -p ligerito -- prove     # proving only
//!   cargo bench -p ligerito -- verify    # verification only
//!   cargo bench -p ligerito -- 20        # 2^20 size only

use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use ligerito::{
    prove_sha256, verify_sha256,
    hardcoded_config_20, hardcoded_config_20_verifier,
    hardcoded_config_24, hardcoded_config_24_verifier,
    hardcoded_config_26, hardcoded_config_26_verifier,
};
use rand::Rng;
use std::marker::PhantomData;

fn generate_random_poly(size: usize) -> Vec<BinaryElem32> {
    let mut rng = rand::thread_rng();
    (0..size)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect()
}

fn bench_prove_20(c: &mut Criterion) {
    let mut group = c.benchmark_group("prove");
    group.sample_size(10);

    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly = generate_random_poly(1 << 20);

    group.bench_function(BenchmarkId::new("sha256", "2^20"), |b| {
        b.iter(|| {
            let proof = prove_sha256(&config, black_box(&poly)).unwrap();
            black_box(proof)
        });
    });

    group.finish();
}

fn bench_prove_24(c: &mut Criterion) {
    let mut group = c.benchmark_group("prove");
    group.sample_size(10);

    let config = hardcoded_config_24(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly = generate_random_poly(1 << 24);

    group.bench_function(BenchmarkId::new("sha256", "2^24"), |b| {
        b.iter(|| {
            let proof = prove_sha256(&config, black_box(&poly)).unwrap();
            black_box(proof)
        });
    });

    group.finish();
}

fn bench_verify_20(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify");
    group.sample_size(50);

    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly = generate_random_poly(1 << 20);
    let proof = prove_sha256(&config, &poly).unwrap();
    let verifier_config = hardcoded_config_20_verifier();

    println!("2^20 proof size: {} bytes ({:.2} KiB)",
             proof.size_of(), proof.size_of() as f64 / 1024.0);

    group.bench_function(BenchmarkId::new("sha256", "2^20"), |b| {
        b.iter(|| {
            let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
            black_box(result)
        });
    });

    group.finish();
}

fn bench_verify_24(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify");
    group.sample_size(50);

    let config = hardcoded_config_24(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly = generate_random_poly(1 << 24);
    let proof = prove_sha256(&config, &poly).unwrap();
    let verifier_config = hardcoded_config_24_verifier();

    println!("2^24 proof size: {} bytes ({:.2} KiB)",
             proof.size_of(), proof.size_of() as f64 / 1024.0);

    group.bench_function(BenchmarkId::new("sha256", "2^24"), |b| {
        b.iter(|| {
            let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
            black_box(result)
        });
    });

    group.finish();
}

fn bench_verify_26(c: &mut Criterion) {
    let mut group = c.benchmark_group("verify");
    group.sample_size(10);

    let config = hardcoded_config_26(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly = generate_random_poly(1 << 26);
    let proof = prove_sha256(&config, &poly).unwrap();
    let verifier_config = hardcoded_config_26_verifier();

    println!("2^26 proof size: {} bytes ({:.2} KiB)",
             proof.size_of(), proof.size_of() as f64 / 1024.0);

    group.bench_function(BenchmarkId::new("sha256", "2^26"), |b| {
        b.iter(|| {
            let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
            black_box(result)
        });
    });

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().without_plots();
    targets = bench_prove_20, bench_prove_24, bench_verify_20, bench_verify_24, bench_verify_26
);
criterion_main!(benches);
