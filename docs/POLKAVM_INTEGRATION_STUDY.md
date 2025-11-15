# PolkaVM Integration Study

This document provides a comprehensive analysis of the PolkaVM instruction set architecture (ISA) and execution model, and how to integrate it with Ligerito for zero-knowledge proof generation.

## 1. PolkaVM Architecture Overview

PolkaVM is a RISC-V based virtual machine designed for blockchain execution. It features:

- **Fully deterministic execution** - crucial for ZK proofs
- **Simple ISA** - based on RV32EM subset
- **Fast single-pass compilation**
- **Low memory footprint**
- **32-bit and 64-bit variants** (ISA32_V1, ISA64_V1)

### Key Design Properties
- No floating point (critical for ZK compatibility)
- No SIMD instructions
- Only 13 registers (vs 32 in full RISC-V)
- Byte-aligned variable-length instruction encoding
- Built-in gas metering support

## 2. Register Layout

PolkaVM uses exactly **13 registers**, all 32-bit (or 64-bit in ISA64 mode):

```rust
pub enum Reg {
    RA = 0,   // Return address
    SP = 1,   // Stack pointer
    T0 = 2,   // Temporary 0
    T1 = 3,   // Temporary 1
    T2 = 4,   // Temporary 2
    S0 = 5,   // Saved register 0
    S1 = 6,   // Saved register 1
    A0 = 7,   // Argument/return 0
    A1 = 8,   // Argument/return 1
    A2 = 9,   // Argument/return 2
    A3 = 10,  // Argument/return 3
    A4 = 11,  // Argument/return 4
    A5 = 12,  // Argument/return 5
}
```

**ABI Details:**
- Input/Output registers: A0-A5, T0-T2 (9 registers)
- Maximum input registers: 9
- Maximum output registers: 2
- RA is used for return address (jump to RA with offset 0 = ret)
- SP points to the stack

## 3. Memory Model

PolkaVM uses a sophisticated segmented memory model:

### Memory Segments

1. **Read-Only Data (ro_data)**
   - Base address: `0x10000` (VM_ADDRESS_SPACE_BOTTOM)
   - Contains program constants and immutable data
   - Page-aligned size

2. **Read-Write Data (rw_data)**
   - Starts after ro_data (with page gap)
   - Example: `0x30000`
   - Contains initialized mutable data
   - Can grow via heap allocation

3. **Heap**
   - Starts immediately after rw_data's initial size
   - Grows upward via `sbrk` instruction
   - Dynamic allocation region

4. **Stack**
   - Grows downward from high address
   - Example range: `0xfffdc000` to `0xfffe0000`
   - SP register points to current stack top

5. **Auxiliary Data (aux_data)**
   - Optional segment at highest addresses
   - Used for external data passing

### Memory Layout Diagram

```
0x00000000 ┌─────────────────┐
           │   (unmapped)    │
0x00010000 ├─────────────────┤ <- ro_data_address
           │   Read-Only     │
           │     Data        │
           ├─────────────────┤
           │   (page gap)    │
0x00030000 ├─────────────────┤ <- rw_data_address / heap_base
           │   Read-Write    │
           │     Data        │
           ├─────────────────┤
           │      Heap       │
           │       ↓         │
           │    (grows down) │
           ├─────────────────┤
           │   (unmapped)    │
0xfffdc000 ├─────────────────┤ <- stack_address_low
           │      Stack      │
           │       ↑         │
           │   (grows up)    │
0xfffe0000 ├─────────────────┤ <- stack_address_high
           │   (page gap)    │
           ├─────────────────┤
           │   Aux Data      │
0xffffffff └─────────────────┘
```

### Special Addresses

- **VM_ADDR_RETURN_TO_HOST**: `0xffff0000` - virtual address that returns control to host when jumped to
- **Page sizes**: 4KB to 64KB (configurable, must be power of 2)
- **Maximum code size**: 32 MB
- **Maximum jump table entries**: 16M

## 4. Complete Instruction Set

PolkaVM has **~100 instructions** organized by operand types. Instructions are variable-length encoded using varints.

### 4.1 Control Flow Instructions

#### No Arguments
```
trap            = 0   // Halt with trap
fallthrough     = 17  // Continue to next basic block
```

