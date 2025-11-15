//! Execution trace for pcVM programs (Phase 1 and Phase 2)

use super::memory::ReadOnlyMemory;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Opcodes supported in Phase 1 and Phase 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    /// rd = rs1 + rs2 (wrapping addition)
    ADD = 0x00,
    /// rd = rs1 - rs2 (wrapping subtraction)
    SUB = 0x01,
    /// rd = rs1 * rs2 (lower 32 bits)
    MUL = 0x02,
    /// rd = rs1 & rs2 (bitwise AND)
    AND = 0x03,
    /// rd = rs1 | rs2 (bitwise OR)
    OR = 0x04,
    /// rd = rs1 ^ rs2 (bitwise XOR)
    XOR = 0x05,
    /// rd = rs1 << rs2[4:0] (logical left shift)
    SLL = 0x06,
    /// rd = rs1 >> rs2[4:0] (logical right shift)
    SRL = 0x07,
    /// rd = immediate value
    LI = 0x08,
    /// rd = mem[rs1 + imm] (Phase 2: load from read-only memory)
    LOAD = 0x09,
    /// Halt execution
    HALT = 0xFF,
}

impl Opcode {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x00 => Some(Opcode::ADD),
            0x01 => Some(Opcode::SUB),
            0x02 => Some(Opcode::MUL),
            0x03 => Some(Opcode::AND),
            0x04 => Some(Opcode::OR),
            0x05 => Some(Opcode::XOR),
            0x06 => Some(Opcode::SLL),
            0x07 => Some(Opcode::SRL),
            0x08 => Some(Opcode::LI),
            0x09 => Some(Opcode::LOAD),
            0xFF => Some(Opcode::HALT),
            _ => None,
        }
    }
}

/// A single execution step in the trace
#[derive(Debug, Clone)]
pub struct RegisterOnlyStep {
    /// Program counter (instruction index)
    pub pc: u32,

    /// Register values BEFORE this instruction executes
    /// Registers: a0-a7 (8 argument/return registers), t0-t4 (5 temporary registers)
    pub regs: [u32; 13],

    /// The instruction being executed
    pub opcode: Opcode,

    /// Destination register index (0-12)
    pub rd: u8,

    /// Source register 1 index (0-12)
    pub rs1: u8,

    /// Source register 2 index (0-12)
    pub rs2: u8,

    /// Immediate value (for LI and LOAD instructions)
    pub imm: u32,

    /// Memory address accessed (Phase 2: Some for LOAD, None otherwise)
    pub memory_address: Option<u32>,

    /// Value read from memory (Phase 2: Some for LOAD, None otherwise)
    pub memory_value: Option<u32>,
}

impl RegisterOnlyStep {
    /// Execute this instruction and return the new register state
    pub fn execute(&self) -> [u32; 13] {
        let mut new_regs = self.regs;

        let result = match self.opcode {
            Opcode::ADD => self.regs[self.rs1 as usize].wrapping_add(self.regs[self.rs2 as usize]),
            Opcode::SUB => self.regs[self.rs1 as usize].wrapping_sub(self.regs[self.rs2 as usize]),
            Opcode::MUL => self.regs[self.rs1 as usize].wrapping_mul(self.regs[self.rs2 as usize]),
            Opcode::AND => self.regs[self.rs1 as usize] & self.regs[self.rs2 as usize],
            Opcode::OR  => self.regs[self.rs1 as usize] | self.regs[self.rs2 as usize],
            Opcode::XOR => self.regs[self.rs1 as usize] ^ self.regs[self.rs2 as usize],
            Opcode::SLL => self.regs[self.rs1 as usize] << (self.regs[self.rs2 as usize] & 0x1F),
            Opcode::SRL => self.regs[self.rs1 as usize] >> (self.regs[self.rs2 as usize] & 0x1F),
            Opcode::LI  => self.imm,
            Opcode::LOAD => self.memory_value.unwrap_or(0), // Value already fetched during trace generation
            Opcode::HALT => 0,
        };

        if self.opcode != Opcode::HALT {
            new_regs[self.rd as usize] = result;
        }

        new_regs
    }
}

/// Complete execution trace of a register-only program
#[derive(Debug, Clone)]
pub struct RegisterOnlyTrace {
    /// All execution steps (one per instruction)
    pub steps: Vec<RegisterOnlyStep>,
}

impl RegisterOnlyTrace {
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// Add a step to the trace
    pub fn push(&mut self, step: RegisterOnlyStep) {
        self.steps.push(step);
    }

    /// Get the initial register state
    pub fn initial_state(&self) -> Option<[u32; 13]> {
        self.steps.first().map(|s| s.regs)
    }

    /// Get the final register state (after last instruction)
    pub fn final_state(&self) -> Option<[u32; 13]> {
        if self.steps.is_empty() {
            return None;
        }

        Some(self.steps.last().unwrap().execute())
    }

    /// Validate that the trace is internally consistent
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.steps.is_empty() {
            return Err("Empty trace");
        }

