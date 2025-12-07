# Building

## Standard Build

```bash
# Debug build (faster compilation, slower runtime)
cargo build

# Release build (slower compilation, faster runtime)
cargo build --release
```

## Build Features

Currently no optional features. All functionality is included by default.

## Cross-Compilation

### Linux → macOS

```bash
# Install target
rustup target add x86_64-apple-darwin

# Build (requires macOS SDK)
cargo build --release --target x86_64-apple-darwin
```

### Linux → Windows

```bash
# Install target and linker
rustup target add x86_64-pc-windows-gnu
sudo apt install mingw-w64

# Build
cargo build --release --target x86_64-pc-windows-gnu
```

## Binary Size

Release binary is approximately 18MB. To reduce:

```bash
# Strip debug symbols
strip target/release/crow-agent

# Or use cargo config
# .cargo/config.toml
[profile.release]
strip = true
lto = true
```

## Dependencies

Key dependencies:

| Crate | Purpose |
|-------|---------|
| `rig-core` | AI framework (local fork) |
| `agent-client-protocol` | ACP implementation |
| `tokio` | Async runtime |
| `tracing` | Logging/telemetry |
| `opentelemetry` | Observability |
| `serde` | Serialization |

## Workspace Structure

The project uses Cargo workspace:

```toml
# Cargo.toml
[workspace]
members = [".", "rig/rig-core"]

[dependencies]
rig-core = { path = "rig/rig-core" }
```

## Incremental Builds

Rust handles incremental builds automatically. Only changed files are recompiled.

To force full rebuild:

```bash
cargo clean
cargo build --release
```

## Build Troubleshooting

### OpenSSL Errors

```bash
# Ubuntu/Debian
sudo apt install libssl-dev pkg-config

# macOS
brew install openssl
export OPENSSL_DIR=$(brew --prefix openssl)
```

### Memory Issues

If builds run out of memory:

```bash
# Limit parallelism
cargo build -j 2
```

### Linker Errors

```bash
# Ubuntu/Debian
sudo apt install build-essential

# Ensure correct linker
export CC=gcc
export CXX=g++
```
