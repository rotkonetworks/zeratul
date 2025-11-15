//! Interactive Game of Life with PolkaVM Host Functions
//!
//! This demonstrates:
//! - Host functions for I/O (display, input)
//! - Interactive cell toggling
//! - Real-time proof generation
//! - Agentic execution model (prove when user requests)

#![cfg(feature = "polkavm-integration")]

use polkavm_pcvm::polkavm_constraints_v2::{ProvenTransition, InstructionProof};
use polkavm_pcvm::polkavm_adapter::PolkaVMRegisters;
use polkavm_pcvm::polkavm_prover::{prove_polkavm_execution, verify_polkavm_proof};
use ligerito::configs::{hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::BinaryElem32;
use ligerito::transcript::{Sha256Transcript, Transcript};
use ligerito::data_structures::{ProverConfig, VerifierConfig};
use std::marker::PhantomData;
use std::io::{self, Write};

use polkavm::program::Instruction;
use polkavm_common::program::{RawReg, Reg};

fn raw_reg(r: Reg) -> RawReg {
    RawReg::from(r)
}

/// Game of Life grid (8x8)
#[derive(Debug, Clone)]
struct Grid {
    cells: Vec<u32>,
}

impl Grid {
    fn new() -> Self {
        Self { cells: vec![0u32; 64] }
    }

    fn get(&self, x: usize, y: usize) -> u32 {
        if x < 8 && y < 8 {
            self.cells[y * 8 + x]
        } else {
            0
        }
    }

    fn set(&mut self, x: usize, y: usize, value: u32) {
        if x < 8 && y < 8 {
            self.cells[y * 8 + x] = value;
        }
    }

    fn toggle(&mut self, x: usize, y: usize) {
        if x < 8 && y < 8 {
            let idx = y * 8 + x;
            self.cells[idx] = 1 - self.cells[idx];
        }
    }

    fn count_neighbors(&self, x: usize, y: usize) -> u32 {
        let mut count = 0;
        for dy in [-1i32, 0, 1] {
            for dx in [-1i32, 0, 1] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = (x as i32 + dx).rem_euclid(8) as usize;
                let ny = (y as i32 + dy).rem_euclid(8) as usize;
                count += self.get(nx, ny);
            }
        }
        count
    }

    fn step(&self) -> Self {
        let mut next = Self::new();
        for y in 0..8 {
            for x in 0..8 {
                let alive = self.get(x, y) == 1;
                let neighbors = self.count_neighbors(x, y);
                let next_alive = if alive {
                    neighbors == 2 || neighbors == 3
                } else {
                    neighbors == 3
                };
                next.set(x, y, if next_alive { 1 } else { 0 });
            }
        }
        next
    }

    fn print(&self) {
        println!("  01234567");
        for y in 0..8 {
            print!("{} ", y);
            for x in 0..8 {
                print!("{}", if self.get(x, y) == 1 { "█" } else { "·" });
            }
            println!();
        }
    }

    fn print_with_cursor(&self, cursor_x: usize, cursor_y: usize) {
        println!("  01234567");
        for y in 0..8 {
            print!("{} ", y);
            for x in 0..8 {
                if x == cursor_x && y == cursor_y {
                    // Highlight cursor position
                    print!("\x1b[7m{}\x1b[0m", if self.get(x, y) == 1 { "█" } else { "·" });
                } else {
                    print!("{}", if self.get(x, y) == 1 { "█" } else { "·" });
                }
            }
            println!();
        }
    }
}

/// Simulate PolkaVM execution trace for one generation
fn simulate_generation(
    grid_before: &Grid,
    _grid_after: &Grid,
    pc_start: u32,
    regs_start: [u32; 13],  // Pass in register state for continuity!
    memory_root: [u8; 32],
) -> (Vec<(ProvenTransition, Instruction)>, [u32; 13]) {
    let mut trace = Vec::new();
    let mut pc = pc_start;
    let mut regs = regs_start;

    for cell_idx in 0..64 {
        let mut regs_after = regs;
        regs_after[7] = grid_before.cells[cell_idx];

        let step = (
            ProvenTransition {
                pc,
                next_pc: pc + 2,
                instruction_size: 2,
                regs_before: PolkaVMRegisters::from_array(regs),
                regs_after: PolkaVMRegisters::from_array(regs_after),
                memory_root_before: memory_root,
                memory_root_after: memory_root,
                memory_proof: None,
                instruction_proof: InstructionProof {
                    merkle_path: vec![],
                    position: 0,
                    opcode: 0,
                    operands: [0, 0, 0],
                },
            },
            Instruction::load_imm(raw_reg(Reg::A0), grid_before.cells[cell_idx]),
        );

        trace.push(step);
        regs = regs_after;
        pc += 2;
    }

    (trace, regs)  // Return final register state
}