#### Jump/Branch with Offset
```
jump            = 5   // Unconditional jump to offset
jump_indirect   = 19  // Jump to [reg + imm]
load_imm_and_jump = 6 // reg = imm, jump to offset
load_imm_and_jump_indirect = 42 // dst = imm, jump to [base + offset]
```

#### Conditional Branches (reg, imm, offset)
```
branch_eq_imm                       = 7
branch_not_eq_imm                   = 15
branch_less_unsigned_imm            = 44
branch_less_signed_imm              = 32
branch_greater_or_equal_unsigned_imm = 52
branch_greater_or_equal_signed_imm  = 45
branch_less_or_equal_signed_imm     = 46
branch_less_or_equal_unsigned_imm   = 59
branch_greater_signed_imm           = 53
branch_greater_unsigned_imm         = 50
```

#### Conditional Branches (reg, reg, offset)
```
branch_eq                           = 24
branch_not_eq                       = 30
branch_less_unsigned                = 47
branch_less_signed                  = 48
branch_greater_or_equal_unsigned    = 41
branch_greater_or_equal_signed      = 43
```

### 4.2 Data Movement

#### Load Immediate
```
load_imm        = 4   // reg = imm32
load_imm64      = 118 // reg = imm64 (64-bit mode only)
move_reg        = 82  // dst = src
```

### 4.3 Memory Access

#### Load from Absolute Address (reg, imm)
```
load_u8         = 60  // Zero-extend
load_i8         = 74  // Sign-extend
load_u16        = 76
load_i16        = 66
load_i32        = 10
load_u32        = 102 // 64-bit mode only
load_u64        = 95  // 64-bit mode only
```

#### Load from Indirect Address (reg, reg, imm)
```
load_indirect_u8    = 11  // Load from [reg + imm]
load_indirect_i8    = 21
load_indirect_u16   = 37
load_indirect_i16   = 33
load_indirect_i32   = 1
load_indirect_u32   = 99  // 64-bit mode
load_indirect_u64   = 91  // 64-bit mode
```

#### Store to Absolute Address (reg, imm)
```
store_u8        = 71  // Store to [imm]
store_u16       = 69
store_u32       = 22
store_u64       = 96  // 64-bit mode
```

#### Store to Indirect Address (reg, reg, imm)
```
store_indirect_u8   = 16  // Store to [base + imm]
store_indirect_u16  = 29
store_indirect_u32  = 3
store_indirect_u64  = 90  // 64-bit mode
```

#### Store Immediate (imm, imm)
```
store_imm_u8    = 62  // Store immediate to absolute address
store_imm_u16   = 79
store_imm_u32   = 38
store_imm_u64   = 98  // 64-bit mode
```

#### Store Immediate Indirect (reg, imm, imm)
```
store_imm_indirect_u8   = 26  // Store imm2 to [reg + imm1]
store_imm_indirect_u16  = 54
store_imm_indirect_u32  = 13
store_imm_indirect_u64  = 93  // 64-bit mode
```

### 4.4 Arithmetic Operations

#### Three-Register ALU (reg, reg, reg)
```
add_32          = 8
add_64          = 101 // 64-bit mode
sub_32          = 20
sub_64          = 112 // 64-bit mode
mul_32          = 34
mul_64          = 113 // 64-bit mode
div_unsigned_32 = 68
div_unsigned_64 = 114 // 64-bit mode
div_signed_32   = 64
div_signed_64   = 115 // 64-bit mode
rem_unsigned_32 = 73
rem_unsigned_64 = 116 // 64-bit mode
rem_signed_32   = 70
rem_signed_64   = 117 // 64-bit mode
```

#### ALU with Immediate (reg, reg, imm)
```
add_imm_32      = 2
add_imm_64      = 104 // 64-bit mode
mul_imm_32      = 35
mul_imm_64      = 121 // 64-bit mode
negate_and_add_imm_32 = 40  // dst = imm - src
negate_and_add_imm_64 = 136 // 64-bit mode
```

#### Upper Multiply (high bits of product)
```
mul_upper_signed_signed         = 67
mul_upper_unsigned_unsigned     = 57
mul_upper_signed_unsigned       = 81
```

### 4.5 Bitwise Operations