        // Check PC increments sequentially
        for i in 0..self.steps.len() - 1 {
            if self.steps[i + 1].pc != self.steps[i].pc + 1 {
                return Err("PC does not increment sequentially");
            }
        }

        // Check that each step's next state matches the next step's current state
        for i in 0..self.steps.len() - 1 {
            let expected_next_regs = self.steps[i].execute();
            let actual_next_regs = self.steps[i + 1].regs;

            if expected_next_regs != actual_next_regs {
                return Err("Register state mismatch between steps");
            }
        }

        // Last instruction should be HALT
        if self.steps.last().unwrap().opcode != Opcode::HALT {
            return Err("Trace does not end with HALT");
        }

        Ok(())
    }
}

impl Default for RegisterOnlyTrace {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple instruction encoding for our register-only VM
#[derive(Debug, Clone, Copy)]
pub struct Instruction {
    pub opcode: Opcode,
    pub rd: u8,
    pub rs1: u8,
    pub rs2: u8,
    pub imm: u32,
}

impl Instruction {
    /// Create a new register-register instruction
    pub fn new_rrr(opcode: Opcode, rd: u8, rs1: u8, rs2: u8) -> Self {
        Self { opcode, rd, rs1, rs2, imm: 0 }
    }

    /// Create a new immediate instruction
    pub fn new_imm(rd: u8, imm: u32) -> Self {
        Self { opcode: Opcode::LI, rd, rs1: 0, rs2: 0, imm }
    }

    /// Create a LOAD instruction: rd = mem[rs1 + imm]
    pub fn new_load(rd: u8, rs1: u8, imm: u32) -> Self {
        Self { opcode: Opcode::LOAD, rd, rs1, rs2: 0, imm }
    }

    /// Create a HALT instruction
    pub fn halt() -> Self {
        Self { opcode: Opcode::HALT, rd: 0, rs1: 0, rs2: 0, imm: 0 }
    }
}

/// A simple program is just a list of instructions
pub type Program = Vec<Instruction>;

/// Execute a program and generate a trace (Phase 1: no memory)
pub fn execute_and_trace(program: &Program, initial_regs: [u32; 13]) -> RegisterOnlyTrace {
    execute_and_trace_with_memory(program, initial_regs, None)
}

/// Execute a program with optional memory and generate a trace (Phase 2)
pub fn execute_and_trace_with_memory(
    program: &Program,
    initial_regs: [u32; 13],
    memory: Option<&ReadOnlyMemory>,
) -> RegisterOnlyTrace {
    let mut trace = RegisterOnlyTrace::new();
    let mut regs = initial_regs;

    for (pc, instr) in program.iter().enumerate() {
        // Handle memory access for LOAD instruction
        let (memory_address, memory_value) = if instr.opcode == Opcode::LOAD {
            let addr = regs[instr.rs1 as usize].wrapping_add(instr.imm);
            let value = memory.map(|m| m.read_unchecked(addr)).unwrap_or(0);
            (Some(addr), Some(value))
        } else {
            (None, None)
        };

        let step = RegisterOnlyStep {
            pc: pc as u32,
            regs,
            opcode: instr.opcode,
            rd: instr.rd,
            rs1: instr.rs1,
            rs2: instr.rs2,
            imm: instr.imm,
            memory_address,
            memory_value,
        };

        // Execute and update registers
        regs = step.execute();

        trace.push(step);

        // Stop at HALT
        if instr.opcode == Opcode::HALT {
            break;
        }
    }

    trace
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_addition() {
        // Program: a0 = a1 + a2, then HALT
        let program = vec![
            Instruction::new_rrr(Opcode::ADD, 0, 1, 2),
            Instruction::halt(),
        ];

        // Initial state: a1=5, a2=3
        let mut initial = [0u32; 13];
        initial[1] = 5;
        initial[2] = 3;

        let trace = execute_and_trace(&program, initial);

        assert_eq!(trace.steps.len(), 2);
        assert_eq!(trace.final_state().unwrap()[0], 8); // a0 = 5 + 3
        assert!(trace.validate().is_ok());
    }

    #[test]
    fn test_complex_computation() {
        // Program: a0 = (a1 + a2) * a3
        let program = vec![
            Instruction::new_rrr(Opcode::ADD, 0, 1, 2),  // a0 = a1 + a2
            Instruction::new_rrr(Opcode::MUL, 0, 0, 3),  // a0 = a0 * a3
            Instruction::halt(),
        ];

        // Initial: a1=5, a2=3, a3=2
        let mut initial = [0u32; 13];
        initial[1] = 5;
        initial[2] = 3;
        initial[3] = 2;

        let trace = execute_and_trace(&program, initial);

        assert_eq!(trace.steps.len(), 3);
        assert_eq!(trace.final_state().unwrap()[0], 16); // (5+3)*2 = 16
        assert!(trace.validate().is_ok());
    }
}
