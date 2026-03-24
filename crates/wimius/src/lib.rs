//! Wimius: PVM execution verification via Binius64
//!
//! Translates PVM (rv64em) execution traces into Binius64 circuits,
//! enabling light clients to verify work package execution without
//! re-running the PVM.
//!
//! # Architecture
//!
//! ```text
//! javm execution trace → wimius → binius64 circuit → proof → light client
//! ```
//!
//! Each PVM instruction maps to 1-3 binius64 AND/MUL constraints:
//! - ADD/SUB: 1 iadd gate (native 64-bit integer add)
//! - AND/OR/XOR: 1 band/bor/bxor gate (free in binary fields)
//! - MUL: 1 imul gate (64×64→128 in one constraint)
//! - SLT: 1 icmp_ult gate
//! - Shifts: 1 shl/shr/sar gate (shifted value indices)
//! - Branches: 1 comparison + 1 select
//!
//! The opcode selector (~30 comparisons per step) dominates the
//! constraint count, not the ALU operations themselves.
//!
//! # Windowed Proving
//!
//! For traces longer than WINDOW_SIZE steps, the trace is split into
//! windows. Each window is proven independently. State continuity
//! between windows is verified by the light client checking that
//! window[i].final_state == window[i+1].initial_state.
//!
//! # Memory Consistency
//!
//! Memory accesses are verified via offline memory checking: the
//! prover provides a sorted log of (address, value, timestamp) and
//! a grand product argument proves the log is a permutation of the
//! actual access sequence.

pub mod trace;
pub mod opcodes;
pub mod circuit;
pub mod memory;

/// Default window size for chunked proving (steps per window).
pub const WINDOW_SIZE: usize = 1024;

/// Number of PVM registers (ra, sp, t0-t2, s0-s1, a0-a5).
pub const NUM_REGS: usize = 13;
