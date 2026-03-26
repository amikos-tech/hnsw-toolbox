# Codebase Structure

**Analysis Date:** 2026-03-26

## Directory Layout

```
hnsw-toolbox/
├── .github/
│   └── workflows/
│       ├── ci.yml               # Lint + test + build CI pipeline
│       ├── claude-code-review.yml # Claude code review automation
│       └── claude.yml           # Claude automation workflow
├── .planning/
│   └── codebase/                # Architecture/planning docs (this directory)
├── src/                         # Rust source (core library)
│   ├── lib.rs                   # Crate root, re-exports public API
│   ├── error.rs                 # ExtractError enum (thiserror)
│   ├── header.rs                # HNSW binary header parser
│   ├── extractor.rs             # Extract persisted HNSW -> columnar output
│   ├── importer.rs              # Build new HNSW index from columnar input
│   ├── metadata.rs              # Chroma metadata loader (Python serialized format)
│   └── ffi.rs                   # C-ABI FFI entry points (JSON protocol)
├── target/                      # Rust build output (gitignored)
├── Cargo.toml                   # Rust package manifest
├── Cargo.lock                   # Rust dependency lockfile
├── go.mod                       # Go module definition
├── go.sum                       # Go dependency checksums
├── hnswtoolbox.go               # Go bindings (purego, no CGO)
├── hnswtoolbox_test.go          # Go tests
├── Makefile                     # Cross-language build/test/lint orchestration
├── README.md                    # Project documentation
├── LICENSE.md                   # License file
└── .gitignore                   # Ignores /target
```

## Directory Purposes

**`src/`:**
- Purpose: All Rust source code for the core library
- Contains: 7 Rust modules covering parsing, extraction, import, FFI
- Key files: `src/lib.rs` (crate root), `src/extractor.rs` (largest module ~1037 lines), `src/ffi.rs` (FFI boundary)

**`.github/workflows/`:**
- Purpose: GitHub Actions CI configuration
- Contains: 3 YAML workflow files
- Key files: `ci.yml` (lint/test/build pipeline on push to main and PRs)

**`target/`:**
- Purpose: Rust build artifacts including the shared library
- Contains: Debug/release builds, dependency compilations
- Generated: Yes
- Committed: No (gitignored)

**`.planning/codebase/`:**
- Purpose: Architecture and planning documentation
- Generated: By analysis tools
- Committed: Yes

## Key File Locations

**Entry Points:**
- `src/lib.rs`: Rust crate root; declares modules and re-exports public types
- `src/ffi.rs`: FFI entry points (`#[no_mangle] pub unsafe extern "C"` functions)
- `hnswtoolbox.go`: Go package entry; `Init()` loads library, `ExtractIndex()`/`BuildIndex()` perform operations

**Configuration:**
- `Cargo.toml`: Rust edition (2021), crate-type `["rlib", "cdylib"]`, all Rust dependencies
- `go.mod`: Go module `github.com/amikos-tech/hnsw-toolbox`, Go 1.21, purego dependency
- `Makefile`: Platform-aware build targets, lint, test, format commands

**Core Logic (Rust):**
- `src/header.rs`: Binary header parsing for hnswlib persistence format (32-bit and 64-bit)
- `src/extractor.rs`: Streaming extraction from `header.bin` + `data_level0.bin` to Parquet/Arrow IPC
- `src/importer.rs`: Reads Parquet/Arrow IPC, builds `fast-hnsw` `LabeledIndex` with configurable distance metric
- `src/metadata.rs`: Loads Chroma `index_metadata` serialized data (u64 or i64 keys)
- `src/error.rs`: `ExtractError` enum with 10 variants covering IO, parsing, corruption, validation errors

**Go Bindings:**
- `hnswtoolbox.go`: Complete Go API -- types, `Init`/`Close`, `ExtractIndex`, `BuildIndex`, `Version`, `LastError`
- `hnswtoolbox_test.go`: Go tests (currently JSON unmarshal contract tests for response types)

**Testing (Rust):**
- `src/extractor.rs` (inline `#[cfg(test)] mod tests`): Extraction tests, property-based tests with proptest, f32 bit-preservation tests
- `src/importer.rs` (inline `#[cfg(test)] mod tests`): Build-from-Parquet, build-from-Arrow-IPC, label fallback tests

**CI:**
- `.github/workflows/ci.yml`: 3 jobs (lint, test, build) on ubuntu-latest

## Naming Conventions

**Files:**
- Rust: `snake_case.rs` (e.g., `extractor.rs`, `data_level0.bin`)
- Go: `lowercasepkg.go` and `lowercasepkg_test.go` (single package, no subdirectories)
- Config: Standard names (`Cargo.toml`, `go.mod`, `Makefile`)

**Directories:**
- `src/` for Rust source (standard Cargo convention)
- `.github/workflows/` for CI (standard GitHub convention)
- No Go subdirectories -- Go source lives at the project root

## Where to Add New Code

**New Rust Module:**
- Create `src/module_name.rs`
- Add `pub mod module_name;` to `src/lib.rs`
- Add public re-exports to `src/lib.rs` if the types should be part of the crate's public API
- If exposed via FFI: add new `#[no_mangle] pub unsafe extern "C" fn` to `src/ffi.rs`

**New FFI Function:**
- Add the `extern "C"` function in `src/ffi.rs`
- Follow the existing pattern: JSON request -> deserialize -> call core function -> serialize response -> return CString
- Wrap in `std::panic::catch_unwind` and use `set_last_error_message` for error cases
- Add corresponding Go function in `hnswtoolbox.go`: register symbol in `Init()`, add Go wrapper function

**New Go Function (wrapping existing FFI):**
- Add the Go type definitions and wrapper function in `hnswtoolbox.go`
- Register the FFI symbol pointer in `Init()` following existing pattern
- Add tests in `hnswtoolbox_test.go`

**New Distance Metric (Rust importer):**
- Add variant to `DistanceMetric` enum in `src/importer.rs`
- Add match arm in `build_index_from_columnar` calling `build_with_metric` with the new `fast_hnsw::distance` type
- Add corresponding Go constant in `hnswtoolbox.go`

**New Output/Input Format:**
- Add variant to `OutputFormat` (in `src/extractor.rs`) or `InputFormat` (in `src/importer.rs`)
- Implement `ColumnarSink` arm or reader arm respectively
- Add corresponding Go constant

**Tests:**
- Rust: Add inline `#[test]` functions in the `#[cfg(test)] mod tests` block of the relevant module
- Go: Add test functions in `hnswtoolbox_test.go`

## Special Directories

**`target/`:**
- Purpose: Rust compilation output; contains the shared library artifact
- Generated: Yes (by `cargo build`)
- Committed: No (gitignored)
- Key artifacts: `target/debug/libhnsw_toolbox.dylib` (macOS), `target/debug/libhnsw_toolbox.so` (Linux), `target/debug/hnsw_toolbox.dll` (Windows)

**`.planning/`:**
- Purpose: Planning and analysis documents consumed by development tooling
- Generated: By analysis commands
- Committed: Yes

## Build Artifacts

**Debug build:** `make build` or `cargo build`
- Output: `target/debug/libhnsw_toolbox.{dylib,so,dll}`

**Release build:** `make build-release` or `cargo build --release`
- Output: `target/release/libhnsw_toolbox.{dylib,so,dll}`

**Platform detection:** The `Makefile` auto-detects OS via `uname -s` and selects the correct library extension. It also respects `CARGO_TARGET_DIR` for custom build directories.

---

*Structure analysis: 2026-03-26*
