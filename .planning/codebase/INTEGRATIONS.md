# External Integrations

**Analysis Date:** 2026-03-26

## APIs & External Services

**None.** This is a pure offline library with no network communication. All operations are local filesystem reads/writes.

## Data Storage

**Databases:**
- None. No database connections.

**File Formats (Read):**
- Chroma/hnswlib persisted index directory (read by `src/extractor.rs`):
  - `header.bin` - 60 or 100 bytes of HNSW index metadata (parsed in `src/header.rs`)
  - `data_level0.bin` - Raw binary records containing vectors, labels, and delete markers
  - `index_metadata.pickle` - Optional Chroma-specific label-to-user-id mappings (parsed in `src/metadata.rs` using `serde-pickle`)
- Apache Parquet files (read by `src/importer.rs` via `parquet` crate)
- Apache Arrow IPC files (read by `src/importer.rs` via `arrow-ipc` crate)

**File Formats (Write):**
- Apache Parquet (written by `src/extractor.rs` via `parquet::arrow::ArrowWriter`)
- Apache Arrow IPC (written by `src/extractor.rs` via `arrow_ipc::writer::FileWriter`)
- `fast-hnsw` native format `.hnsw` (written by `src/importer.rs` via `fast_hnsw::labeled::LabeledIndex::save`)

**File Storage:**
- Local filesystem only

**Caching:**
- None

## Chroma Integration

This toolbox is designed specifically to work with [Chroma](https://www.trychroma.com/) HNSW index files. Key integration points:

**Index extraction (`src/extractor.rs`):**
- Reads the hnswlib persistence format used by Chroma's vector segments
- Understands the binary layout: link-list prefix, vector payload, label suffix, delete marker
- Header parsing in `src/header.rs` supports both 32-bit and 64-bit word sizes

**Metadata join (`src/metadata.rs`):**
- Parses Chroma's `index_metadata.pickle` files containing:
  - `label_to_id`: Maps hnswlib integer labels to Chroma string document IDs
  - `id_to_seq_id`: Maps Chroma document IDs to sequence IDs
- Supports both u64 and i64 key types in the serialized data

**Index rebuild (`src/importer.rs`):**
- Builds new HNSW indices from extracted columnar data using `fast-hnsw`
- Note: output format is `fast-hnsw` native, NOT hnswlib/Chroma persistence format
- Supports distance metrics: Euclidean, SquaredEuclidean, Cosine, DotProduct, Manhattan

## FFI / Inter-Language Communication

**Rust-to-Go bridge (`src/ffi.rs` -> `hnswtoolbox.go`):**
- Communication pattern: JSON-over-C-ABI
- Rust exposes 5 C-ABI symbols via `#[no_mangle] pub unsafe extern "C"` functions:
  - `hnsw_toolbox_version() -> *const c_char`
  - `hnsw_toolbox_extract_index(*const c_char) -> *mut c_char`
  - `hnsw_toolbox_build_index(*const c_char) -> *mut c_char`
  - `hnsw_toolbox_get_last_error() -> *const c_char`
  - `hnsw_toolbox_free_string(*mut c_char)`
- Go loads these symbols via `purego.RegisterLibFunc` in `hnswtoolbox.go`
- Request/response payloads are JSON-encoded null-terminated C strings
- Error handling: null return from extract/build functions signals error; caller reads `get_last_error`
- Thread safety: Go side uses `sync.Mutex` (`callMu`) to serialize all FFI calls; Rust side uses `std::sync::Mutex` for the last-error slot

**Go API surface (`hnswtoolbox.go`):**
- `Init(libraryPath string) error` - Load shared library and bind symbols
- `Close() error` - Unload shared library
- `Version() (string, error)` - Get library version
- `ExtractIndex(ExtractRequest) (*ExtractResponse, error)` - Extract HNSW index to Parquet/Arrow
- `BuildIndex(BuildRequest) (*BuildResponse, error)` - Build new HNSW from Parquet/Arrow
- `LastError() string` - Read last native error message

## Authentication & Identity

**Auth Provider:**
- Not applicable (offline library)

## Monitoring & Observability

**Error Tracking:**
- None (library consumers are responsible)

**Logs:**
- None. No logging framework. Errors propagated via `Result<T, ExtractError>` in Rust and `error` returns in Go.

## CI/CD & Deployment

**Hosting:**
- GitHub repository: `github.com/amikos-tech/hnsw-toolbox`

**CI Pipeline:**
- GitHub Actions (`.github/workflows/ci.yml`)
- Triggers: push to `main`, pull requests to `main`, manual dispatch
- Jobs: lint (clippy + golangci-lint), test (make test), build (debug + release)

**Deployment:**
- Library distribution only. No server deployment.
- Consumers must build the Rust shared library for their target platform, then reference it from Go via `Init(path)`.

## Environment Configuration

**Required env vars:**
- None

**Optional env vars:**
- `CARGO_TARGET_DIR` - Override Cargo build output directory (used by `Makefile`)

**Secrets:**
- `CLAUDE_CODE_OAUTH_TOKEN` - GitHub Actions secret for Claude Code workflows only; not needed for library operation

## Webhooks & Callbacks

**Incoming:**
- None

**Outgoing:**
- None

## Arrow Columnar Schema

The columnar exchange format between extract and build operations uses this Arrow schema (defined in `src/extractor.rs` `build_schema()`):

| Column        | Arrow Type                              | Nullable |
|---------------|-----------------------------------------|----------|
| `internal_id` | `UInt64`                                | No       |
| `label`       | `UInt64`                                | No       |
| `deleted`     | `Boolean`                               | No       |
| `user_id`     | `Utf8`                                  | Yes      |
| `seq_id`      | `UInt64`                                | Yes      |
| `vector`      | `FixedSizeList(Float32, dim)`           | No       |

The importer (`src/importer.rs`) requires at minimum a `vector` column; `label`, `internal_id`, and `deleted` are optional with documented fallback behavior.

---

*Integration audit: 2026-03-26*