#### Three-Register (reg, reg, reg)
```
and             = 23
or              = 12
xor             = 28
shift_logical_left_32   = 55
shift_logical_left_64   = 100 // 64-bit mode
shift_logical_right_32  = 51
shift_logical_right_64  = 108 // 64-bit mode
shift_arithmetic_right_32 = 77
shift_arithmetic_right_64 = 109 // 64-bit mode
```

#### With Immediate (reg, reg, imm)
```
and_imm         = 18
or_imm          = 49
xor_imm         = 31
shift_logical_left_imm_32   = 9
shift_logical_left_imm_64   = 105 // 64-bit mode
shift_logical_right_imm_32  = 14
shift_logical_right_imm_64  = 106 // 64-bit mode
shift_arithmetic_right_imm_32 = 25
shift_arithmetic_right_imm_64 = 107 // 64-bit mode
```

#### Alternative Shift Forms (operand order swapped)
```
shift_logical_left_imm_alt_32       = 75
shift_logical_left_imm_alt_64       = 110
shift_logical_right_imm_alt_32      = 72
shift_logical_right_imm_alt_64      = 103
shift_arithmetic_right_imm_alt_32   = 80
shift_arithmetic_right_imm_alt_64   = 111
```

### 4.6 Comparison Operations

#### Three-Register (reg, reg, reg)
```
set_less_than_unsigned  = 36  // dst = (s1 < s2) ? 1 : 0
set_less_than_signed    = 58
```

#### With Immediate (reg, reg, imm)
```
set_less_than_unsigned_imm      = 27
set_less_than_signed_imm        = 56
set_greater_than_unsigned_imm   = 39
set_greater_than_signed_imm     = 61
```

### 4.7 Conditional Move

```
cmov_if_zero        = 83  // dst = cond ? src : dst (if cond == 0)
cmov_if_not_zero    = 84  // dst = cond ? src : dst (if cond != 0)
cmov_if_zero_imm    = 85  // dst = cond ? imm : dst (if cond == 0)
cmov_if_not_zero_imm = 86 // dst = cond ? imm : dst (if cond != 0)
```

### 4.8 System Operations

```
ecalli          = 78  // External call (host function call)
sbrk            = 87  // Heap allocation (dst = new_heap_top, src = size)
```

### Instruction Encoding

Instructions use variable-length encoding:
- Opcode byte (1 byte)
- Register arguments packed in nibbles (4 bits each)
- Immediates encoded as varints (1-5 bytes typically)
- Offsets stored as PC-relative deltas

Example encoding for `add_imm_32`:
```
Byte 0: Opcode (2)
Byte 1: dst_reg | (src_reg << 4)
Bytes 2+: Immediate (varint)
```

## 5. Execution Model

### Basic Execution Flow

1. **Program Counter (PC)** tracks current instruction offset
2. Instructions execute sequentially unless control flow changes
3. Each instruction updates registers and/or memory
4. Branches/jumps modify PC
5. `ecalli` interrupts execution for host calls
6. Execution halts on `trap`, return to host, or PC out of bounds

### Key Execution Properties

1. **Deterministic Division/Remainder**
   - Division by zero returns specific values (not trap!)
   - `divu(x, 0) = 0xFFFFFFFF`
   - `remu(x, 0) = x`
   - `div(x, 0) = -1`
   - `rem(x, 0) = x`
   - Overflow cases handled: `div(INT_MIN, -1) = INT_MIN`

2. **Memory Access**
   - Must be within allocated regions
   - Unaligned access allowed (no alignment traps)
   - Out-of-bounds access causes trap

3. **No Hidden State**
   - Only 13 registers + PC + memory
   - No flags register
   - No privileged state

## 6. Differences from Our Simple VM

### Current Simple VM (Ligerito)
```rust
enum Instruction {
    Add(u8, u8, u8),     // dest, src1, src2
    Sub(u8, u8, u8),
    Mul(u8, u8, u8),
    LoadImm(u8, u32),
    Load(u8, u32),
    Store(u32, u8),
    Halt,
}
```

### Key Differences

| Aspect | Simple VM | PolkaVM |
|--------|-----------|---------|
| Registers | Unlimited (u8 index) | 13 fixed registers |
| Instructions | ~7 basic ops | ~100 instructions |
| Memory Model | Flat array | Segmented (ro/rw/stack/heap) |
| Addressing | Absolute only | Absolute + indirect |
| Data Types | 32-bit only | 8/16/32/64-bit |
| Encoding | Fixed-size | Variable-length varint |
| Branches | None | 16+ conditional branches |
| Signed Ops | None | Separate signed/unsigned |
| Control Flow | Linear + Halt | Jumps, branches, calls |
| Division | Not defined | Defined behavior for div-by-zero |

