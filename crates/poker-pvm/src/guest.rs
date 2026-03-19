//! PolkaVM guest program for poker game engine.
//!
//! entry: receives game state + signed action via host calls
//! exit:  writes new game state + action result via host calls
//!
//! the host (WIM tracer) records all memory reads/writes.
//! the trace can be verified by anyone without re-executing.
//!
//! host call interface:
//!   ecalli 0: read_state   — host writes GameState to guest memory at a0
//!   ecalli 1: read_action  — host writes SignedAction to guest memory at a0
//!   ecalli 2: write_result — guest has ActionResult at a0, host reads it
//!   ecalli 3: write_state  — guest has GameState at a0, host reads it

#![cfg_attr(feature = "pvm", no_std)]
#![cfg_attr(feature = "pvm", no_main)]

#[cfg(feature = "pvm")]
mod pvm_guest {
    extern crate alloc;
    use core::panic::PanicInfo;

    use poker_pvm::{GameState, SignedAction, ActionResult, Action, Rules};

    // bump allocator
    #[global_allocator]
    static ALLOCATOR: BumpAlloc = BumpAlloc;

    struct BumpAlloc;
    const ARENA_SIZE: usize = 64 * 1024;
    static mut ARENA: [u8; ARENA_SIZE] = [0; ARENA_SIZE];
    static mut OFFSET: usize = 0;

    unsafe impl core::alloc::GlobalAlloc for BumpAlloc {
        unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
            let align = layout.align();
            let size = layout.size();
            let off = unsafe { &mut OFFSET };
            *off = (*off + align - 1) & !(align - 1);
            let ptr = unsafe { ARENA.as_mut_ptr().add(*off) };
            *off += size;
            if *off > ARENA_SIZE { core::ptr::null_mut() } else { ptr }
        }
        unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
    }

    // host call stubs (implemented by PVM host / WIM tracer)
    extern "C" {
        fn ecalli_0(state_ptr: *mut u8, state_len: u32) -> u32;  // read_state
        fn ecalli_1(action_ptr: *mut u8, action_len: u32) -> u32; // read_action
        fn ecalli_2(result_ptr: *const u8, result_len: u32) -> u32; // write_result
        fn ecalli_3(state_ptr: *const u8, state_len: u32) -> u32;  // write_state
    }

    /// fixed-size serialization for PVM (no serde, no heap)
    /// GameState as raw bytes (fixed layout)
    const STATE_SIZE: usize = core::mem::size_of::<GameState>();
    const ACTION_SIZE: usize = core::mem::size_of::<SignedAction>();
    const RESULT_SIZE: usize = core::mem::size_of::<ActionResult>();

    #[no_mangle]
    pub extern "C" fn main() -> i32 {
        // allocate buffers on stack
        let mut state_buf = [0u8; STATE_SIZE];
        let mut action_buf = [0u8; ACTION_SIZE];

        // read game state from host
        unsafe { ecalli_0(state_buf.as_mut_ptr(), STATE_SIZE as u32) };
        let state: &mut GameState = unsafe { &mut *(state_buf.as_mut_ptr() as *mut GameState) };

        // read signed action from host
        unsafe { ecalli_1(action_buf.as_mut_ptr(), ACTION_SIZE as u32) };
        let action: &SignedAction = unsafe { &*(action_buf.as_ptr() as *const SignedAction) };

        // apply action — the core deterministic computation
        let result = match state.apply(action) {
            Ok(r) => r,
            Err(_) => ActionResult {
                valid: false,
                hand_over: false,
                winner: 255,
                payout: 0,
                advance_phase: false,
            },
        };

        // if showdown, evaluate winner
        if state.phase == poker_pvm::Phase::Showdown {
            state.showdown();
        }

        // write result to host
        let result_bytes = unsafe {
            core::slice::from_raw_parts(
                &result as *const ActionResult as *const u8,
                RESULT_SIZE,
            )
        };
        unsafe { ecalli_2(result_bytes.as_ptr(), RESULT_SIZE as u32) };

        // write new state to host
        let state_bytes = unsafe {
            core::slice::from_raw_parts(
                state as *const GameState as *const u8,
                STATE_SIZE,
            )
        };
        unsafe { ecalli_3(state_bytes.as_ptr(), STATE_SIZE as u32) };

        0 // success
    }

    #[panic_handler]
    fn panic(_: &PanicInfo) -> ! { loop {} }
}

// native test entry point
#[cfg(not(feature = "pvm"))]
fn main() {
    use poker_pvm::*;

    println!("poker-pvm guest (native mode)\n");

    let mut state = GameState::new(Rules::default());
    state.deal([12, 25], [0, 13], [4, 5, 6, 7, 8]);
    println!("dealt: phase={:?} pot={} stacks={:?}", state.phase, state.pot, state.stacks);

    // play a full hand
    let actions = [
        (0, Action::Call, 0),   // SB calls
        (1, Action::Check, 0),  // BB checks → flop
        (1, Action::Check, 0),  // check
        (0, Action::Check, 0),  // check → turn
        (1, Action::Check, 0),  // check
        (0, Action::Check, 0),  // check → river
        (1, Action::Check, 0),  // check
        (0, Action::Check, 0),  // check → showdown
    ];

    for (i, (seat, action, amount)) in actions.iter().enumerate() {
        let signed = SignedAction {
            seat: *seat, action: *action, amount: *amount, seq: (i + 1) as u32, sig: [0; 64],
        };
        match state.apply(&signed) {
            Ok(r) => {
                println!("  seat {} {:?}: phase={:?} pot={} advance={}", seat, action, state.phase, state.pot, r.advance_phase);
                if r.hand_over {
                    println!("  hand over: winner={}", r.winner);
                }
            }
            Err(e) => println!("  ERROR: {}", e),
        }
    }

    if state.phase == Phase::Showdown {
        let winner = state.showdown();
        println!("\nshowdown: winner=seat {} stacks={:?}", winner, state.stacks);
    }
}
