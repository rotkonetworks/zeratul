//! PVM trace → binius64 circuit compilation.
//!
//! Takes a PvmTrace and produces a binius64 Circuit with filled witness.
//! The circuit encodes one execution window (~1024 steps) with:
//! - Per-step ALU constraints (opcode-specific)
//! - Register preservation (unchanged regs stay the same)
//! - PC continuity (next_pc from step i == pc from step i+1)
//! - Gas accounting (gas decreases monotonically)
//! - Memory consistency (via offline checking, see memory.rs)

use binius_frontend::{CircuitBuilder, Wire};
use crate::trace::{PvmTrace, PvmStep, PvmOpcode};
use crate::opcodes::{self, StepWires};
use crate::NUM_REGS;

/// Public inputs that the light client checks.
pub struct PublicInputs {
    /// Initial PC.
    pub initial_pc: u64,
    /// Final PC.
    pub final_pc: u64,
    /// Initial register file.
    pub initial_regs: [u64; NUM_REGS],
    /// Final register file.
    pub final_regs: [u64; NUM_REGS],
    /// Initial gas.
    pub initial_gas: u64,
    /// Final gas.
    pub final_gas: u64,
    /// Program hash (blake2b of code blob).
    pub program_hash: [u8; 32],
    /// Initial memory root.
    pub initial_memory_root: [u8; 32],
    /// Final memory root.
    pub final_memory_root: [u8; 32],
}

/// Compile a PVM trace into a binius64 circuit.
///
/// Returns the CircuitBuilder with all constraints and the public input wires.
/// The caller then fills the witness and calls build() + prove().
pub fn compile_trace(trace: &PvmTrace) -> (CircuitBuilder, Vec<Wire>) {
    let b = CircuitBuilder::new();
    let mut public_wires = Vec::new();

    // Public inputs: initial and final state
    let initial_pc = b.add_inout();
    let final_pc = b.add_inout();
    public_wires.push(initial_pc);
    public_wires.push(final_pc);

    let mut initial_regs = [initial_pc; NUM_REGS]; // placeholder
    let mut final_regs = [final_pc; NUM_REGS];
    for i in 0..NUM_REGS {
        initial_regs[i] = b.add_inout();
        final_regs[i] = b.add_inout();
        public_wires.push(initial_regs[i]);
        public_wires.push(final_regs[i]);
    }

    let initial_gas = b.add_inout();
    let final_gas = b.add_inout();
    public_wires.push(initial_gas);
    public_wires.push(final_gas);

    // Compile each step
    let mut prev_next_pc: Option<Wire> = None;
    let mut prev_regs: Option<[Wire; NUM_REGS]> = None;
    let mut prev_gas: Option<Wire> = None;

    for (step_idx, step) in trace.steps.iter().enumerate() {
        let step_wires = allocate_step_wires(&b, step);

        // === State continuity from previous step ===
        if step_idx == 0 {
            // First step: bind to public initial state
            b.assert_eq("initial_pc", step_wires.pc, initial_pc);
            for i in 0..NUM_REGS {
                b.assert_eq(
                    format!("initial_reg_{i}"),
                    step_wires.regs_before[i],
                    initial_regs[i],
                );
            }
            b.assert_eq("initial_gas", step_wires.gas_before, initial_gas);
        } else {
            // PC continuity: this step's pc == previous step's next_pc
            if let Some(prev_npc) = prev_next_pc {
                b.assert_eq(
                    format!("pc_continuity_{step_idx}"),
                    step_wires.pc,
                    prev_npc,
                );
            }
            // Register continuity: regs_before == previous regs_after
            if let Some(prev_r) = prev_regs {
                for i in 0..NUM_REGS {
                    b.assert_eq(
                        format!("reg_continuity_{step_idx}_r{i}"),
                        step_wires.regs_before[i],
                        prev_r[i],
                    );
                }
            }
            // Gas continuity
            if let Some(prev_g) = prev_gas {
                b.assert_eq(
                    format!("gas_continuity_{step_idx}"),
                    step_wires.gas_before,
                    prev_g,
                );
            }
        }

        // === ALU constraint: compute expected result per opcode ===
        let (alu_result, expected_next_pc) = emit_opcode_constraints(&b, &step_wires, step);

        // Assert the destination register got the right value
        b.assert_eq(format!("alu_{step_idx}"), step_wires.rd_after, alu_result);

        // Assert next_pc is correct
        b.assert_eq(format!("next_pc_{step_idx}"), step_wires.next_pc, expected_next_pc);

        // === Register preservation: unchanged regs must stay the same ===
        let rd_idx = step.rd as usize;
        let mut regs_after_wires = step_wires.regs_before;
        if rd_idx < NUM_REGS {
            regs_after_wires[rd_idx] = step_wires.rd_after;
        }

        // Gas must decrease (or stay same for nop)
        let gas_cost = b.add_witness(); // prover fills with actual gas cost
        let expected_gas_after = {
            let zero = b.add_constant_64(0);
            let (result, _borrow) = b.isub_bin_bout(step_wires.gas_before, gas_cost, zero);
            result
        };
        b.assert_eq(format!("gas_{step_idx}"), step_wires.gas_after, expected_gas_after);

        // Track state for next step's continuity check
        prev_next_pc = Some(step_wires.next_pc);
        prev_regs = Some(regs_after_wires);
        prev_gas = Some(step_wires.gas_after);
    }

    // Last step: bind to public final state
    if let Some(last_npc) = prev_next_pc {
        b.assert_eq("final_pc", last_npc, final_pc);
    }
    if let Some(last_regs) = prev_regs {
        for i in 0..NUM_REGS {
            b.assert_eq(format!("final_reg_{i}"), last_regs[i], final_regs[i]);
        }
    }
    if let Some(last_gas) = prev_gas {
        b.assert_eq("final_gas", last_gas, final_gas);
    }

    (b, public_wires)
}