## 7. Integration Strategy for Ligerito

### Phase 1: Extend ISA Support

1. **Add PolkaVM instruction enum**
   ```rust
   pub enum PolkaVMInstruction {
       // All ~100 PolkaVM instructions
       Add32(Reg, Reg, Reg),
       LoadImm(Reg, u32),
       BranchEq(Reg, u32, u32),  // reg, imm, offset
       // ...
   }
   ```

2. **Implement register mapping**
   ```rust
   pub struct PolkaVMRegisters {
       regs: [u32; 13],  // or [u64; 13] for 64-bit
   }

   impl PolkaVMRegisters {
       fn get(&self, reg: Reg) -> u32 {
           self.regs[reg as usize]
       }
       fn set(&mut self, reg: Reg, val: u32) {
           self.regs[reg as usize] = val;
       }
   }
   ```

3. **Implement segmented memory model**
   ```rust
   pub struct PolkaVMMemory {
       ro_data: Vec<u8>,
       rw_data: Vec<u8>,
       stack: Vec<u8>,
       heap_size: u32,
       memory_map: MemoryMap,
   }

   impl PolkaVMMemory {
       fn read_u32(&self, addr: u32) -> Result<u32, MemoryError> {
           // Route to appropriate segment
       }
       fn write_u32(&mut self, addr: u32, val: u32) -> Result<(), MemoryError> {
           // Route to appropriate segment, check permissions
       }
   }
   ```

### Phase 2: Execution Tracing

The key insight is that PolkaVM's interpreter already provides instruction-level tracing:

```rust
// From tests.rs - step tracing example
config.set_step_tracing(true);
let module = Module::from_blob(&engine, &config, blob).unwrap();
let mut instance = module.instantiate().unwrap();

loop {
    match instance.run().unwrap() {
        InterruptKind::Step => {
            // Capture state after each instruction
            let pc = instance.program_counter().unwrap();
            let regs = Reg::ALL.map(|r| instance.reg(r));
            // Record trace entry
        }
        InterruptKind::Finished => break,
        InterruptKind::Trap => return Err(...),
        _ => {}
    }
}
```

### Phase 3: Trace Extraction

Modify PolkaVM interpreter to generate Ligerito-compatible traces:

```rust
pub struct PolkaVMTraceEntry {
    pub pc: u32,
    pub instruction: PolkaVMInstruction,
    pub registers_before: [u32; 13],
    pub registers_after: [u32; 13],
    pub memory_reads: Vec<(u32, Vec<u8>)>,   // (addr, data)
    pub memory_writes: Vec<(u32, Vec<u8>)>,
}

pub fn extract_trace_from_polkavm(
    program: &ProgramBlob,
    input_regs: [u32; 13],
) -> Result<Vec<PolkaVMTraceEntry>, Error> {
    let mut config = ModuleConfig::new();
    config.set_step_tracing(true);

    let engine = Engine::new(&Config::default())?;
    let module = Module::from_blob(&engine, &config, program)?;
    let mut instance = module.instantiate()?;

    // Set input registers
    for (reg, val) in Reg::ALL.iter().zip(input_regs.iter()) {
        instance.set_reg(*reg, *val);
    }

    let mut trace = Vec::new();

    loop {
        let regs_before = Reg::ALL.map(|r| instance.reg(r));

        match instance.run()? {
            InterruptKind::Step => {
                let pc = instance.program_counter().unwrap();
                let regs_after = Reg::ALL.map(|r| instance.reg(r));

                // Decode instruction at PC
                let inst = decode_instruction(&module, pc)?;

                trace.push(PolkaVMTraceEntry {
                    pc: pc.0,
                    instruction: inst,
                    registers_before: regs_before,
                    registers_after: regs_after,
                    memory_reads: vec![],  // TODO: track via memory hooks
                    memory_writes: vec![],
                });
            }
            InterruptKind::Finished => break,
            other => return Err(Error::UnexpectedInterrupt(other)),
        }
    }

    Ok(trace)
}
```

