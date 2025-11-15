//! Simple PolkaVM guest program for testing Ligerito integration

#![no_std]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        core::arch::asm!("unimp", options(noreturn));
    }
}

/// Add two numbers: a + b
#[polkavm_derive::polkavm_export]
extern "C" fn add(a: u32, b: u32) -> u32 {
    a + b
}

/// Multiply two numbers: a * b
#[polkavm_derive::polkavm_export]
extern "C" fn mul(a: u32, b: u32) -> u32 {
    a * b
}

/// Simple computation: (a + b) * c
#[polkavm_derive::polkavm_export]
extern "C" fn compute(a: u32, b: u32, c: u32) -> u32 {
    let sum = a + b;
    sum * c
}
