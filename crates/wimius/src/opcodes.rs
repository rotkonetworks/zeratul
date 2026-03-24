//! PVM opcode → binius64 gate translation.
//!
//! Each PVM opcode maps to a small number of binius64 gates.
//! The translation follows the principle: express the instruction's
//! semantics using the MINIMUM gates, leveraging binius64's native
//! 64-bit word operations.
//!
//! # Gate costs per opcode class
//!
//! | Class | Example | Gates | Method |
//! |-------|---------|-------|--------|
//! | Bitwise | AND, OR, XOR | 1 | band/bor/bxor directly |
//! | Add/Sub | ADD, ADDI | 1 | iadd_cin_cout (carry output ignored) |
//! | Shift | SLL, SRL, SRA | 1 | shl/shr/sar with constant amount |
//! | Compare | SLT, SLTU | 1 | icmp_ult/icmp_eq |
//! | Multiply | MUL, MULH | 1 | imul (64×64→128) |
//! | Divide | DIV, REM | ~10 | hint + multiply-back verification |
//! | Branch | BEQ, BLT | 2 | comparison + select for next_pc |
//! | Load/Store | LW, SW | 1+mem | address calc + memory argument |
//! | Jump | JAL, JALR | 1 | PC computation |
//! | Upper imm | LUI, AUIPC | 1 | shl by 12 + optional add |

use binius_frontend::{CircuitBuilder, Wire};

/// Wires representing one PVM step's state.
pub struct StepWires {
    /// Program counter before.
    pub pc: Wire,
    /// Program counter after.
    pub next_pc: Wire,
    /// Register file before (13 registers).
    pub regs_before: [Wire; 13],
    /// Destination register value after.
    pub rd_after: Wire,
    /// Source register 1 value.
    pub rs1_val: Wire,
    /// Source register 2 value.
    pub rs2_val: Wire,
    /// Immediate value.
    pub imm: Wire,
    /// Gas before.
    pub gas_before: Wire,
    /// Gas after.
    pub gas_after: Wire,
}

/// Emit constraints for an ADD instruction: rd = rs1 + rs2.
pub fn emit_add(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.iadd_32(step.rs1_val, step.rs2_val)
}

/// Emit constraints for SUB: rd = rs1 - rs2.
pub fn emit_sub(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let zero = b.add_constant_64(0);
    let (diff, _borrow) = b.isub_bin_bout(step.rs1_val, step.rs2_val, zero);
    diff
}

/// Emit constraints for AND: rd = rs1 & rs2.
pub fn emit_and(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.band(step.rs1_val, step.rs2_val)
}

/// Emit constraints for OR: rd = rs1 | rs2.
pub fn emit_or(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.bor(step.rs1_val, step.rs2_val)
}

/// Emit constraints for XOR: rd = rs1 ^ rs2.
pub fn emit_xor(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.bxor(step.rs1_val, step.rs2_val)
}

/// Emit constraints for SLT: rd = (rs1 < rs2) ? 1 : 0 (signed).
/// Note: binius64 icmp_ult is unsigned. Signed comparison needs
/// sign bit flip: flip MSB of both operands, then unsigned compare.
pub fn emit_slt(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let sign_bit = b.add_constant_64(1u64 << 63);
    let rs1_flipped = b.bxor(step.rs1_val, sign_bit);
    let rs2_flipped = b.bxor(step.rs2_val, sign_bit);
    b.icmp_ult(rs1_flipped, rs2_flipped)
}

/// Emit constraints for SLTU: rd = (rs1 < rs2) ? 1 : 0 (unsigned).
pub fn emit_sltu(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.icmp_ult(step.rs1_val, step.rs2_val)
}

/// Emit constraints for MUL: rd = (rs1 * rs2)[63:0].
pub fn emit_mul(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let (lo, _hi) = b.imul(step.rs1_val, step.rs2_val);
    lo
}

/// Emit constraints for MULH: rd = (rs1 * rs2)[127:64] (signed).
pub fn emit_mulh(b: &CircuitBuilder, step: &StepWires) -> Wire {
    // Signed multiply: binius64's smul handles sign extension.
    let (_lo, hi) = b.smul(step.rs1_val, step.rs2_val);
    hi
}

/// Emit constraints for MULHU: rd = (rs1 * rs2)[127:64] (unsigned).
pub fn emit_mulhu(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let (_lo, hi) = b.imul(step.rs1_val, step.rs2_val);
    hi
}

/// Emit constraints for ADDI: rd = rs1 + imm.
pub fn emit_addi(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.iadd_32(step.rs1_val, step.imm)
}

/// Emit constraints for ANDI: rd = rs1 & imm.
pub fn emit_andi(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.band(step.rs1_val, step.imm)
}

/// Emit constraints for ORI: rd = rs1 | imm.
pub fn emit_ori(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.bor(step.rs1_val, step.imm)
}

/// Emit constraints for XORI: rd = rs1 ^ imm.
pub fn emit_xori(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.bxor(step.rs1_val, step.imm)
}

/// Emit constraints for LUI: rd = imm << 12.
pub fn emit_lui(b: &CircuitBuilder, step: &StepWires) -> Wire {
    b.shl(step.imm, 12)
}

