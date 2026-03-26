# Architecture

**Analysis Date:** 2026-03-26

## Pattern Overview

**Overall:** Dual-language library with Rust core + Go FFI bindings

**Key Characteristics:**
- Rust implements all core logic (binary parsing, extraction, index building, serialization)
- Rust exposes a C-ABI FFI layer (`cdylib`) with JSON-based request/response protocol
- Go bindings load the Rust shared library at runtime via `purego` (no CGO required)
- Communication across the FFI boundary uses null-terminated JSON C strings
- Two main operations: **extract** (persisted HNSW -> columnar) and **build** (columnar -> new HNSW)

## Layers

**Rust Core (`src/`):**
- Purpose: All data processing logic -- parsing persisted HNSW binary format, extracting vectors/metadata, building new indices, writing columnar output
- Location: `src/`
- Contains: Binary parsing, streaming extraction, columnar writers, index construction
- Depends on: `arrow-*`, `parquet`, `fast-hnsw`, `serde-pickle`, `serde_json`, `thiserror`
- Used by: FFI layer, Go bindings, direct Rust consumers

**FFI Layer (`src/ffi.rs`):**
- Purpose: Exposes Rust core as C-ABI functions callable from any language
- Location: `src/ffi.rs`
- Contains: 5 exported `extern "C"` symbols, JSON serde for request/response, panic catching, last-error slot
- Depends on: Rust core modules (`extractor`, `importer`, `metadata`)
- Used by: Go bindings (via dynamic loading)

**Go Bindings (`hnswtoolbox.go`):**
- Purpose: Idiomatic Go API wrapping the Rust FFI via dynamic library loading
- Location: `hnswtoolbox.go`
- Contains: Go types mirroring Rust structs, `Init`/`Close` lifecycle, `ExtractIndex`/`BuildIndex` functions
- Depends on: `github.com/ebitengine/purego` for `dlopen`/`dlsym` without CGO
- Used by: Go consumers of the library

## Data Flow

**Extract Flow (persisted HNSW -> columnar file):**

1. Go caller invokes `hnswtoolbox.ExtractIndex(request)` with index directory path
2. Go marshals `ExtractRequest` to JSON, appends null terminator, calls `fnExtractIndex` FFI symbol
3. Rust `hnsw_toolbox_extract_index` in `src/ffi.rs` deserializes JSON into `ExtractRequest`
4. Rust `extract_index_to_columnar` in `src/extractor.rs` resolves `IndexPaths` (`header.bin` + `data_level0.bin`)
5. `PersistentHeader::from_path` in `src/header.rs` parses the binary header (supports 32-bit and 64-bit word sizes)
6. `extract_index_with_header` streams records from `data_level0.bin` via `BufReader`, parsing label/vector/deleted per record
7. Optional metadata join from `index_metadata.pickle` via `src/metadata.rs` (adds `user_id`/`seq_id`)
8. `ColumnarWriter` buffers records into Arrow `RecordBatch` arrays, flushing at configurable `batch_size`
9. `ColumnarSink` writes batches to either Parquet or Arrow IPC file format
10. Rust returns `ExtractResponse` JSON; Go deserializes and returns `*ExtractResponse`

**Build Flow (columnar file -> new HNSW index):**

1. Go caller invokes `hnswtoolbox.BuildIndex(request)` with input Parquet/Arrow path
2. Same JSON FFI protocol as extract
3. Rust `build_index_from_columnar` in `src/importer.rs` opens the columnar file
4. Reads `RecordBatch` rows, resolves `vector`, `label`, `deleted` columns with flexible type handling
5. Inserts non-deleted vectors into `fast_hnsw::labeled::LabeledIndex` with configured distance metric
6. Saves index to output path via `index.save()`
7. Returns `BuildResponse` JSON with scan/insert/skip statistics

**State Management:**
- No persistent state; each operation is stateless (reads input files, writes output files)
- Go module has `Init`/`Close` lifecycle for the shared library handle (protected by `sync.Mutex`)
- FFI calls are serialized through a `callMu` mutex on the Go side
- Rust FFI uses a `static LAST_ERROR: Mutex<Option<CString>>` for error reporting

## Key Abstractions

