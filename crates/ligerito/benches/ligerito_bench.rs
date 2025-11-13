use binary_fields::{BinaryElem32, BinaryElem128};
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use ligerito::{
    prove_sha256, verify_sha256,
    hardcoded_config_20, hardcoded_config_20_verifier,
    hardcoded_config_24, hardcoded_config_24_verifier,
    hardcoded_config_28, hardcoded_config_28_verifier,
    hardcoded_config_30, hardcoded_config_30_verifier,
};
use rand::Rng;
use std::marker::PhantomData;

fn generate_random_poly(size: usize) -> Vec<BinaryElem32> {
    let mut rng = rand::thread_rng();
    (0..size)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect()
}

fn bench_proving(c: &mut Criterion) {
    let mut group = c.benchmark_group("proving");
    group.sample_size(10); // Reduce sample size for large benchmarks

    // 2^20 - 4 MiB polynomial
    group.bench_function(BenchmarkId::from_parameter("2^20"), |b| {
        let config = hardcoded_config_20(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 20);

        b.iter(|| {
            let proof = prove_sha256(&config, black_box(&poly)).unwrap();
            black_box(proof)
        });
    });

    // 2^24 - 64 MiB polynomial
    group.bench_function(BenchmarkId::from_parameter("2^24"), |b| {
        let config = hardcoded_config_24(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 24);

        b.iter(|| {
            let proof = prove_sha256(&config, black_box(&poly)).unwrap();
            black_box(proof)
        });
    });

    // 2^28 - 1 GiB polynomial
    group.bench_function(BenchmarkId::from_parameter("2^28"), |b| {
        let config = hardcoded_config_28(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 28);

        b.iter(|| {
            let proof = prove_sha256(&config, black_box(&poly)).unwrap();
            black_box(proof)
        });
    });

    // 2^30 - 4 GiB polynomial
    group.bench_function(BenchmarkId::from_parameter("2^30"), |b| {
        let config = hardcoded_config_30(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 30);

        b.iter(|| {
            let proof = prove_sha256(&config, black_box(&poly)).unwrap();
            black_box(proof)
        });
    });

    group.finish();
}

fn bench_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("verification");
    group.sample_size(100); // More samples for verification (faster)

    // 2^20
    {
        let config = hardcoded_config_20(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 20);
        let proof = prove_sha256(&config, &poly).unwrap();
        let verifier_config = hardcoded_config_20_verifier();

        group.bench_function(BenchmarkId::from_parameter("2^20"), |b| {
            b.iter(|| {
                let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
                black_box(result)
            });
        });
    }

    // 2^24
    {
        let config = hardcoded_config_24(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 24);
        let proof = prove_sha256(&config, &poly).unwrap();
        let verifier_config = hardcoded_config_24_verifier();

        group.bench_function(BenchmarkId::from_parameter("2^24"), |b| {
            b.iter(|| {
                let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
                black_box(result)
            });
        });
    }

    // 2^28
    {
        let config = hardcoded_config_28(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 28);
        let proof = prove_sha256(&config, &poly).unwrap();
        let verifier_config = hardcoded_config_28_verifier();

        group.bench_function(BenchmarkId::from_parameter("2^28"), |b| {
            b.iter(|| {
                let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
                black_box(result)
            });
        });
    }

    // 2^30
    {
        let config = hardcoded_config_30(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let poly = generate_random_poly(1 << 30);
        let proof = prove_sha256(&config, &poly).unwrap();
        let verifier_config = hardcoded_config_30_verifier();

        group.bench_function(BenchmarkId::from_parameter("2^30"), |b| {
            b.iter(|| {
                let result = verify_sha256(black_box(&verifier_config), black_box(&proof)).unwrap();
                black_box(result)
            });
        });
    }

    group.finish();
}

fn bench_proof_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("proof_size");
    group.sample_size(10);

    // Measure proof sizes for different polynomial sizes
    for &log_size in &[20, 24, 28, 30] {
        let size = 1 << log_size;
        let poly = generate_random_poly(size);

        let (config, proof) = match log_size {
            20 => {
                let cfg = hardcoded_config_20(
                    PhantomData::<BinaryElem32>,
                    PhantomData::<BinaryElem128>,
                );
                let p = prove_sha256(&cfg, &poly).unwrap();
                (cfg, p)
            }
            24 => {
                let cfg = hardcoded_config_24(
                    PhantomData::<BinaryElem32>,
                    PhantomData::<BinaryElem128>,
                );
                let p = prove_sha256(&cfg, &poly).unwrap();
                (cfg, p)
            }
            28 => {
                let cfg = hardcoded_config_28(
                    PhantomData::<BinaryElem32>,
                    PhantomData::<BinaryElem128>,
                );
                let p = prove_sha256(&cfg, &poly).unwrap();
                (cfg, p)
            }
            30 => {
                let cfg = hardcoded_config_30(
                    PhantomData::<BinaryElem32>,
                    PhantomData::<BinaryElem128>,
                );
                let p = prove_sha256(&cfg, &poly).unwrap();
                (cfg, p)
            }
            _ => panic!("Unsupported size"),
        };

        let proof_size = proof.size_of();
        println!("2^{} proof size: {} bytes ({:.2} KiB)",
                 log_size, proof_size, proof_size as f64 / 1024.0);
    }

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default();
    targets = bench_proving, bench_verification, bench_proof_size
);
criterion_main!(benches);
