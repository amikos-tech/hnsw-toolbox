# hnsw-toolbox

Low-memory extraction utilities for Chroma/HNSW persisted indices, exposed as:

- Rust library APIs
- C-ABI FFI (`cdylib`)
- Pure Go bindings via [`purego`](https://github.com/ebitengine/purego) (no CGO)

## What It Extracts

From a persisted HNSW directory (`header.bin`, `data_level0.bin`):

- `internal_id`
- `label`
- `deleted`
- `vector`

Optional metadata join from `index_metadata.pickle`:

- `user_id`
- `seq_id`

Extract summary also includes `index_properties` sourced from `header.bin`:

- `m`
- `ef_construction`
- `cur_element_count`
- `max_elements`
- `persisted_version`
- `word_size_bytes`

Output format:

- `parquet`
- `arrow_ipc`

## What It Builds

From a columnar export (`parquet` or `arrow_ipc`) containing at least:

- `vector` (fixed-size list/list of float32/float64)
- optional `label` (otherwise falls back to `internal_id`, then row index)
- optional `deleted` (bool/int)

Build output:

- A new persisted HNSW file (`.hnsw`) produced by the Rust `fast-hnsw` format.

Note: this build output is not Chroma/hnswlib's native persistence layout (`header.bin`, `data_level0.bin`, ...).

## Build

```bash
cargo build
```

Or via Make:

```bash
make build
make build-release
```

Dynamic library output (platform-specific):

- macOS: `target/debug/libhnsw_toolbox.dylib`
- Linux: `target/debug/libhnsw_toolbox.so`
- Windows: `target/debug/hnsw_toolbox.dll`

## Rust API (core)

Use `extract_index(...)` for record-by-record streaming callbacks, or
`extract_index_to_columnar(...)` for file output.

Use `build_index_from_columnar(...)` to build a new HNSW index file from
Parquet/Arrow exports.

## FFI API

### Exported Symbols

- `hnsw_toolbox_version() -> *const c_char`
- `hnsw_toolbox_extract_index(request_json: *const c_char) -> *mut c_char`
- `hnsw_toolbox_build_index(request_json: *const c_char) -> *mut c_char`
- `hnsw_toolbox_get_last_error() -> *const c_char`
- `hnsw_toolbox_free_string(ptr: *mut c_char)`

### Request JSON

```json
{
  "index_dir": "/path/to/vector-segment-dir",
  "output_path": "/tmp/rebuilt.parquet",
  "output_format": "parquet",
  "metadata_path": "/path/to/index_metadata.pickle",
  "include_deleted": false,
  "batch_size": 1024
}
```

`output_format`: `"parquet"` or `"arrow_ipc"` (defaults to `"parquet"`).

### Response JSON

```json
{
  "output_path": "/tmp/rebuilt.parquet",
  "output_format": "parquet",
  "summary": {
    "scanned": 10000,
    "emitted": 8000,
    "deleted_skipped": 2000,
    "dimension": 384,
    "index_properties": {
      "m": 16,
      "ef_construction": 200,
      "cur_element_count": 10000,
      "max_elements": 12000,
      "persisted_version": 1,
      "word_size_bytes": 8
    }
  }
}
```

### Build Request JSON

```json
{
  "input_path": "/tmp/extracted.parquet",
  "output_path": "/tmp/rebuilt.hnsw",
  "input_format": "parquet",
  "metric": "euclidean",
  "include_deleted": false,
  "m": 16,
  "m0": 32,
  "ef_construction": 200,
  "batch_size": 1024,
  "capacity": 100000,
  "seed": 42
}
```

`input_format`: `"parquet"` or `"arrow_ipc"` (defaults to `"parquet"`).  
`metric`: `"euclidean"`, `"squared_euclidean"`, `"cosine"`, `"dot_product"`, `"manhattan"` (defaults to `"euclidean"`).

### Build Response JSON

```json
{
  "input_path": "/tmp/extracted.parquet",
  "output_path": "/tmp/rebuilt.hnsw",
  "input_format": "parquet",
  "metric": "euclidean",
  "summary": {
    "scanned": 10000,
    "inserted": 8000,
    "deleted_skipped": 2000,
    "dimension": 384
  }
}
```

## Go Usage (Purego, No CGO)

```go
import "github.com/amikos-tech/hnsw-toolbox"

err := hnswtoolbox.Init("/abs/path/to/libhnsw_toolbox.dylib")
if err != nil {
    panic(err)
}
defer hnswtoolbox.Close()

resp, err := hnswtoolbox.ExtractIndex(hnswtoolbox.ExtractRequest{
    IndexDir:       "/path/to/vector-segment-dir",
    OutputPath:     "/tmp/extracted.parquet",
    OutputFormat:   hnswtoolbox.OutputFormatParquet,
    MetadataPath:   "/path/to/index_metadata.pickle",
    IncludeDeleted: false,
    BatchSize:      1024,
})
if err != nil {
    panic(err)
}
_ = resp
```

Build a new index:

```go
buildResp, err := hnswtoolbox.BuildIndex(hnswtoolbox.BuildRequest{
    InputPath:      "/tmp/extracted.parquet",
    OutputPath:     "/tmp/rebuilt.hnsw",
    InputFormat:    hnswtoolbox.InputFormatParquet,
    Metric:         hnswtoolbox.DistanceMetricEuclidean,
    IncludeDeleted: false,
    M:              16,
    EfConstruction: 200,
    BatchSize:      1024,
})
if err != nil {
    panic(err)
}
_ = buildResp
```

## Validation

```bash
cargo test
cargo clippy --all-targets -- -D warnings
go test ./...
```

Or via Make:

```bash
make test
make lint
make fmt
```
