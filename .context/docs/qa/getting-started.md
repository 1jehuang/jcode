---
slug: getting-started
category: getting-started
generatedAt: 2026-05-06T19:06:36.302Z
---

# How do I set up and run this project?

## Getting Started

### Prerequisites

- Rust toolchain with Cargo, matching the workspace edition in `Cargo.toml`.
- Platform build tools required by common Rust crates.

### Installation

```bash
# Clone the repository
git clone <repository-url>
cd jcode-harness

# Fetch and build Rust dependencies through Cargo
cargo check -p jcode
```

### Running

```bash
# Run the primary CLI
cargo run -p jcode --bin jcode -- --help

# Run the harness CLI
cargo run -p jcode --bin jcode-harness -- --help
```

### Validation

```bash
cargo fmt --check
cargo test -p jcode-storage
cargo test -p jcode skill_router --lib
```
