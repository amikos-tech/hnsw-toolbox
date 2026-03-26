# Technology Stack

**Analysis Date:** 2026-03-26

## Languages

**Primary:**
- Rust (edition 2021) - Core library: HNSW index extraction, import, header parsing, metadata handling, FFI layer
- Go 1.21 - Thin binding layer that loads the Rust cdylib at runtime via purego (no CGO)

**Secondary:**
- None

## Runtime

**Environment:**
- Rust compiles to a native shared library (cdylib): `.dylib` (macOS), `.so` (Linux), `.dll` (Windows)
- Go consumers load the shared library at runtime via `purego.Dlopen`
- Little-endian host required (enforced at runtime in `src/header.rs`)

**Package Manager:**
- Cargo (Rust) - Lockfile: `Cargo.lock` (present, committed)
- Go modules - Lockfile: `go.sum` (present, committed)

## Frameworks

**Core:**
- No web framework - this is a library/toolkit, not a service

**Testing:**
- Rust: built-in `#[cfg(test)]` modules with `cargo test`
- Rust: `proptest` 1.6 - Property-based testing for extraction correctness
- Rust: `tempfile` 3.15 - Temporary directory fixtures
- Go: standard `testing` package

**Build/Dev:**
- Cargo - Rust build system
- Make (`Makefile`) - Unified build/test/lint/fmt orchestration across Rust and Go
- `cargo clippy` - Rust linter (enforced with `-D warnings`)
- `golangci-lint` v2.5.0 - Go linter (run via `go run`)
- `cargo fmt` / `gofmt` / `goimports` - Code formatting

## Key Dependencies

**Critical (Rust):**
- `fast-hnsw` 1.0.0 - HNSW index construction and persistence (builds new indices from columnar data, saves/loads `.hnsw` files)
- `arrow-array` 54.2 - Apache Arrow array types for columnar record batches
- `arrow-ipc` 54.2 - Arrow IPC file reader/writer for `.arrow` format I/O
- `arrow-schema` 54.2 - Arrow schema definitions and data types
- `parquet` 54.2 (with `arrow` feature) - Apache Parquet reader/writer

**Serialization (Rust):**
- `serde` 1.0 (with `derive` feature) - Struct serialization/deserialization
- `serde_json` 1.0 - JSON encoding for FFI request/response payloads
- `serde-pickle` 1.2 - Decoding Chroma `index_metadata.pickle` files (used in `src/metadata.rs`)

**Error handling (Rust):**
- `thiserror` 2.0 - Derive macro for `ExtractError` enum in `src/error.rs`

**Critical (Go):**
- `github.com/ebitengine/purego` v0.8.4 - Pure-Go dynamic library loading (dlopen/dlsym) without CGO; used to bind all five FFI symbols from the Rust cdylib

## Crate Configuration

**Crate type:** `["rlib", "cdylib"]` (defined in `Cargo.toml`)
- `rlib` - Standard Rust library for direct Rust consumers
- `cdylib` - C-ABI shared library for FFI consumers (Go, C, Python, etc.)

## Build Artifacts

**Rust shared library (platform-dependent):**
- macOS: `target/{debug,release}/libhnsw_toolbox.dylib`
- Linux: `target/{debug,release}/libhnsw_toolbox.so`
- Windows: `target/{debug,release}/hnsw_toolbox.dll`

**Make targets:**
- `make build` / `make build-debug` - Debug build via `cargo build --locked`
- `make build-release` - Release build via `cargo build --locked --release`
- `make test` / `make test-all` - Run both `cargo test` and `go test ./...`
- `make lint` - Run `cargo clippy` and `golangci-lint`
- `make fmt` - Run `cargo fmt`, `gofmt`, and optionally `goimports`
- `make clean` - Run `cargo clean`

## Configuration

**Environment:**
- No `.env` files; no runtime environment variables required
- `CARGO_TARGET_DIR` - Optional Makefile variable to override Cargo output directory
- Go consumers must pass the absolute path to the Rust shared library at init time: `hnswtoolbox.Init("/path/to/libhnsw_toolbox.dylib")`

**Build:**
- `Cargo.toml` - Rust package manifest and dependency declarations
- `go.mod` - Go module definition (`github.com/amikos-tech/hnsw-toolbox`)
- `Makefile` - Cross-platform build orchestration with OS detection (darwin/linux/windows)

## Platform Requirements

**Development:**
- Rust stable toolchain (edition 2021)
- Go >= 1.21
- Make
- Little-endian host (x86_64, aarch64)

**Production:**
- The Rust shared library must be built for the target platform before Go consumers can use it
- No external services, databases, or network connections required
- Filesystem access only (reads HNSW index files, writes Parquet/Arrow/HNSW output)

## CI/CD

**GitHub Actions (`.github/workflows/ci.yml`):**
- Runs on `ubuntu-latest`
- Three jobs: `lint`, `test`, `build`
- Uses `dtolnay/rust-toolchain@stable` for Rust
- Uses `actions/setup-go@v5` with Go 1.21
- Uses `Swatinem/rust-cache@v2` for Cargo caching
- Lint: `golangci-lint-action@v8` + `cargo clippy`
- Test: `make test`
- Build: `make build` + `make build-release`

**Additional workflows:**
- `.github/workflows/claude.yml` - Claude Code integration (issue/PR comment automation)
- `.github/workflows/claude-code-review.yml` - Claude Code PR review (currently manual trigger only)

---

*Stack analysis: 2026-03-26*
