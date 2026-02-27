//! Doom benchmark - measures execution speed and proving overhead
//!
//! Runs doom.polkavm headlessly, measures:
//! - Raw execution time per frame (polkavm JIT)
//! - Steps per frame
//! - Actual proving time for subset of steps

use polkavm::{Config, Engine, Instance, Linker, Module, ModuleConfig, ProgramBlob, Caller};
use std::time::{Duration, Instant};
use ligerito::{prove, verify};
use ligerito::configs::{hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

type Error = Box<dyn std::error::Error + Send + Sync>;

struct State {
    rom: Vec<u8>,
    frame_count: u32,
    gas_before: i64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // Load doom.polkavm
    let doom_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/alice/rotko/polkavm/examples/doom/roms/doom.polkavm".to_string());

    let wad_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "/home/alice/rotko/polkavm/examples/doom/roms/doom1.wad".to_string());

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║              Doom PolkaVM Benchmark                           ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    println!("Loading program: {}", doom_path);
    let program_bytes = std::fs::read(&doom_path)?;
    println!("  Program size: {} bytes", program_bytes.len());

    println!("Loading WAD: {}", wad_path);
    let wad_bytes = std::fs::read(&wad_path)?;
    println!("  WAD size: {} bytes\n", wad_bytes.len());

    // Create engine and module
    let mut config = Config::from_env()?;
    config.set_backend(Some(polkavm::BackendKind::Interpreter));
    let engine = Engine::new(&config)?;

    let mut module_config = ModuleConfig::new();
    module_config.set_page_size(0x4000);
    module_config.set_gas_metering(Some(polkavm::GasMeteringKind::Sync));

    let blob = ProgramBlob::parse(program_bytes.into())?;
    let module = Module::from_blob(&engine, &module_config, blob)?;

    // Setup linker with minimal host functions
    let mut linker = Linker::<State, Error>::new();

    linker.define_typed(
        "ext_output_video",
        |caller: Caller<State>, _address: u32, _width: u32, _height: u32| -> Result<(), Error> {
            caller.user_data.frame_count += 1;
            Ok(())
        },
    )?;

    linker.define_typed(
        "ext_output_audio",
        |_caller: Caller<State>, _address: u32, _samples: u32| -> Result<(), Error> {
            Ok(())
        },
    )?;

    linker.define_typed("ext_rom_size", |caller: Caller<State>| -> u32 {
        caller.user_data.rom.len() as u32
    })?;

    linker.define_typed(
        "ext_rom_read",
        |caller: Caller<State>, pointer: u32, offset: u32, length: u32| -> Result<(), Error> {
            let chunk = caller
                .user_data
                .rom
                .get(offset as usize..offset as usize + length as usize)
                .ok_or_else(|| format!("invalid ROM read: offset = 0x{offset:x}, length = {length}"))?;
            Ok(caller.instance.write_memory(pointer, chunk)?)
        },
    )?;

    linker.define_typed(
        "ext_stdout",
        |_caller: Caller<State>, _buffer: u32, length: u32| -> Result<i32, Error> {
            Ok(length as i32)
        },
    )?;

    // Instantiate
    println!("Instantiating...");
    let instance_pre = linker.instantiate_pre(&module)?;
    let mut instance: Instance<State, Error> = instance_pre.instantiate()?;

    let mut state = State {
        rom: wad_bytes,
        frame_count: 0,
        gas_before: 0,
    };

    // Set initial gas
    let initial_gas: i64 = 1_000_000_000_000; // 1 trillion
    instance.set_gas(initial_gas);

    // Initialize
    println!("Initializing Doom...\n");
    let init_start = Instant::now();
    instance.call_typed(&mut state, "ext_initialize", ()).map_err(|e| format!("{:?}", e))?;
    let init_time = init_start.elapsed();
    println!("Initialization: {:?}\n", init_time);

    // Benchmark frames
    let num_frames = 100;
    println!("Running {} frames...\n", num_frames);

    let mut frame_times: Vec<Duration> = Vec::with_capacity(num_frames);
    let mut gas_per_frame: Vec<i64> = Vec::with_capacity(num_frames);
    let gas_start = instance.gas();

    let total_start = Instant::now();
    for i in 0..num_frames {
        let gas_before = instance.gas();
        let frame_start = Instant::now();
        instance.call_typed(&mut state, "ext_tick", ()).map_err(|e| format!("{:?}", e))?;
        let frame_time = frame_start.elapsed();
        let gas_after = instance.gas();
        let gas_used = gas_before - gas_after;

        frame_times.push(frame_time);
        gas_per_frame.push(gas_used);

        if (i + 1) % 10 == 0 {
            let elapsed = total_start.elapsed();
            let fps = (i + 1) as f64 / elapsed.as_secs_f64();
            print!("\rFrame {}/{}: {:.1} FPS, {} gas/frame", i + 1, num_frames, fps, gas_used);
        }
    }
    let total_time = total_start.elapsed();
    let total_gas = gas_start - instance.gas();
    println!("\n");

    // Calculate stats
    let avg_frame_time = total_time / num_frames as u32;
    let min_frame = frame_times.iter().min().unwrap();
    let max_frame = frame_times.iter().max().unwrap();
    let fps = num_frames as f64 / total_time.as_secs_f64();

    println!("═══════════════════════════════════════════════════════════════");
    println!("                        RESULTS");
    println!("═══════════════════════════════════════════════════════════════\n");

    println!("Execution (PolkaVM JIT):");
    println!("  Frames:     {}", num_frames);
    println!("  Total time: {:?}", total_time);
    println!("  Avg/frame:  {:?}", avg_frame_time);
    println!("  Min frame:  {:?}", min_frame);
    println!("  Max frame:  {:?}", max_frame);
    println!("  FPS:        {:.1}", fps);
    println!();

    // Gas measurements (1 gas ≈ 1 instruction)
    let avg_gas_per_frame = total_gas / num_frames as i64;
    let min_gas = gas_per_frame.iter().min().unwrap();
    let max_gas = gas_per_frame.iter().max().unwrap();

    println!("Gas (instructions) per frame:");
    println!("  Total gas:  {} ({:.1} M)", total_gas, total_gas as f64 / 1_000_000.0);
    println!("  Avg/frame:  {} ({:.1} M)", avg_gas_per_frame, avg_gas_per_frame as f64 / 1_000_000.0);
    println!("  Min frame:  {} ({:.1} M)", min_gas, *min_gas as f64 / 1_000_000.0);
    println!("  Max frame:  {} ({:.1} M)", max_gas, *max_gas as f64 / 1_000_000.0);
    println!();

    // Proving estimates
    // Game of life: 370ms for 640 steps = 1.7 steps/ms
    // Ligerito alone: ~100ms for 1M elements
    // Full constraint gen: ~580 steps/ms (from game of life 370ms for 640 steps)
    let steps_per_ms_with_constraints = 1730u64; // game of life measurement
    let prove_time_per_frame_ms = avg_gas_per_frame as u64 / steps_per_ms_with_constraints;

    println!("Proving estimates (ligerito + constraints):");
    println!("  Per frame:  ~{} ms", prove_time_per_frame_ms);
    println!("  100 frames: ~{:.1} s", prove_time_per_frame_ms as f64 * 100.0 / 1000.0);
    println!();

    // Block time analysis
    let block_time_s = 6.0;
    let gameplay_per_block_s = 3.0;
    let frames_per_block = (gameplay_per_block_s * fps) as u64;
    let prove_time_per_block_ms = frames_per_block * prove_time_per_frame_ms;

    println!("6-second block analysis:");
    println!("  Gameplay window: {:.0}s", gameplay_per_block_s);
    println!("  Frames to prove: {}", frames_per_block);
    println!("  Prove time:      ~{:.1}s (estimated)", prove_time_per_block_ms as f64 / 1000.0);
    println!("  Time budget:     {:.0}s", block_time_s - gameplay_per_block_s);
    println!();

    // Actually benchmark ligerito proving at multiple scales
    println!("═══════════════════════════════════════════════════════════════");
    println!("           ACTUAL LIGERITO PROVING BENCHMARK");
    println!("═══════════════════════════════════════════════════════════════\n");

    // Test at multiple scales
    let scales = vec![
        (20, "1M"),
        (24, "16M"),
        (28, "256M"),
    ];

    for (log_size, label) in &scales {
        let poly_size: usize = 1 << log_size;
        println!("────────────────────────────────────────");
        println!("2^{} = {} elements ({} bytes)", log_size, poly_size, poly_size * 4);
        println!("────────────────────────────────────────");

        // Check memory
        let required_mb = (poly_size * 4) / (1024 * 1024);
        if required_mb > 8000 {
            println!("  SKIPPED: requires {}MB RAM\n", required_mb);
            continue;
        }

        print!("  Generating polynomial... ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let gen_start = Instant::now();
        let poly: Vec<BinaryElem32> = (0..poly_size)
            .map(|i| BinaryElem32::from(i as u32))
            .collect();
        println!("{:?}", gen_start.elapsed());

        // Get matching prover/verifier configs
        let (prover_config, verifier_config) = match *log_size {
            20 => (
                hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>),
                hardcoded_config_20_verifier()
            ),
            24 => (
                ligerito::configs::hardcoded_config_24(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>),
                ligerito::configs::hardcoded_config_24_verifier()
            ),
            28 => (
                ligerito::configs::hardcoded_config_28(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>),
                ligerito::configs::hardcoded_config_28_verifier()
            ),
            _ => continue,
        };

        print!("  Proving... ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let prove_start = Instant::now();
        let proof = match prove(&prover_config, &poly) {
            Ok(p) => p,
            Err(e) => {
                println!("FAILED: {:?}\n", e);
                continue;
            }
        };
        let prove_time = prove_start.elapsed();
        println!("{:?}", prove_time);

        print!("  Verifying... ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let verify_start = Instant::now();
        let valid = verify(&verifier_config, &proof).unwrap_or(false);
        let verify_time = verify_start.elapsed();
        println!("{:?} (valid: {})", verify_time, valid);

        let proof_size = std::mem::size_of_val(&proof);
        println!("  Proof size: ~{} KB", proof_size / 1024);

        // Calculate throughput
        let elements_per_sec = poly_size as f64 / prove_time.as_secs_f64();
        println!("  Throughput: {:.1}M elements/sec", elements_per_sec / 1_000_000.0);

        // How many doom frames could this prove?
        let doom_steps_provable = poly_size as f64;
        let doom_frames = doom_steps_provable / avg_gas_per_frame as f64;
        println!("  Doom frames at this size: {:.1}", doom_frames);
        println!();

        // Drop poly to free memory before next iteration
        drop(poly);
    }

    // Summary for doom
    println!("═══════════════════════════════════════════════════════════════");
    println!("                    DOOM PROVING ANALYSIS");
    println!("═══════════════════════════════════════════════════════════════\n");

    let doom_3s_steps = (avg_gas_per_frame as f64 * 96.0) as u64; // 96 frames at 32fps
    println!("3 seconds of doom gameplay:");
    println!("  Frames: 96 (at 32 fps)");
    println!("  Total instructions: {} ({:.2}B)", doom_3s_steps, doom_3s_steps as f64 / 1_000_000_000.0);
    println!("  Required polynomial size: 2^{:.0}", (doom_3s_steps as f64).log2().ceil());
    println!();

    println!("NOTE: This is ligerito polynomial commitment only.");
    println!("      Full constraint generation adds significant overhead.");
    println!();

    Ok(())
}
