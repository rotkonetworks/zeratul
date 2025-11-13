# Contributing to Zeratul (Ligerito)

Thank you for your interest in contributing! This guide will help you get started.

## Table of Contents

- [Development Setup](#development-setup)
- [Code Organization](#code-organization)
- [Making Changes](#making-changes)
- [Testing](#testing)
- [Performance](#performance)
- [Pull Request Process](#pull-request-process)
- [Code Style](#code-style)

## Development Setup

### Prerequisites

```bash
# Install Rust (stable or nightly for SIMD features)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install development tools
cargo install cargo-watch cargo-edit cargo-tree

# Clone the repository
git clone https://github.com/yourusername/zeratul.git
cd zeratul
```

### Build Everything

```bash
# Build all workspace members
cargo build --workspace

# Build with all features
cargo build --all-features

# Build release
cargo build --release --workspace
```

### Run Tests

```bash
# Run all tests
cargo test --workspace

# Run specific package tests
cargo test --package ligerito

# Run with output
cargo test -- --nocapture
```

## Code Organization

See [PROJECT_STRUCTURE.md](PROJECT_STRUCTURE.md) for detailed organization.

### Key Principles

1. **Feature Gating**: Use conditional compilation for optional features
   ```rust
   #[cfg(feature = "prover")]
   pub fn prove(...) { }

   #[cfg(not(feature = "std"))]
   extern crate alloc;
   ```

2. **No Circular Dependencies**: Libraries should form a DAG
   ```
   binary-fields → reed-solomon → ligerito
   binary-fields → merkle-tree → ligerito
   ```

3. **Minimal Public API**: Only export what's necessary
   ```rust
   pub use verifier::{verify, verify_sha256};  // Public
   pub(crate) fn internal_helper() { }         // Internal
   ```

4. **Documentation**: Document all public items
   ```rust
   /// Verify a Ligerito proof
   ///
   /// # Arguments
   /// * `config` - Verifier configuration
   /// * `proof` - Proof to verify
   ///
   /// # Returns
   /// `Ok(true)` if valid, `Ok(false)` if invalid, `Err` on error
   pub fn verify<T, U>(...) -> Result<bool>
   ```

## Making Changes

### 1. Create a Branch

```bash
git checkout -b feature/your-feature-name
# or
git checkout -b fix/bug-description
```

### 2. Make Your Changes

Follow these guidelines:

#### Adding a New Feature

```bash
# 1. Add feature flag to Cargo.toml
[features]
my-feature = ["dependency-name"]

# 2. Implement with feature gates
#[cfg(feature = "my-feature")]
pub mod my_feature {
    // Implementation
}

# 3. Add tests
#[cfg(all(test, feature = "my-feature"))]
mod tests {
    // Tests
}

# 4. Update documentation
```

#### Fixing a Bug

```bash
# 1. Add a failing test first
#[test]
fn test_bug_reproduction() {
    // Reproduce the bug
}

# 2. Fix the bug

# 3. Verify test passes
cargo test test_bug_reproduction
```

#### Performance Optimization

```bash
# 1. Benchmark before
cargo bench --bench my_bench > before.txt

# 2. Make changes

# 3. Benchmark after
cargo bench --bench my_bench > after.txt

# 4. Compare
diff before.txt after.txt
```

### 3. Code Style

#### Formatting

```bash
# Format code
cargo fmt --all

# Check formatting
cargo fmt --all -- --check
```

#### Linting

```bash
# Run clippy
cargo clippy --workspace -- -D warnings

# Fix clippy warnings
cargo clippy --workspace --fix
```

#### Style Guidelines

- **Imports**: Group and order
  ```rust
  // Standard library
  use std::sync::Arc;

  // External crates
  use serde::{Deserialize, Serialize};

  // Internal crates
  use binary_fields::BinaryFieldElement;

  // Local modules
  use crate::utils::evaluate_lagrange_basis;
  ```

- **Naming**:
  - `snake_case` for functions and variables
  - `PascalCase` for types and traits
  - `SCREAMING_SNAKE_CASE` for constants
  - Descriptive names over abbreviations

- **Comments**:
  - `///` for public documentation
  - `//` for implementation comments
  - Explain *why*, not *what* (code shows what)

- **Error Handling**:
  ```rust
  // Prefer Result over panic
  pub fn do_thing() -> Result<Output> {
      let value = try_operation()?;
      Ok(value)
  }

  // Use custom error types
  #[derive(Debug, thiserror::Error)]
  pub enum MyError {
      #[error("Invalid input: {0}")]
      InvalidInput(String),
  }
  ```

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_functionality() {
        let result = my_function(input);
        assert_eq!(result, expected);
    }

    #[test]
    #[should_panic(expected = "error message")]
    fn test_error_handling() {
        my_function(invalid_input);
    }
}
```

### Integration Tests

```rust
// tests/integration_test.rs
use ligerito::{prove, verify, hardcoded_config_20};

#[test]
fn test_prove_verify_integration() {
    // Full prove/verify cycle
}
```

### Benchmarks

```rust
// benches/my_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn benchmark_function(c: &mut Criterion) {
    c.bench_function("my function", |b| {
        b.iter(|| my_function(input));
    });
}

criterion_group!(benches, benchmark_function);
criterion_main!(benches);
```

## Performance

### Profiling

```bash
# CPU profiling with flamegraph
cargo flamegraph --example profile_proving

# Memory profiling with heaptrack
heaptrack cargo run --example prove_verify --release
```

### Benchmarking

```bash
# Run benchmarks
cargo bench --package ligerito

# Compare with Julia
cd benchmarks
./compare_proper_tuned.sh
```

### Performance Guidelines

1. **Use SIMD where possible**: See `binary-fields/src/simd.rs`
2. **Minimize allocations**: Reuse buffers
3. **Parallel where beneficial**: Use rayon for data parallelism
4. **Profile before optimizing**: Measure, don't guess

## Pull Request Process

### 1. Before Submitting

```bash
# Ensure all tests pass
cargo test --workspace

# Ensure code is formatted
cargo fmt --all

# Ensure no clippy warnings
cargo clippy --workspace -- -D warnings

# Build documentation
cargo doc --workspace --no-deps

# Run benchmarks if performance-related
cargo bench --workspace
```

### 2. Commit Messages

Follow conventional commits:

```
feat: Add support for 2^32 polynomial size
fix: Correct sumcheck verification for edge case
docs: Update README with new examples
perf: Optimize Merkle tree construction
test: Add integration tests for verifier-only build
refactor: Simplify transcript interface
```

### 3. Pull Request Description

Include:

```markdown
## Description
Brief description of changes

## Motivation
Why this change is needed

## Changes
- List of specific changes
- Breaking changes (if any)

## Testing
How this was tested

## Performance Impact
Benchmark results (if applicable)

## Checklist
- [ ] Tests added/updated
- [ ] Documentation updated
- [ ] Benchmarks run (if performance-related)
- [ ] No clippy warnings
- [ ] Formatted with rustfmt
```

### 4. Review Process

- Address review comments
- Keep commits atomic and well-described
- Rebase on main if needed

```bash
git fetch origin
git rebase origin/main
git push --force-with-lease
```

## Specific Contribution Areas

### Adding a New Proof Size

1. Add configuration to `ligerito/src/configs.rs`:
   ```rust
   pub fn hardcoded_config_32<T, U>(...) -> ProverConfig<T, U>
   pub fn hardcoded_config_32_verifier() -> VerifierConfig
   ```

2. Export in `ligerito/src/lib.rs`:
   ```rust
   pub use configs::{hardcoded_config_32, hardcoded_config_32_verifier};
   ```

3. Add tests in `ligerito/tests/`

4. Add benchmark in `examples/bench_32.rs`

### Adding a New Transcript

1. Implement `Transcript` trait in `ligerito/src/transcript.rs`:
   ```rust
   pub struct MyTranscript { ... }
   impl Transcript for MyTranscript { ... }
   ```

2. Add to `FiatShamir` enum:
   ```rust
   pub enum FiatShamir {
       MyTranscript(MyTranscript),
       // ...
   }
   ```

3. Add feature flag and tests

### Optimizing Performance

1. Profile to identify bottleneck
2. Create benchmark for specific operation
3. Implement optimization
4. Verify with benchmarks and tests
5. Document performance gains in PR

## Getting Help

- **Documentation**: See docs in repository root
- **Examples**: Check `examples/` directory
- **Issues**: Open an issue on GitHub
- **Discussions**: Use GitHub Discussions for questions

## License

By contributing, you agree that your contributions will be licensed under the same license as the project.