### Phase 4: Constraint Generation

For each PolkaVM instruction, generate constraints:

```rust
impl ConstraintGenerator for PolkaVMInstruction {
    fn generate_constraints<F: PrimeField>(
        &self,
        cs: &mut ConstraintSystem<F>,
        regs_before: &[Variable],
        regs_after: &[Variable],
        memory: &MemoryConstraints<F>,
    ) -> Result<(), Error> {
        match self {
            Self::Add32(dst, s1, s2) => {
                let dst_idx = *dst as usize;
                let s1_idx = *s1 as usize;
                let s2_idx = *s2 as usize;

                // dst_after = s1_before + s2_before (mod 2^32)
                cs.enforce_add_32(
                    regs_after[dst_idx],
                    regs_before[s1_idx],
                    regs_before[s2_idx],
                );

                // All other registers unchanged
                for i in 0..13 {
                    if i != dst_idx {
                        cs.enforce_equal(regs_after[i], regs_before[i]);
                    }
                }
            }

            Self::LoadIndirectU32(dst, base, offset) => {
                let addr_var = cs.add_u32(
                    regs_before[*base as usize],
                    cs.constant(*offset),
                );

                // Read from memory
                let value = memory.read_u32(cs, addr_var)?;
                cs.enforce_equal(regs_after[*dst as usize], value);

                // Other registers unchanged
                // ...
            }

            Self::BranchEq(reg, imm, _offset) => {
                // PC change handled at higher level
                // Just verify condition
                let is_equal = cs.is_equal(
                    regs_before[*reg as usize],
                    cs.constant(*imm),
                );
                // PC_next = is_equal ? (PC + offset) : (PC + inst_len)
            }

            // ... ~100 more instruction implementations
        }
    }
}
```

### Phase 5: Handle Memory Segments

```rust
pub struct SegmentedMemoryConstraints<F: PrimeField> {
    ro_data: MerkleTreeMemory<F>,      // Read-only, can use simple array
    rw_data: MerkleTreeMemory<F>,      // Mutable
    stack: MerkleTreeMemory<F>,        // Mutable
    memory_map: MemoryMap,
}

impl<F: PrimeField> SegmentedMemoryConstraints<F> {
    fn read_u32(&self, cs: &mut ConstraintSystem<F>, addr: Variable) -> Result<Variable, Error> {
        // Route to appropriate segment based on address
        // Use range checks to determine segment

        let in_ro_data = cs.is_in_range(
            addr,
            self.memory_map.ro_data_address(),
            self.memory_map.ro_data_address() + self.memory_map.ro_data_size(),
        );

        let in_rw_data = cs.is_in_range(
            addr,
            self.memory_map.rw_data_address(),
            self.memory_map.rw_data_address() + self.memory_map.rw_data_size(),
        );

        // ... check other segments

        // Conditionally read from correct segment
        let ro_value = self.ro_data.read(cs, addr)?;
        let rw_value = self.rw_data.read(cs, addr)?;
        // ...

        cs.select(in_ro_data, ro_value,
            cs.select(in_rw_data, rw_value, /* ... */))
    }
}
```

## 8. Example PolkaVM Programs

### Example 1: Simple Addition

```assembly
// Add two numbers and return
@main:
    a0 = a0 + a1      // add_32 instruction
    ret               // jump_indirect to RA+0
```

Encoded as:
```rust
let program = [
    asm::add_32(A0, A0, A1),
    asm::ret(),
];
```

Expected trace:
1. `pc=0, add_32(a0, a0, a1)`: `a0 = input1 + input2`
2. `pc=1, jump_indirect(ra, 0)`: Jump to return address

### Example 2: Fibonacci

```assembly
@fib:
    // Input: a0 = n
    // Output: a0 = fib(n)

    // if n <= 1, return n
    a0 <u 2
    jump @base_case if a0 <u 2

    // else compute fib(n-1) + fib(n-2)
    s0 = a0          // save n
    a0 = a0 - 1
    ecalli 0         // recurse (simplified)
    s1 = a0          // save fib(n-1)
    a0 = s0 - 2
    ecalli 0         // recurse
    a0 = a0 + s1     // fib(n-2) + fib(n-1)
    ret

@base_case:
    // a0 already contains n (0 or 1)
    ret
```

### Example 3: Memory Operations

