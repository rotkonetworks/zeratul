//! Homeostatic 30% blend tests
use poker_pvm::cfr::brain::Brain;
use poker_pvm::cfr::moe_native::NativeMoE;
use poker_pvm::cfr::tournament::run_tournament_seeded;

fn run_match(a_fn: &dyn Fn() -> Brain, b_fn: &dyn Fn() -> Brain, hands: u32, id: u64) -> f64 {
    let seed = 0xA5A5 ^ (id.wrapping_mul(0x9E3779B97F4A7C15));
    let mut a1 = a_fn();
    let mut b1 = b_fn();
    let r1 = run_tournament_seeded(&mut a1, &mut b1, hands, 10, 1000, seed);
    let mut a2 = b_fn();
    let mut b2 = a_fn();
    let r2 = run_tournament_seeded(&mut a2, &mut b2, hands, 10, 1000, seed);
    (r1.p0_bb_per_100 - r2.p0_bb_per_100) / 2.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let strategy_path = args.get(1).map(|s| s.as_str()).unwrap_or("strategy.bin");
    let moe_path = args.get(2).map(|s| s.as_str()).unwrap_or("moe_native_v5.bin");
    let hands: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2000);

    let strategy = std::fs::read(strategy_path)
        .unwrap_or_else(|e| { eprintln!("failed to load {}: {}", strategy_path, e); std::process::exit(1); });

    let moe_file = moe_path.to_string();

    println!("=== Homeostatic 30% Blend Tests ({} hands) ===\n", hands);

    // 1. BP vs BP
    {
        let s = strategy.clone();
        let e = run_match(&|| Brain::new(&s), &|| Brain::new(&s), hands, 1);
        println!("BP vs BP (sanity):              {:>+7.1}", e);
    }

    // 2. MoE 30% vs BP
    {
        let s = strategy.clone();
        let m = moe_file.clone();
        let e = run_match(
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; Brain::with_native_moe(&s, moe) },
            &|| Brain::new(&s),
            hands, 2,
        );
        println!("MoE 30% vs BP:                 {:>+7.1}", e);
    }

    // 3. MoE 30% + homeostatic vs BP
    {
        let s = strategy.clone();
        let m = moe_file.clone();
        let e = run_match(
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; moe.enable_hebbian(0.0001); Brain::with_native_moe(&s, moe) },
            &|| Brain::new(&s),
            hands, 3,
        );
        println!("MoE 30% + homeo vs BP:         {:>+7.1}", e);
    }

    // 4. MoE 30% vs MoE 30% (mirror — should be ~0)
    {
        let s = strategy.clone();
        let m = moe_file.clone();
        let e = run_match(
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; Brain::with_native_moe(&s, moe) },
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; Brain::with_native_moe(&s, moe) },
            hands, 4,
        );
        println!("MoE 30% vs MoE 30% (mirror):   {:>+7.1}  (expect ≈0)", e);
    }

    // 5. MoE 30% + homeo vs MoE 30% + homeo (arms race!)
    {
        let s = strategy.clone();
        let m = moe_file.clone();
        let e = run_match(
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; moe.enable_hebbian(0.0001); Brain::with_native_moe(&s, moe) },
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; moe.enable_hebbian(0.0001); Brain::with_native_moe(&s, moe) },
            hands, 5,
        );
        println!("Homeo 30% vs Homeo 30% (race): {:>+7.1}  (expect ≈0)", e);
    }

    // 6. MoE 30% + homeo vs MoE 30% (no heb) — does adaptation beat static?
    {
        let s = strategy.clone();
        let m = moe_file.clone();
        let e = run_match(
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; moe.enable_hebbian(0.0001); Brain::with_native_moe(&s, moe) },
            &|| { let mut moe = NativeMoE::load(&m).unwrap(); moe.blend_weight = 0.3; Brain::with_native_moe(&s, moe) },
            hands, 6,
        );
        println!("Homeo 30% vs MoE 30% (static): {:>+7.1}  (pos = homeo helps)", e);
    }

    // 7. v9 (search-trained) 30% vs v5 30%
    {
        let s = strategy.clone();
        let m5 = moe_file.clone();
        let m9_path = m5.replace("v5", "v9");
        if std::fs::metadata(&m9_path).is_ok() {
            let e = run_match(
                &|| { let mut moe = NativeMoE::load(&m9_path).unwrap(); moe.blend_weight = 0.3; Brain::with_native_moe(&s, moe) },
                &|| { let mut moe = NativeMoE::load(&m5).unwrap(); moe.blend_weight = 0.3; Brain::with_native_moe(&s, moe) },
                hands, 7,
            );
            println!("v9 (search) vs v5 (blueprint):  {:>+7.1}  (pos = v9 better)", e);
        }
    }
}
