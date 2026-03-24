//! PVM execution trace representation.
//!
//! Captures the state at each step of PVM execution in a format
//! ready for translation into binius64 value vectors.

use crate::NUM_REGS;

/// PVM state at a single execution step.
#[derive(Debug, Clone)]
pub struct PvmStep {
    /// Program counter before this instruction executes.
    pub pc: u64,
    /// Program counter after this instruction executes.
    pub next_pc: u64,
    /// Raw instruction word (opcode + operands encoded).
    pub instruction: u32,
    /// Decoded opcode class.
    pub opcode: PvmOpcode,
    /// Destination register index (0-12, or NONE).
    pub rd: u8,
    /// Source register 1 index.
    pub rs1: u8,
    /// Source register 2 index.
    pub rs2: u8,
    /// Immediate value (sign-extended to 64 bits).
    pub imm: u64,
    /// Register file BEFORE this instruction.
    pub regs_before: [u64; NUM_REGS],
    /// Register file AFTER this instruction.
    pub regs_after: [u64; NUM_REGS],
    /// Gas remaining before this instruction.
    pub gas_before: u64,
    /// Gas remaining after this instruction.
    pub gas_after: u64,
    /// Memory access (if any).
    pub mem_access: Option<MemoryAccess>,
}

/// Memory access record.
#[derive(Debug, Clone)]
pub struct MemoryAccess {
    /// Address accessed.
    pub address: u64,
    /// Value read or written.
    pub value: u64,
    /// Access width in bytes (1, 2, 4, 8).
    pub width: u8,
    /// True if store, false if load.
    pub is_store: bool,
    /// Monotonic timestamp for memory ordering.
    pub timestamp: u64,
}

/// PVM opcode classes matching rv64em.
///
/// Grouped by constraint pattern — opcodes in the same group
/// produce the same binius64 gate structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PvmOpcode {
    // R-type integer
    Add = 0,
    Sub = 1,
    And = 2,
    Or = 3,
    Xor = 4,
    Sll = 5,
    Srl = 6,
    Sra = 7,
    Slt = 8,
    Sltu = 9,

    // I-type integer
    Addi = 10,
    Andi = 11,
    Ori = 12,
    Xori = 13,
    Slli = 14,
    Srli = 15,
    Srai = 16,
    Slti = 17,
    Sltiu = 18,

    // Load
    Lb = 20,
    Lh = 21,
    Lw = 22,
    Ld = 23,
    Lbu = 24,
    Lhu = 25,
    Lwu = 26,

    // Store
    Sb = 30,
    Sh = 31,
    Sw = 32,
    Sd = 33,

    // Branch
    Beq = 40,
    Bne = 41,
    Blt = 42,
    Bge = 43,
    Bltu = 44,
    Bgeu = 45,

    // Upper immediate
    Lui = 50,
    Auipc = 51,

    // Jump
    Jal = 52,
    Jalr = 53,

    // M-extension (multiply/divide)
    Mul = 60,
    Mulh = 61,
    Mulhsu = 62,
    Mulhu = 63,
    Div = 64,
    Divu = 65,
    Rem = 66,
    Remu = 67,

    // System
    Ecall = 70,
    Halt = 71,

    // Invalid / padding
    Nop = 255,
}

/// A complete execution trace for a PVM window.
#[derive(Debug, Clone)]
pub struct PvmTrace {
    /// Execution steps.
    pub steps: Vec<PvmStep>,
    /// Hash of the program being executed.
    pub program_hash: [u8; 32],
    /// Initial memory state (Merkle root or hash).
    pub initial_memory_root: [u8; 32],
    /// Final memory state.
    pub final_memory_root: [u8; 32],
}

impl PvmTrace {
    /// Initial register state (from first step).
    pub fn initial_regs(&self) -> Option<[u64; NUM_REGS]> {
        self.steps.first().map(|s| s.regs_before)
    }

    /// Final register state (from last step).
    pub fn final_regs(&self) -> Option<[u64; NUM_REGS]> {
        self.steps.last().map(|s| s.regs_after)
    }

    /// Initial PC.
    pub fn initial_pc(&self) -> Option<u64> {
        self.steps.first().map(|s| s.pc)
    }

    /// Final PC (after last instruction).
    pub fn final_pc(&self) -> Option<u64> {
        self.steps.last().map(|s| s.next_pc)
    }

    /// Total gas consumed.
    pub fn gas_consumed(&self) -> u64 {
        match (self.steps.first(), self.steps.last()) {
            (Some(first), Some(last)) => first.gas_before - last.gas_after,
            _ => 0,
        }
    }

    /// All memory accesses in order.
    pub fn memory_accesses(&self) -> Vec<&MemoryAccess> {
        self.steps.iter().filter_map(|s| s.mem_access.as_ref()).collect()
    }
}
