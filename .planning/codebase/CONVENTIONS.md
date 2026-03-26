# Coding Conventions

**Analysis Date:** 2026-03-26

## Languages

This is a dual-language codebase: Rust (core library + FFI) and Go (bindings). Follow the conventions of each language in its respective files.

## Naming Patterns

**Rust Files:**
- snake_case for files: `extractor.rs`, `data_level0.bin`
- snake_case for functions: `extract_index`, `build_index_from_columnar`, `parse_label`
- snake_case for variables: `raw_record`, `vector_start`, `label_offset`
- PascalCase for types and enums: `ExtractError`, `HeaderWordSize`, `OutputFormat`, `PersistentHeader`
- SCREAMING_SNAKE_CASE for constants: `HNSW_PERSISTENCE_VERSION`, `LAST_ERROR`, `VERSION`
- Enum variants use PascalCase: `HeaderWordSize::U32`, `ExtractError::CorruptIndex`

**Go Files:**
- PascalCase for exported types and functions: `ExtractIndex`, `BuildRequest`, `ExtractSummary`
- camelCase for unexported identifiers: `stateMu`, `callMu`, `fnExtractIndex`, `goStringFromPtr`
- Package name is lowercase single word: `hnswtoolbox`
- JSON tags use snake_case: `` `json:"output_path"` ``, `` `json:"ef_construction"` ``

**FFI Symbols:**
- Prefix all C-exported symbols with `hnsw_toolbox_`: `hnsw_toolbox_version`, `hnsw_toolbox_extract_index`, `hnsw_toolbox_get_last_error`

## Code Style

**Formatting:**
- Rust: `cargo fmt` (standard rustfmt, no custom `.rustfmt.toml`)
- Go: `gofmt -w .` + `goimports -w .` (if available)

**Linting:**
- Rust: `cargo clippy --locked --all-targets -- -D warnings` (zero warnings policy)
- Go: `golangci-lint v2.5.0` with `--timeout=5m`
- No custom lint config files (`.golangci.yml`, `.clippy.toml`) -- uses tool defaults

**Run all formatting and linting:**
```bash
make fmt    # Format both Go and Rust
make lint   # Lint both Go and Rust
```

## Import Organization

**Rust (see `src/extractor.rs`, `src/importer.rs`):**
1. `std` imports grouped together
2. Blank line
3. External crate imports (`arrow_*`, `parquet`, `serde`, `fast_hnsw`)
4. Blank line
5. `crate::` internal imports

Example from `src/extractor.rs`:
```rust
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{...};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_ipc::writer::FileWriter as ArrowIpcFileWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use serde::{Deserialize, Serialize};

use crate::error::ExtractError;
use crate::header::{HeaderWordSize, PersistentHeader};
use crate::metadata::MetadataEntry;
```

**Go (see `hnswtoolbox.go`):**
- Standard library imports first, then external packages, grouped with blank lines
```go
import (
    "encoding/json"
    "errors"
    "fmt"
    "strings"
    "sync"
    "unsafe"

    "github.com/ebitengine/purego"
)
```

## Error Handling

**Rust Error Pattern:**
- Single unified error enum `ExtractError` in `src/error.rs` using `thiserror::Error`
- All public functions return `Result<T, ExtractError>`
- Use `#[from]` for automatic conversion from `io::Error`, `serde_pickle::Error`, `serde_json::Error`, `ArrowError`, `ParquetError`
- Domain errors use named variants with descriptive string messages: `InvalidHeader(String)`, `CorruptIndex(String)`, `InvalidInput(String)`, `MissingFile(PathBuf)`
- Use `?` operator for propagation; avoid `unwrap()` in non-test code

Example from `src/header.rs`:
```rust
if self.cur_element_count > self.max_elements {
    return Err(ExtractError::InvalidHeader(format!(
        "cur_element_count {} exceeds max_elements {}",
        self.cur_element_count, self.max_elements
    )));
}
```

