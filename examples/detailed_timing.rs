/// detailed timing breakdown of ligerito proving
/// measures time spent in each major component

use ligerito::*;
use ligerito::transcript::Transcript;
use ligerito::{ligero, data_structures, transcript, utils, sumcheck_polys};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;
use std::time::Instant;
use rayon::prelude::*;

fn main() {
    println!("=== detailed ligerito timing breakdown ===\n");

    let size = 1048576; // 2^20
    let poly: Vec<BinaryElem32> = (0u32..size as u32)
        .map(|i| BinaryElem32::from(i % 0xFFFFFFFF))
        .collect();

    let config = hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);

    println!("polynomial size: 2^20 = {} elements\n", size);

    // warmup
    let _ = prove_sha256(&config, &poly);

    println!("=== detailed timing (median of 3 runs) ===\n");

    let mut times = vec![];
    for run in 0..3 {
        println!("run {}:", run + 1);
        let t = time_prove(&config, &poly);
        times.push(t);
        println!();
    }

    // compute median
    times.sort_by(|a, b| a.total.partial_cmp(&b.total).unwrap());
    let median = &times[1];

    println!("=== median timing breakdown ===\n");
    print_timing(median);

    println!("\n=== optimization opportunities ===\n");
    analyze_bottlenecks(median);
}

struct Timings {
    total: f64,
    initial_commit: f64,
    initial_challenges: f64,
    partial_eval: f64,
    recursive_commit: f64,
    query_selection: f64,
    merkle_open: f64,
    sumcheck_induce: f64,
    sumcheck_rounds: f64,
}

fn time_prove(config: &ProverConfig<BinaryElem32, BinaryElem128>, poly: &[BinaryElem32]) -> Timings {
    let total_start = Instant::now();

    // initial commit - break down into components
    let t0_total = Instant::now();

    // poly2mat
    let t0 = Instant::now();
    let mut poly_mat = ligero::poly2mat(poly, config.initial_dims.0, config.initial_dims.1, 4);
    let poly2mat_time = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  poly2mat: {:.2}ms", poly2mat_time);

    // encode_cols (FFT)
    let t0 = Instant::now();
    ligero::encode_cols(&mut poly_mat, &config.initial_reed_solomon, true);
    let encode_time = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  encode_cols (FFT): {:.2}ms", encode_time);

    // hash rows
    let t0 = Instant::now();
    let hashed_rows: Vec<_> = poly_mat.par_iter()
        .map(|row| ligero::hash_row(row))
        .collect();
    let hash_time = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  hash rows (SHA256): {:.2}ms", hash_time);

    // build merkle tree
    let t0 = Instant::now();
    let tree = merkle_tree::build_merkle_tree(&hashed_rows);
    let merkle_time = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  build merkle tree: {:.2}ms", merkle_time);

    let wtns_0 = data_structures::RecursiveLigeroWitness { mat: poly_mat, tree };
    let initial_commit = t0_total.elapsed().as_secs_f64() * 1000.0;
    println!("  initial commit total: {:.2}ms", initial_commit);

    // initial challenges
    let t0 = Instant::now();
    let mut fs = transcript::Sha256Transcript::new(0);
    fs.absorb_root(&wtns_0.tree.get_root());
    let partial_evals_0: Vec<BinaryElem32> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();
    let initial_challenges = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  get initial challenges: {:.2}ms", initial_challenges);

    // partial evaluation
    let t0 = Instant::now();
    let mut f_evals = poly.to_vec();
    utils::partial_eval_multilinear(&mut f_evals, &partial_evals_0);
    let partial_eval = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  partial eval multilinear: {:.2}ms", partial_eval);

    // recursive commit
    let t0 = Instant::now();
    let partial_evals_0_u: Vec<BinaryElem128> = partial_evals_0.iter().map(|&x| BinaryElem128::from(x)).collect();
    let f_evals_u: Vec<BinaryElem128> = f_evals.iter().map(|&x| BinaryElem128::from(x)).collect();
    let wtns_1 = ligero::ligero_commit(&f_evals_u, config.dims[0].0, config.dims[0].1, &config.reed_solomon_codes[0]);
    fs.absorb_root(&wtns_1.tree.get_root());
    let recursive_commit = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  recursive commit: {:.2}ms", recursive_commit);

    // query selection
    let t0 = Instant::now();
    let rows = wtns_0.mat.len();
    let queries = fs.get_distinct_queries(rows, 148);
    let alpha = fs.get_challenge::<BinaryElem128>();
    let query_selection = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  query selection: {:.2}ms", query_selection);

    // merkle opening
    let t0 = Instant::now();
    let opened_rows: Vec<Vec<BinaryElem32>> = queries.iter()
        .map(|&q| wtns_0.mat[q].clone())
        .collect();
    let mtree_proof = wtns_0.tree.prove(&queries);
    let merkle_open = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  merkle opening: {:.2}ms", merkle_open);

    // sumcheck induce
    let t0 = Instant::now();
    let n = f_evals.len().trailing_zeros() as usize;
    let sks_vks: Vec<BinaryElem32> = utils::eval_sk_at_vks(1 << n);
    let (basis_poly, enforced_sum) = sumcheck_polys::induce_sumcheck_poly(
        n,
        &sks_vks,
        &opened_rows,
        &partial_evals_0_u,
        &queries,
        alpha,
    );
    let sumcheck_induce = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  sumcheck induce: {:.2}ms", sumcheck_induce);

    // sumcheck rounds
    let t0 = Instant::now();
    let mut current_poly = basis_poly;
    fs.absorb_elem(enforced_sum);

    for _i in 0..config.recursive_steps {
        for _j in 0..config.ks[_i] {
            let s0 = current_poly.iter().step_by(2).fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
            let s1 = current_poly.iter().skip(1).step_by(2).fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));

            fs.absorb_elem(s0);
            fs.absorb_elem(s1);

            let r = fs.get_challenge::<BinaryElem128>();

            let one_minus_r = BinaryElem128::one().add(&r);
            let half = current_poly.len() / 2;
            for k in 0..half {
                current_poly[k] = current_poly[2*k].mul(&one_minus_r).add(&current_poly[2*k+1].mul(&r));
            }
            current_poly.truncate(half);
        }
    }
    let sumcheck_rounds = t0.elapsed().as_secs_f64() * 1000.0;
    println!("  sumcheck rounds (folding): {:.2}ms", sumcheck_rounds);

    let total = total_start.elapsed().as_secs_f64() * 1000.0;
    println!("  total: {:.2}ms", total);

    Timings {
        total,
        initial_commit,
        initial_challenges,
        partial_eval,
        recursive_commit,
        query_selection,
        merkle_open,
        sumcheck_induce,
        sumcheck_rounds,
    }
}