```assembly
@memory_test:
    // Store value to memory
    u32 [0x20000] = 0x12345678    // store_imm_u32

    // Load it back
    a0 = u32 [0x20000]            // load_u32

    // Use indirect access
    a1 = 0x20000
    u32 [a1 + 4] = 0xABCDEF00     // store_indirect_u32
    a2 = u32 [a1 + 4]             // load_indirect_u32

    ret
```

### Example 4: Conditional Logic

```assembly
@max:
    // Return max(a0, a1)
    jump @ret_a0 if a0 >u a1      // branch_greater_unsigned_imm
    a0 = a1
@ret_a0:
    ret
```

### Example 5: Loop

```assembly
@sum_array:
    // a0 = array_ptr
    // a1 = length
    // Returns sum in a0

    s0 = 0                        // sum = 0
    s1 = 0                        // i = 0

@loop:
    jump @done if s1 >= a1        // if i >= length, exit

    t0 = u32 [a0 + s1 * 4]        // load array[i]
    s0 = s0 + t0                  // sum += array[i]
    s1 = s1 + 1                   // i++
    jump @loop

@done:
    a0 = s0
    ret
```

## 9. Testing Strategy

### Unit Tests

Test individual instruction constraint generation:

```rust
#[test]
fn test_add32_constraints() {
    let cs = ConstraintSystem::new();
    let inst = PolkaVMInstruction::Add32(A0, A1, A2);

    let regs_before = allocate_registers(&mut cs);
    let regs_after = allocate_registers(&mut cs);

    cs.set(regs_before[7], 10);  // A0 = 10
    cs.set(regs_before[8], 20);  // A1 = 20
    cs.set(regs_before[9], 30);  // A2 = 30

    inst.generate_constraints(&mut cs, &regs_before, &regs_after, &memory)?;

    assert_eq!(cs.get(regs_after[7]), 50);  // A0 = 20 + 30
}
```

### Integration Tests

Test full program execution:

```rust
#[test]
fn test_fibonacci_proof() {
    let program = compile_asm("
        @fib:
            jump @ret if a0 <u 2
            // ... fibonacci logic
            ret
        @ret:
            ret
    ");

    let trace = extract_trace_from_polkavm(&program, [5, 0, 0, ...])?;
    let proof = generate_proof(&trace)?;
    assert!(verify_proof(&proof, &[5], &[8]));  // fib(5) = 8
}
```

### Comparison Tests

Cross-check against PolkaVM interpreter:

```rust
#[test]
fn test_trace_matches_polkavm() {
    let program = load_polkavm_program();

    // Run in PolkaVM
    let polkavm_result = run_polkavm(&program, inputs);

    // Run in Ligerito
    let trace = extract_trace(&program, inputs);
    let ligerito_result = execute_trace(&trace);

    assert_eq!(polkavm_result, ligerito_result);
}
```

## 10. Implementation Roadmap

### Phase 1: Foundation (2-3 weeks)
- [ ] Define PolkaVMInstruction enum with all ~100 instructions
- [ ] Implement PolkaVMRegisters (13 registers)
- [ ] Implement segmented memory model
- [ ] Write instruction decoder (blob → instructions)

### Phase 2: Trace Extraction (1-2 weeks)
- [ ] Integrate with PolkaVM interpreter
- [ ] Implement step-by-step execution tracing
- [ ] Capture register state changes
- [ ] Capture memory access patterns
- [ ] Handle control flow (branches, jumps)

### Phase 3: Constraint Generation (3-4 weeks)
- [ ] Implement constraints for arithmetic ops (~20 instructions)
- [ ] Implement constraints for memory ops (~30 instructions)
- [ ] Implement constraints for control flow (~20 instructions)
- [ ] Implement constraints for remaining ops (~30 instructions)
- [ ] Test each instruction type

### Phase 4: Memory Proving (2-3 weeks)
- [ ] Implement Merkle tree for each segment
- [ ] Generate constraints for memory routing
- [ ] Prove read-only data immutability
- [ ] Prove memory isolation between segments

### Phase 5: Integration Testing (1-2 weeks)
- [ ] Test with simple PolkaVM programs
- [ ] Test with real blockchain guest programs
- [ ] Benchmark proof generation time
- [ ] Optimize constraint system