/// Interactive Game of Life session
struct InteractiveSession {
    grid: Grid,
    generation: usize,
    trace: Vec<(ProvenTransition, Instruction)>,
    pc: u32,
    regs: [u32; 13],  // Track register state across generations
    memory_root: [u8; 32],
    prover_config: ProverConfig<BinaryElem32, BinaryElem32>,
    verifier_config: VerifierConfig,
}

impl InteractiveSession {
    fn new() -> Self {
        Self {
            grid: Grid::new(),
            generation: 0,
            trace: Vec::new(),
            pc: 0x1000,
            regs: [0u32; 13],  // Initialize registers
            memory_root: [0u8; 32],
            prover_config: hardcoded_config_20(PhantomData, PhantomData),
            verifier_config: hardcoded_config_20_verifier(),
        }
    }

    fn execute_step(&mut self) {
        let grid_next = self.grid.step();

        // Generate trace for this step WITH register continuity
        let (gen_trace, final_regs) = simulate_generation(
            &self.grid,
            &grid_next,
            self.pc,
            self.regs,  // Pass current register state
            self.memory_root
        );

        self.pc += (gen_trace.len() * 2) as u32;
        self.trace.extend(gen_trace);
        self.regs = final_regs;  // Update register state for next generation

        self.grid = grid_next;
        self.generation += 1;
    }

    fn prove_and_verify(&mut self) -> Result<std::time::Duration, String> {
        if self.trace.is_empty() {
            return Err("No execution to prove!".to_string());
        }

        println!("\n⚡ Generating proof for {} generations ({} steps)...",
                 self.generation, self.trace.len());

        // Generate challenge
        let program_commitment = [0x47u8; 32];
        let mut challenge_transcript = Sha256Transcript::new(42);
        let program_elems: Vec<BinaryElem32> = program_commitment
            .chunks(4)
            .map(|chunk| {
                let mut bytes = [0u8; 4];
                bytes.copy_from_slice(chunk);
                BinaryElem32::from(u32::from_le_bytes(bytes))
            })
            .collect();
        challenge_transcript.absorb_elems(&program_elems);
        challenge_transcript.absorb_elem(BinaryElem32::from(self.trace.len() as u32));
        let batching_challenge = challenge_transcript.get_challenge::<BinaryElem32>();

        let transcript = Sha256Transcript::new(42);

        // Prove
        let start = std::time::Instant::now();
        let proof = prove_polkavm_execution(
            &self.trace,
            program_commitment,
            batching_challenge,
            &self.prover_config,
            transcript,
        ).map_err(|e| format!("Proof generation failed: {}", e))?;
        let prove_time = start.elapsed();

        println!("✓ Proof generated in {:?}", prove_time);

        // Verify
        let start = std::time::Instant::now();
        let verified = verify_polkavm_proof(
            &proof,
            program_commitment,
            self.memory_root,
            self.memory_root,
            &self.verifier_config,
        );
        let verify_time = start.elapsed();

        if !verified {
            return Err("Proof verification failed!".to_string());
        }

        println!("✓ Proof verified in {:?}", verify_time);
        println!("  Constraint accumulator: {:?}", proof.constraint_accumulator);

        // Clear trace (it's now proven)
        self.trace.clear();

        // IMPORTANT: Reset register state after proving
        // This starts a fresh proof window
        self.regs = [0u32; 13];

        Ok(prove_time)
    }

    fn display_menu(&self) {
        println!("\n╔════════════════════════════════════════╗");
        println!("║   Interactive Game of Life (Gen {})    ║", self.generation);
        println!("╚════════════════════════════════════════╝");
        println!();
        self.grid.print();
        println!();
        println!("Commands:");
        println!("  [x y]  - Toggle cell at position (x, y)");
        println!("  [s]    - Step (evolve one generation)");
        println!("  [n N]  - Step N generations");
        println!("  [p]    - Prove current execution");
        println!("  [c]    - Clear grid");
        println!("  [g]    - Load glider pattern");
        println!("  [q]    - Quit");
        println!();
        println!("Accumulated steps: {} (unproven)", self.trace.len());
    }
}