**Rust FFI Error Pattern (see `src/ffi.rs`):**
- FFI functions wrap logic in `std::panic::catch_unwind`
- On error, store message via `set_last_error_message()` and return `ptr::null_mut()`
- On success, clear last error via `clear_last_error()` and return JSON response as `*mut c_char`
- Thread-safe global error slot: `static LAST_ERROR: Mutex<Option<CString>>`

**Go Error Pattern (see `hnswtoolbox.go`):**
- Return `(T, error)` tuples per Go convention
- Validate inputs early with descriptive `errors.New(...)` messages
- Wrap underlying errors with `fmt.Errorf("context: %w", err)`
- Check FFI null returns and read last error string from Rust side

## Module Design

**Rust Module Structure (`src/lib.rs`):**
- Each module in its own file: `error.rs`, `extractor.rs`, `ffi.rs`, `header.rs`, `importer.rs`, `metadata.rs`
- `lib.rs` declares public modules and re-exports key types
- Re-exports follow pattern: `pub use module::Type`
- Internal helpers are private functions within each module (no `pub`)

**Exports (see `src/lib.rs`):**
```rust
pub use error::ExtractError;
pub use extractor::{
    extract_index, extract_index_to_columnar, ExtractIndexProperties, ExtractOptions,
    ExtractSummary, ExtractedRecord, OutputFormat,
};
pub use header::{HeaderWordSize, PersistentHeader, HNSW_PERSISTENCE_VERSION};
pub use importer::{
    build_index_from_columnar, BuildOptions, BuildSummary, DistanceMetric, InputFormat,
};
pub use metadata::{load_chroma_metadata, MetadataEntry};
```

**Go Package Design (`hnswtoolbox.go`):**
- Single-file package with all public API
- Lifecycle: `Init(libraryPath) -> use ExtractIndex/BuildIndex -> Close()`
- Thread safety via `sync.Mutex` for both state (`stateMu`) and FFI calls (`callMu`)
- No barrel files or sub-packages

## Struct Design

**Rust:**
- Use `#[derive(Debug, Clone, Serialize)]` on response/summary types
- Use `#[derive(Debug, Clone, Deserialize)]` on request types
- Implement `Default` for option structs (see `BuildOptions`, `ExtractOptions`)
- Use `#[serde(rename_all = "snake_case")]` for enum serialization
- Use `#[serde(default)]` on optional request fields in FFI layer
- Private internal state structs (e.g., `BuildState`, `BatchBuffer`, `HeaderCursor`) are not `pub`

**Go:**
- Public request/response structs use PascalCase fields
- Internal FFI payload structs use JSON tags with `omitempty`
- Optional fields use pointers: `M0 *int`, `Capacity *int`, `Seed *uint64`

## Documentation Style

**Rust:**
- `/// Safety` doc comments on all `unsafe extern "C"` FFI functions (see `src/ffi.rs`)
- No doc comments on internal/private functions
- Inline comments are rare and used only for non-obvious behavior (e.g., `// Decode raw lanes directly to preserve exact f32 bit patterns`)

**Go:**
- No doc comments on exported functions currently

## Comments

- Minimal commenting overall; code is expected to be self-explanatory via naming
- No TODO/FIXME/HACK comments in the codebase
- Single inline comment style where binary format details need explanation

## Commit Message Conventions

- Use conventional commit style: `feat:`, `fix:`, lowercase imperative
- Short single-line messages preferred
- Examples from git log:
  - `feat: add hnsw extraction toolbox with ffi, purego bindings, tests, and ci`
  - `fix clippy manual_is_multiple_of lint`
  - `add columnar hnsw build path and bit-exact f32 export tests`
  - `Use t.Errorf for index properties JSON assertions`
  - `Address review feedback for index_properties contract`

## Serde Conventions

**Enum serialization (Rust):**
- Always use `#[serde(rename_all = "snake_case")]` for enums exposed through JSON
- Enums with default variants use `#[default]` attribute

**Request/Response JSON contract:**
- snake_case field names in JSON (both Rust and Go sides)
- Optional fields default via `unwrap_or_default()` or `unwrap_or(value)` on Rust side
- Go side mirrors with `omitempty` JSON tags

---

*Convention analysis: 2026-03-26*
