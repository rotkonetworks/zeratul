//! Phase 2 integration tests: Read-only memory support

use ligerito::pcvm::{
    trace::{Program, Instruction, Opcode, execute_and_trace_with_memory},
    memory::ReadOnlyMemory,
};

#[test]
fn test_simple_load() {
    // Create memory with a constant value
    let mut memory = ReadOnlyMemory::with_size(0x2000);
    memory.write(0x1000, 42).unwrap();

    // Program: Load value from memory
    let program = vec![
        Instruction::new_imm(1, 0x1000),     // a1 = 0x1000 (address)
        Instruction::new_load(0, 1, 0),      // a0 = mem[a1 + 0] = mem[0x1000] = 42
        Instruction::halt(),
    ];

    let initial_regs = [0u32; 13];
    let trace = execute_and_trace_with_memory(&program, initial_regs, Some(&memory));

    assert!(trace.validate().is_ok());

    let final_state = trace.final_state().unwrap();
    assert_eq!(final_state[0], 42, "a0 should contain loaded value");
}

#[test]
fn test_load_with_offset() {
    // Create memory with values
    let mut memory = ReadOnlyMemory::with_size(0x2000);
    memory.write(0x1000, 10).unwrap();
    memory.write(0x1004, 20).unwrap();
    memory.write(0x1008, 30).unwrap();

    // Program: Load with offset
    let program = vec![
        Instruction::new_imm(1, 0x1000),     // a1 = 0x1000 (base address)
        Instruction::new_load(0, 1, 0),      // a0 = mem[0x1000] = 10
        Instruction::new_load(2, 1, 4),      // a2 = mem[0x1004] = 20
        Instruction::new_load(3, 1, 8),      // a3 = mem[0x1008] = 30
        Instruction::halt(),
    ];

    let initial_regs = [0u32; 13];
    let trace = execute_and_trace_with_memory(&program, initial_regs, Some(&memory));

    assert!(trace.validate().is_ok());

    let final_state = trace.final_state().unwrap();
    assert_eq!(final_state[0], 10);
    assert_eq!(final_state[2], 20);
    assert_eq!(final_state[3], 30);
}

#[test]
fn test_load_and_compute() {
    // Load two values and compute with them
    let mut memory = ReadOnlyMemory::with_size(0x2000);
    memory.write(0x1000, 15).unwrap();
    memory.write(0x1004, 25).unwrap();

    // Program: Load and add
    let program = vec![
        Instruction::new_imm(1, 0x1000),          // a1 = 0x1000
        Instruction::new_load(0, 1, 0),           // a0 = mem[0x1000] = 15
        Instruction::new_load(2, 1, 4),           // a2 = mem[0x1004] = 25
        Instruction::new_rrr(Opcode::ADD, 3, 0, 2), // a3 = a0 + a2 = 40
        Instruction::halt(),
    ];

    let initial_regs = [0u32; 13];
    let trace = execute_and_trace_with_memory(&program, initial_regs, Some(&memory));

    assert!(trace.validate().is_ok());

    let final_state = trace.final_state().unwrap();
    assert_eq!(final_state[3], 40);
}

#[test]
fn test_memory_hash_integrity() {
    let mut mem1 = ReadOnlyMemory::with_size(100);
    mem1.write(50, 123).unwrap();

    let mut mem2 = ReadOnlyMemory::with_size(100);
    mem2.write(50, 456).unwrap();

    // Different memory contents should have different hashes
    assert_ne!(mem1.hash, mem2.hash);

    // Same contents should have same hash
    let mut mem3 = ReadOnlyMemory::with_size(100);
    mem3.write(50, 123).unwrap();
    assert_eq!(mem1.hash, mem3.hash);
}

#[test]
fn test_backward_compatibility() {
    // Phase 1 programs (no memory) should still work
    let program = vec![
        Instruction::new_imm(0, 10),
        Instruction::new_imm(1, 20),
        Instruction::new_rrr(Opcode::ADD, 2, 0, 1),
        Instruction::halt(),
    ];

    let initial_regs = [0u32; 13];

    // Should work without memory
    let trace = execute_and_trace_with_memory(&program, initial_regs, None);
    assert!(trace.validate().is_ok());

    let final_state = trace.final_state().unwrap();
    assert_eq!(final_state[2], 30);
}

#[test]
fn test_load_out_of_bounds() {
    // Load from invalid address returns 0
    let memory = ReadOnlyMemory::with_size(100);

    let program = vec![
        Instruction::new_imm(1, 1000),       // a1 = 1000 (out of bounds)
        Instruction::new_load(0, 1, 0),      // a0 = mem[1000] = 0 (default)
        Instruction::halt(),
    ];

    let initial_regs = [0u32; 13];
    let trace = execute_and_trace_with_memory(&program, initial_regs, Some(&memory));

    assert!(trace.validate().is_ok());

    let final_state = trace.final_state().unwrap();
    assert_eq!(final_state[0], 0, "Out of bounds read should return 0");
}