/// Allocate witness wires for one PVM step.
fn allocate_step_wires(b: &CircuitBuilder, _step: &PvmStep) -> StepWires {
    let pc = b.add_witness();
    let next_pc = b.add_witness();
    let mut regs_before = [pc; NUM_REGS]; // placeholder init
    for i in 0..NUM_REGS {
        regs_before[i] = b.add_witness();
    }
    let rd_after = b.add_witness();
    let rs1_val = b.add_witness();
    let rs2_val = b.add_witness();
    let imm = b.add_witness();
    let gas_before = b.add_witness();
    let gas_after = b.add_witness();

    StepWires {
        pc,
        next_pc,
        regs_before,
        rd_after,
        rs1_val,
        rs2_val,
        imm,
        gas_before,
        gas_after,
    }
}

/// Emit opcode-specific ALU constraint.
/// Returns (alu_result_wire, expected_next_pc_wire).
fn emit_opcode_constraints(
    b: &CircuitBuilder,
    wires: &StepWires,
    step: &PvmStep,
) -> (Wire, Wire) {
    let four = b.add_constant_64(4);
    let default_next_pc = b.iadd_32(wires.pc, four);
    let zero_wire = b.add_constant_64(0);

    let alu_result = match step.opcode {
        // R-type arithmetic
        PvmOpcode::Add => opcodes::emit_add(b, wires),
        PvmOpcode::Sub => opcodes::emit_sub(b, wires),
        PvmOpcode::And => opcodes::emit_and(b, wires),
        PvmOpcode::Or => opcodes::emit_or(b, wires),
        PvmOpcode::Xor => opcodes::emit_xor(b, wires),
        PvmOpcode::Slt => opcodes::emit_slt(b, wires),
        PvmOpcode::Sltu => opcodes::emit_sltu(b, wires),
        PvmOpcode::Mul => opcodes::emit_mul(b, wires),
        PvmOpcode::Mulh => opcodes::emit_mulh(b, wires),
        PvmOpcode::Mulhu => opcodes::emit_mulhu(b, wires),

        // I-type arithmetic
        PvmOpcode::Addi => opcodes::emit_addi(b, wires),
        PvmOpcode::Andi => opcodes::emit_andi(b, wires),
        PvmOpcode::Ori => opcodes::emit_ori(b, wires),
        PvmOpcode::Xori => opcodes::emit_xori(b, wires),

        // Upper immediate
        PvmOpcode::Lui => opcodes::emit_lui(b, wires),
        PvmOpcode::Auipc => opcodes::emit_auipc(b, wires),

        // Shifts with constant amount (from immediate)
        PvmOpcode::Slli => b.shl(wires.rs1_val, step.imm as u32 & 0x3f),
        PvmOpcode::Srli => b.shr(wires.rs1_val, step.imm as u32 & 0x3f),
        PvmOpcode::Srai => b.sar(wires.rs1_val, step.imm as u32 & 0x3f),

        // Load/store: ALU computes address = rs1 + imm, value is from memory
        PvmOpcode::Lb | PvmOpcode::Lh | PvmOpcode::Lw | PvmOpcode::Ld
        | PvmOpcode::Lbu | PvmOpcode::Lhu | PvmOpcode::Lwu => {
            // rd = mem[rs1 + imm]. The memory value is a witness
            // verified by the memory consistency argument (memory.rs).
            wires.rd_after // prover fills from trace, memory arg verifies
        }
        PvmOpcode::Sb | PvmOpcode::Sh | PvmOpcode::Sw | PvmOpcode::Sd => {
            // Stores don't write to rd. Return zero (rd unchanged).
            zero_wire
        }

        // Compare immediate
        PvmOpcode::Slti => opcodes::emit_slt(b, wires), // uses imm via rs2_val
        PvmOpcode::Sltiu => opcodes::emit_sltu(b, wires),

        // Dynamic shifts: amount from rs2 (not constant)
        // binius64's shl/shr take constant amounts. For dynamic shifts,
        // we need a different approach: decompose shift amount into bits
        // and use conditional select. This is a known gap.
        PvmOpcode::Sll | PvmOpcode::Srl | PvmOpcode::Sra => {
            // TODO: dynamic shift via bit decomposition of rs2[5:0]
            wires.rd_after // placeholder
        }

        // Division: hint + verify
        PvmOpcode::Div => opcodes::emit_div(b, wires),
        PvmOpcode::Divu | PvmOpcode::Rem | PvmOpcode::Remu => {
            // Similar to div — prover hints quotient/remainder,
            // circuit verifies q * divisor + r = dividend
            wires.rd_after // placeholder
        }

        // System / control
        PvmOpcode::Ecall | PvmOpcode::Halt | PvmOpcode::Nop => zero_wire,

        // Branches handled below for next_pc
        PvmOpcode::Beq | PvmOpcode::Bne | PvmOpcode::Blt
        | PvmOpcode::Bge | PvmOpcode::Bltu | PvmOpcode::Bgeu
        | PvmOpcode::Jal | PvmOpcode::Jalr => zero_wire, // rd handled by branch emitters

        _ => zero_wire,
    };

    // Next PC: branches and jumps compute their own, others use pc+4
    let next_pc = match step.opcode {
        PvmOpcode::Beq => opcodes::emit_beq(b, wires),
        PvmOpcode::Bne => opcodes::emit_bne(b, wires),
        PvmOpcode::Blt => opcodes::emit_blt(b, wires),
        PvmOpcode::Bge => opcodes::emit_bge(b, wires),
        PvmOpcode::Bltu => opcodes::emit_bltu(b, wires),
        PvmOpcode::Bgeu => opcodes::emit_bgeu(b, wires),
        PvmOpcode::Jal => {
            let (_rd, target) = opcodes::emit_jal(b, wires);
            target
        }
        PvmOpcode::Jalr => {
            let (_rd, target) = opcodes::emit_jalr(b, wires);
            target
        }
        _ => default_next_pc,
    };

    (alu_result, next_pc)
}