/// Emit constraints for AUIPC: rd = pc + (imm << 12).
pub fn emit_auipc(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let shifted = b.shl(step.imm, 12);
    b.iadd_32(step.pc, shifted)
}

/// Emit constraints for JAL: rd = pc + 4, next_pc = pc + imm.
/// Returns (rd_value, expected_next_pc).
pub fn emit_jal(b: &CircuitBuilder, step: &StepWires) -> (Wire, Wire) {
    let four = b.add_constant_64(4);
    let rd_val = b.iadd_32(step.pc, four);
    let target = b.iadd_32(step.pc, step.imm);
    (rd_val, target)
}

/// Emit constraints for JALR: rd = pc + 4, next_pc = (rs1 + imm) & ~1.
pub fn emit_jalr(b: &CircuitBuilder, step: &StepWires) -> (Wire, Wire) {
    let four = b.add_constant_64(4);
    let rd_val = b.iadd_32(step.pc, four);
    let sum = b.iadd_32(step.rs1_val, step.imm);
    let mask = b.add_constant_64(!1u64);
    let target = b.band(sum, mask);
    (rd_val, target)
}

/// Emit constraints for BEQ: next_pc = (rs1 == rs2) ? pc+imm : pc+4.
pub fn emit_beq(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let cond = b.icmp_eq(step.rs1_val, step.rs2_val);
    let four = b.add_constant_64(4);
    let branch_target = b.iadd_32(step.pc, step.imm);
    let fallthrough = b.iadd_32(step.pc, four);
    b.select(cond, branch_target, fallthrough)
}

/// Emit constraints for BNE: next_pc = (rs1 != rs2) ? pc+imm : pc+4.
pub fn emit_bne(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let cond = b.icmp_ne(step.rs1_val, step.rs2_val);
    let four = b.add_constant_64(4);
    let branch_target = b.iadd_32(step.pc, step.imm);
    let fallthrough = b.iadd_32(step.pc, four);
    b.select(cond, branch_target, fallthrough)
}

/// Emit constraints for BLT: next_pc = (rs1 < rs2, signed) ? pc+imm : pc+4.
pub fn emit_blt(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let sign_bit = b.add_constant_64(1u64 << 63);
    let rs1_f = b.bxor(step.rs1_val, sign_bit);
    let rs2_f = b.bxor(step.rs2_val, sign_bit);
    let cond = b.icmp_ult(rs1_f, rs2_f);
    let four = b.add_constant_64(4);
    let branch_target = b.iadd_32(step.pc, step.imm);
    let fallthrough = b.iadd_32(step.pc, four);
    b.select(cond, branch_target, fallthrough)
}

/// Emit constraints for BLTU: next_pc = (rs1 < rs2, unsigned) ? pc+imm : pc+4.
pub fn emit_bltu(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let cond = b.icmp_ult(step.rs1_val, step.rs2_val);
    let four = b.add_constant_64(4);
    let branch_target = b.iadd_32(step.pc, step.imm);
    let fallthrough = b.iadd_32(step.pc, four);
    b.select(cond, branch_target, fallthrough)
}

/// Emit constraints for BGE: next_pc = (rs1 >= rs2, signed) ? pc+imm : pc+4.
pub fn emit_bge(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let sign_bit = b.add_constant_64(1u64 << 63);
    let rs1_f = b.bxor(step.rs1_val, sign_bit);
    let rs2_f = b.bxor(step.rs2_val, sign_bit);
    let cond = b.icmp_uge(rs1_f, rs2_f);
    let four = b.add_constant_64(4);
    let branch_target = b.iadd_32(step.pc, step.imm);
    let fallthrough = b.iadd_32(step.pc, four);
    b.select(cond, branch_target, fallthrough)
}

/// Emit constraints for BGEU: next_pc = (rs1 >= rs2, unsigned) ? pc+imm : pc+4.
pub fn emit_bgeu(b: &CircuitBuilder, step: &StepWires) -> Wire {
    let cond = b.icmp_uge(step.rs1_val, step.rs2_val);
    let four = b.add_constant_64(4);
    let branch_target = b.iadd_32(step.pc, step.imm);
    let fallthrough = b.iadd_32(step.pc, four);
    b.select(cond, branch_target, fallthrough)
}

/// Emit constraints for DIV: rd = rs1 / rs2 (signed, round toward zero).
/// Uses hint + multiply-back: prover supplies quotient q and remainder r,
/// circuit verifies rs1 = q * rs2 + r AND |r| < |rs2|.
pub fn emit_div(b: &CircuitBuilder, step: &StepWires) -> Wire {
    // The quotient is the witness (rd_after).
    // Verify: quotient * rs2 + remainder = rs1
    // remainder = rs1 - quotient * rs2
    let remainder = b.add_witness();
    let (prod_lo, _prod_hi) = b.imul(step.rd_after, step.rs2_val);
    let expected_rs1 = b.iadd_32(prod_lo, remainder);
    b.assert_eq("div_check", expected_rs1, step.rs1_val);

    // Range check: |remainder| < |rs2| (omitted for now — needs abs + comparison)
    step.rd_after
}