fn print_timing(t: &Timings) {
    let components = [
        ("initial commit", t.initial_commit),
        ("initial challenges", t.initial_challenges),
        ("partial eval", t.partial_eval),
        ("recursive commit", t.recursive_commit),
        ("query selection", t.query_selection),
        ("merkle opening", t.merkle_open),
        ("sumcheck induce", t.sumcheck_induce),
        ("sumcheck rounds", t.sumcheck_rounds),
    ];

    println!("{:>25} {:>12} {:>10}", "component", "time (ms)", "% of total");
    println!("{:-<50}", "");

    for (name, time) in components {
        let pct = (time / t.total) * 100.0;
        println!("{:>25} {:>10.2} ms {:>8.1}%", name, time, pct);
    }

    println!("{:-<50}", "");
    println!("{:>25} {:>10.2} ms {:>8}%", "total", t.total, "100.0");
}

fn analyze_bottlenecks(t: &Timings) {
    let mut hotspots = vec![
        ("initial commit", t.initial_commit),
        ("recursive commit", t.recursive_commit),
        ("sumcheck induce", t.sumcheck_induce),
        ("sumcheck rounds", t.sumcheck_rounds),
    ];

    hotspots.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    println!("top 3 hotspots:");
    for (i, (name, time)) in hotspots.iter().take(3).enumerate() {
        let pct = (time / t.total) * 100.0;
        println!("  {}. {} - {:.2}ms ({:.1}%)", i+1, name, time, pct);
    }

    println!("\noptimization recommendations:");

    if hotspots[0].0 == "initial commit" || hotspots[1].0 == "initial commit" {
        println!("  • initial commit (reed-solomon FFT + merkle):");
        println!("    - parallelize merkle tree construction");
        println!("    - use AVX-512 for FFT butterfly operations");
    }

    if hotspots[0].0 == "sumcheck induce" || hotspots[1].0 == "sumcheck induce" {
        println!("  • sumcheck induce:");
        println!("    - use arena allocator for thread-local buffers");
        println!("    - vectorize dot product with AVX-512");
        println!("    - batch field multiplications using VPCLMULQDQ");
    }

    if hotspots[0].0 == "sumcheck rounds" || hotspots[1].0 == "sumcheck rounds" {
        println!("  • sumcheck rounds (folding):");
        println!("    - vectorize fold operation with AVX-512");
        println!("    - use batch_mul/batch_add from simd module");
        println!("    - reduce allocations with in-place operations");
    }
}