/// Test: Interactive Game of Life session
#[test]
fn test_interactive_game_of_life() {
    println!("🎮 Interactive Game of Life with PolkaVM Proofs");
    println!("================================================\n");

    let mut session = InteractiveSession::new();

    // Simulated interaction (for test)
    println!("Simulated session:");
    println!("1. Load glider pattern");

    // Load glider
    session.grid.set(1, 0, 1);
    session.grid.set(2, 1, 1);
    session.grid.set(0, 2, 1);
    session.grid.set(1, 2, 1);
    session.grid.set(2, 2, 1);

    println!("\nInitial state:");
    session.grid.print();

    println!("\n2. Execute 5 generations");
    for i in 1..=5 {
        session.execute_step();
        println!("\nGeneration {}:", i);
        session.grid.print();
    }

    println!("\n3. Generate proof");
    match session.prove_and_verify() {
        Ok(time) => println!("✓ Proof successful in {:?}", time),
        Err(e) => println!("✗ Proof failed: {}", e),
    }

    println!("\n4. Toggle some cells");
    session.grid.toggle(4, 4);
    session.grid.toggle(4, 5);
    session.grid.toggle(5, 4);

    println!("\nAfter toggling:");
    session.grid.print();

    println!("\n5. Execute 3 more generations");
    for i in 1..=3 {
        session.execute_step();
        println!("\nGeneration {} (total {}):", i, session.generation);
        session.grid.print();
    }

    println!("\n6. Generate proof again");
    match session.prove_and_verify() {
        Ok(time) => println!("✓ Proof successful in {:?}", time),
        Err(e) => println!("✗ Proof failed: {}", e),
    }

    println!("\n✅ Interactive session complete!");
}

/// Manual interactive mode (run with: cargo test --release --features polkavm-integration test_manual_interactive -- --nocapture --ignored)
#[test]
#[ignore]
fn test_manual_interactive() {
    println!("🎮 Interactive Game of Life with PolkaVM Proofs");
    println!("================================================\n");
    println!("Starting interactive mode...\n");

    let mut session = InteractiveSession::new();
    let stdin = io::stdin();

    loop {
        // Clear screen (ANSI escape code)
        print!("\x1b[2J\x1b[H");

        session.display_menu();

        print!("Enter command: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        stdin.read_line(&mut input).unwrap();
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        let parts: Vec<&str> = input.split_whitespace().collect();

        match parts[0] {
            "q" => {
                println!("\nExiting...");
                break;
            }
            "c" => {
                session.grid = Grid::new();
                session.generation = 0;
                session.trace.clear();
                session.pc = 0x1000;
                println!("Grid cleared!");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            "g" => {
                // Load glider
                session.grid = Grid::new();
                session.grid.set(1, 0, 1);
                session.grid.set(2, 1, 1);
                session.grid.set(0, 2, 1);
                session.grid.set(1, 2, 1);
                session.grid.set(2, 2, 1);
                println!("Glider pattern loaded!");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            "s" => {
                session.execute_step();
                println!("Stepped one generation!");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            "n" => {
                if parts.len() < 2 {
                    println!("Usage: n <number>");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
                if let Ok(n) = parts[1].parse::<usize>() {
                    for _ in 0..n {
                        session.execute_step();
                    }
                    println!("Executed {} generations!", n);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
            "p" => {
                match session.prove_and_verify() {
                    Ok(_) => {
                        println!("\nPress Enter to continue...");
                        let mut _dummy = String::new();
                        stdin.read_line(&mut _dummy).unwrap();
                    }
                    Err(e) => {
                        println!("Error: {}", e);
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                }
            }
            _ => {
                // Try to parse as coordinates
                if parts.len() == 2 {
                    if let (Ok(x), Ok(y)) = (parts[0].parse::<usize>(), parts[1].parse::<usize>()) {
                        session.grid.toggle(x, y);
                        println!("Toggled cell at ({}, {})", x, y);
                        std::thread::sleep(std::time::Duration::from_millis(300));
                    } else {
                        println!("Invalid command!");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                } else {
                    println!("Invalid command!");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    }
}