**PersistentHeader (`src/header.rs`):**
- Purpose: Represents the binary header of a persisted hnswlib index
- Examples: `src/header.rs` lines 24-40
- Pattern: Parses fixed-size binary layout with `HeaderCursor`; supports both 32-bit (60 bytes) and 64-bit (100 bytes) word sizes; validates structural invariants after parsing

**ExtractedRecord (`src/extractor.rs`):**
- Purpose: Single vector record extracted from a persisted index
- Fields: `internal_id`, `label`, `deleted`, `vector: Vec<f32>`, optional `user_id`/`seq_id`
- Pattern: Streaming callback -- `extract_index` accepts `FnMut(ExtractedRecord)` for record-at-a-time processing

**ColumnarWriter / ColumnarSink (`src/extractor.rs`):**
- Purpose: Buffered batch writer that accumulates records into Arrow arrays and flushes to Parquet or Arrow IPC
- Pattern: Builder pattern with `BatchBuffer` holding Arrow builders; flushes at `batch_size` threshold

**BuildOptions / BuildState (`src/importer.rs`):**
- Purpose: Configuration for index construction (metric, M, ef_construction, etc.) and running statistics
- Pattern: Options struct with defaults; state tracks scanned/inserted/skipped counts and enforces dimension consistency

**FFI Request/Response (`src/ffi.rs`):**
- Purpose: JSON-serializable structs bridging Go and Rust across C-ABI
- Examples: `ExtractRequest`/`ExtractResponse`, `BuildRequest`/`BuildResponse`
- Pattern: Serde derive with `#[serde(default)]` for optional fields; responses returned as heap-allocated CStrings

## Entry Points

**Rust Library (`src/lib.rs`):**
- Location: `src/lib.rs`
- Triggers: Direct Rust usage or FFI calls
- Responsibilities: Re-exports public API from all modules

**FFI Symbols (`src/ffi.rs`):**
- Location: `src/ffi.rs`
- Triggers: Dynamic library loading (`dlopen` + `dlsym`)
- Responsibilities: 5 exported functions: `hnsw_toolbox_version`, `hnsw_toolbox_extract_index`, `hnsw_toolbox_build_index`, `hnsw_toolbox_get_last_error`, `hnsw_toolbox_free_string`

**Go Package (`hnswtoolbox.go`):**
- Location: `hnswtoolbox.go`
- Triggers: `hnswtoolbox.Init(libraryPath)` must be called before any operations
- Responsibilities: `Init`, `Close`, `Version`, `LastError`, `ExtractIndex`, `BuildIndex`

## Error Handling

**Strategy:** Typed error enum in Rust, string-based errors across FFI, Go `error` interface

**Patterns:**
- Rust uses `thiserror`-derived `ExtractError` enum (`src/error.rs`) with variants for IO, parsing, corruption, validation
- FFI layer catches panics via `std::panic::catch_unwind` and stores error messages in a global `LAST_ERROR` mutex slot
- FFI functions return `null` on error; caller checks `hnsw_toolbox_get_last_error()` for the message
- Go side checks for null return pointer, reads `LastError()`, wraps in `errors.New`
- Go functions also validate inputs (empty strings) before crossing FFI boundary

## Cross-Cutting Concerns

**Logging:** None -- no logging framework on either side. Errors propagate via return values.

**Validation:**
- Rust: Header validation in `src/header.rs` (`PersistentHeader::validate`); build option validation in `src/importer.rs` (`validate_options`); input column type checking in importer
- FFI: JSON parsing validation, null pointer checks, empty string checks
- Go: Empty string checks on required fields before FFI call

**Thread Safety:**
- Go: `stateMu` mutex protects `Init`/`Close`/`isLoaded`; `callMu` mutex serializes all FFI calls
- Rust FFI: `LAST_ERROR` is `Mutex<Option<CString>>`; each FFI call is effectively single-threaded from Go side

**Memory Management:**
- Rust allocates response JSON via `CString::into_raw`; Go must call `hnsw_toolbox_free_string` to deallocate
- Go reads the C string into a Go-managed `string` before freeing the Rust-allocated pointer

---

*Architecture analysis: 2026-03-26*