### Phase 6: Production Readiness (2-3 weeks)
- [ ] Handle ecalli (external calls) with public inputs
- [ ] Support gas metering tracking
- [ ] Documentation
- [ ] Performance optimization

**Total Estimated Time: 11-17 weeks**

## 11. Key Challenges

### Challenge 1: Instruction Count
- **Problem**: 100+ instructions vs our current 7
- **Solution**: Auto-generate constraint code from instruction definitions
- **Mitigation**: Many instructions are variants (32/64-bit, signed/unsigned)

### Challenge 2: Memory Complexity
- **Problem**: Segmented memory with routing logic
- **Solution**: Use conditional memory access with range checks
- **Cost**: Higher constraint count for memory operations

### Challenge 3: Control Flow
- **Problem**: Dynamic jumps and branches
- **Solution**: Use control flow graph with basic blocks
- **Approach**: Prove each basic block + valid transitions

### Challenge 4: Variable-Length Encoding
- **Problem**: Instructions have variable byte lengths
- **Solution**: Use PolkaVM's decoder, don't prove encoding
- **Rationale**: We prove execution, not encoding correctness

### Challenge 5: 64-bit Mode
- **Problem**: Some instructions operate on 64-bit values
- **Solution**: Use field elements > 2^64, or decompose into limbs
- **Approach**: Field-native ops where possible, otherwise limb decomposition

## 12. Proof Size Estimates

For a simple program (100 instructions):

| Component | Constraints per Step | Steps | Total |
|-----------|---------------------|-------|-------|
| Register updates | ~50 | 100 | 5,000 |
| Memory ops | ~200 | 20 | 4,000 |
| Control flow | ~30 | 100 | 3,000 |
| PC tracking | ~20 | 100 | 2,000 |
| **Total** | | | **~14,000** |

With Plonk backend:
- Proof size: ~1-2 KB
- Proving time: ~100-500ms
- Verification time: ~10-50ms

## 13. Next Steps

1. **Review this document** with the team
2. **Prototype instruction constraints** for top 10 most common instructions
3. **Extract a sample trace** from real PolkaVM execution
4. **Generate a proof** for a trivial program (single add instruction)
5. **Validate approach** before full implementation

## Appendix A: Instruction Reference

See section 4 for complete instruction listing.

## Appendix B: Resources

- PolkaVM repository: https://github.com/koute/polkavm
- PolkaVM source code: `/home/alice/src/polkavm/`
- Key files:
  - `/home/alice/src/polkavm/crates/polkavm-common/src/program.rs` - Instruction definitions
  - `/home/alice/src/polkavm/crates/polkavm/src/interpreter.rs` - Execution semantics
  - `/home/alice/src/polkavm/crates/polkavm-common/src/abi.rs` - Memory model
  - `/home/alice/src/polkavm/crates/polkavm/src/tests.rs` - Example programs

## Appendix C: Comparison with RISC-V

PolkaVM is based on RV32EM but has key differences:

| Feature | RISC-V RV32EM | PolkaVM |
|---------|---------------|---------|
| Registers | 32 (x0-x31) | 13 (RA, SP, T0-T2, S0-S1, A0-A5) |
| Instruction encoding | Fixed 32-bit | Variable-length varint |
| Division by zero | Unspecified | Defined return values |
| Memory model | Linear | Segmented |
| Floating point | Optional (F/D) | Not supported |
| Multiplication | M extension | Built-in |

## Appendix D: Division Semantics

Critical for ZK proofs - division must be deterministic:

```rust
fn divu(lhs: u32, rhs: u32) -> u32 {
    if rhs == 0 {
        u32::MAX
    } else {
        lhs / rhs
    }
}

fn remu(lhs: u32, rhs: u32) -> u32 {
    if rhs == 0 {
        lhs
    } else {
        lhs % rhs
    }
}

fn div(lhs: i32, rhs: i32) -> i32 {
    if rhs == 0 {
        -1
    } else if lhs == i32::MIN && rhs == -1 {
        lhs  // Overflow case
    } else {
        lhs / rhs
    }
}

fn rem(lhs: i32, rhs: i32) -> i32 {
    if rhs == 0 {
        lhs
    } else if lhs == i32::MIN && rhs == -1 {
        0  // Overflow case
    } else {
        lhs % rhs
    }
}
```

These semantics MUST be encoded in constraints.
