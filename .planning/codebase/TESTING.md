# Testing Patterns

**Analysis Date:** 2026-03-26

## Test Frameworks

**Rust:**
- Runner: Built-in `cargo test` (no external test runner)
- Assertion: Standard `assert_eq!`, `assert!` macros
- Property testing: `proptest 1.6` for randomized/generative tests
- Temp files: `tempfile 3.15` crate for test directory isolation
- Config: No separate test config file; tests are inline via `#[cfg(test)]`

**Go:**
- Runner: Built-in `go test`
- Assertion: Standard `testing.T` methods (`t.Fatalf`, `t.Errorf`)
- No external assertion libraries (no testify, no gomock)

**Run Commands:**
```bash
make test          # Run all tests (Go + Rust)
make test-rust     # Run Rust tests only
make test-go       # Run Go tests only
cargo test --locked  # Rust tests directly
go test ./...        # Go tests directly
```

## Test File Organization

**Rust -- Co-located inline tests:**
- Tests live inside the same file as the code they test, in a `#[cfg(test)] mod tests {}` block
- Files with tests: `src/extractor.rs` (6 tests), `src/importer.rs` (3 tests)
- Files without tests: `src/error.rs`, `src/ffi.rs`, `src/header.rs`, `src/metadata.rs`, `src/lib.rs`

**Go -- Co-located test file:**
- Test file: `hnswtoolbox_test.go` (same package `hnswtoolbox`)
- Tests: 1 test function (`TestExtractResponseUnmarshalIncludesIndexProperties`)

**No separate test directories.** All tests are co-located with source.

## Rust Test Structure

**Suite Organization Pattern (from `src/extractor.rs`):**
```rust
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;
    // ... test-specific imports

    use super::*;  // Always import parent module

    // Fixture helpers defined as private functions
    fn write_fixture(dir: &Path, records: &[FixtureRecord]) -> Result<(), ExtractError> { ... }

    #[test]
    fn extract_skips_deleted_by_default() {
        // 1. Create temp dir
        let tmp = tempdir().expect("tempdir");
        // 2. Build fixture data
        let records = vec![...];
        write_fixture(tmp.path(), &records).expect("fixture");
        // 3. Execute operation
        let summary = extract_index(...).expect("extract");
        // 4. Assert results
        assert_eq!(summary.scanned, 3);
        assert_eq!(summary.emitted, 2);
    }
}
```

**Test naming:** snake_case descriptive names: `extract_skips_deleted_by_default`, `builds_hnsw_from_parquet`, `falls_back_to_internal_id_when_label_missing`, `columnar_exports_preserve_f32_bits_verbatim`

**Setup/teardown:** No shared setup. Each test creates its own `tempdir()` and builds fixtures inline. Temp dirs auto-clean on drop.

## Go Test Structure

**Pattern (from `hnswtoolbox_test.go`):**
```go
func TestExtractResponseUnmarshalIncludesIndexProperties(t *testing.T) {
    // 1. Define raw JSON fixture inline
    raw := []byte(`{...}`)
    // 2. Unmarshal
    var response ExtractResponse
    if err := json.Unmarshal(raw, &response); err != nil {
        t.Fatalf("unmarshal extract response: %v", err)
    }
    // 3. Assert individual fields
    if response.Summary.IndexProperties.M != 16 {
        t.Errorf("M mismatch: got %d", response.Summary.IndexProperties.M)
    }
}
```

**Error handling in Go tests:**
- Use `t.Fatalf` for fatal setup/unmarshal errors (stops the test)
- Use `t.Errorf` for assertion failures (continues to check other fields)

## Mocking

**No mocking frameworks used in either language.**

- Rust tests construct real binary fixtures (header.bin, data_level0.bin) and exercise real extraction/import logic
- Go tests only test JSON deserialization contract, not FFI calls
- The Go FFI integration requires a built Rust `.dylib/.so` at runtime, so Go tests avoid FFI calls

## Fixtures and Factories

**Rust Fixture Pattern (from `src/extractor.rs`):**

Test helper structs:
```rust
#[derive(Clone)]
struct FixtureRecord {
    label: u64,
    vector: Vec<f32>,
    deleted: bool,
}
```

Binary fixture writer (creates real HNSW header.bin + data_level0.bin):
```rust
fn write_fixture(dir: &Path, records: &[FixtureRecord]) -> Result<(), ExtractError> {
    write_fixture_with_capacity(dir, records, records.len())
}

fn write_fixture_with_capacity(
    dir: &Path,
    records: &[FixtureRecord],
    max_elements: usize,
) -> Result<(), ExtractError> {
    // Writes header.bin and data_level0.bin with correct binary layout
    // Fixed parameters: m=4, max_m0=16, ef_construction=100, word_size=8 bytes
}
```

**Rust Importer Fixture (from `src/importer.rs`):**

Arrow RecordBatch factory:
```rust
fn fixture_batch() -> RecordBatch {
    // Builds a 3-row RecordBatch with internal_id, label, deleted, vector columns
    // Uses arrow builder APIs directly
}
```

**Go Fixture (from `hnswtoolbox_test.go`):**
- Raw JSON byte literals defined inline in the test function
- No shared fixtures or factory functions

**Fixture Locations:**
- No separate `testdata/` or `fixtures/` directories
- All fixtures are generated programmatically within test modules

## Property-Based Testing

**Framework:** `proptest 1.6` (Rust only, see `src/extractor.rs`)

**Pattern:**
```rust
proptest! {
    #[test]
    fn property_exports_match_persisted_vectors_and_tombstones(
        (count, extra_capacity, rows) in
            (1usize..48, 0usize..16).prop_flat_map(|(count, extra_capacity)| {
                prop::collection::vec(
                    (prop::collection::vec(any::<u32>(), 1usize..17), any::<bool>()),
                    count
                )
                .prop_map(move |rows| (count, extra_capacity, rows))
            })
    ) {
        // Generate random HNSW fixtures, extract, and verify:
        // - Scanned/emitted/deleted counts are correct
        // - Vectors match bit-for-bit (f32 -> u32 bits comparison)
        // - Deleted flags are respected
        // Uses prop_assert_eq! instead of assert_eq!
    }
}
```

**What is property-tested:**
- Extraction correctness across random vector dimensions (1..17), random element counts (1..48), random deleted flags
- Bit-exact f32 preservation through the extract pipeline
- Both with-deleted and without-deleted extraction paths

## Coverage

**Requirements:** Not enforced. No coverage thresholds configured.
**Coverage tool:** Not configured. No `--coverage` flags in Makefile or CI.

## Test Types

**Unit Tests (Rust):**
- `src/extractor.rs`: 6 tests covering extraction with/without deleted records, metadata join, Parquet/Arrow IPC output, bit-exact f32 preservation, property-based extraction correctness
- `src/importer.rs`: 3 tests covering Parquet import, Arrow IPC import, label fallback to internal_id

**Unit Tests (Go):**
- `hnswtoolbox_test.go`: 1 test covering JSON deserialization of ExtractResponse with IndexProperties

**Integration Tests:**
- Rust importer tests are effectively integration tests: they write real Parquet/Arrow files, build HNSW indices via `fast-hnsw`, reload them, and verify search results
- No Go integration tests (would require building the Rust cdylib first)

**E2E Tests:**
- Not present. No end-to-end test that exercises Go -> FFI -> Rust -> file output

## Specific Test Scenarios

**Extractor tests (`src/extractor.rs`):**
| Test | What it verifies |
|------|-----------------|
| `extract_skips_deleted_by_default` | Deleted records excluded from output; summary counts correct; index_properties populated |
| `extract_includes_deleted_when_enabled` | `include_deleted: true` emits all records including tombstoned ones |
| `extract_joins_metadata` | Metadata join populates user_id and seq_id on extracted records |
| `writes_parquet_and_arrow_ipc` | Both output formats produce non-empty files with correct emit counts |
| `property_exports_match_persisted_vectors_and_tombstones` | Property test: random fixtures always produce correct extraction |
| `columnar_exports_preserve_f32_bits_verbatim` | Special float values (-0.0, subnormals, inf, NaN payloads) survive round-trip |

**Importer tests (`src/importer.rs`):**
| Test | What it verifies |
|------|-----------------|
| `builds_hnsw_from_parquet` | Parquet -> HNSW build; skip deleted; verify search results |
| `builds_hnsw_from_arrow_ipc` | Arrow IPC -> HNSW build; include deleted; verify search results |
| `falls_back_to_internal_id_when_label_missing` | Without label column, uses internal_id as payload |

**Go tests (`hnswtoolbox_test.go`):**
| Test | What it verifies |
|------|-----------------|
| `TestExtractResponseUnmarshalIncludesIndexProperties` | JSON contract: all IndexProperties fields deserialize correctly |

## CI/CD Test Configuration

**CI Pipeline:** GitHub Actions (`.github/workflows/ci.yml`)

**Jobs:**
1. **lint** - Runs on `ubuntu-latest`:
   - Go 1.21 + Rust stable
   - `golangci-lint v2.5.0` for Go
   - `cargo clippy --locked --all-targets -- -D warnings` for Rust
2. **test** - Runs on `ubuntu-latest`:
   - Go 1.21 + Rust stable
   - Runs `make test` (both `go test ./...` and `cargo test --locked`)
3. **build** - Runs on `ubuntu-latest`:
   - Rust stable only
   - Runs `make build` (debug) and `make build-release`

**Triggers:** push to `main`, pull requests to `main`, `workflow_dispatch`

**Caching:** `Swatinem/rust-cache@v2` for Cargo build cache in all jobs

**Additional CI Workflows:**
- `.github/workflows/claude.yml` - Claude Code for issue/PR comments
- `.github/workflows/claude-code-review.yml` - Claude Code Review (currently `workflow_dispatch` only, PR trigger commented out)

## Test Gaps

**Areas without test coverage:**
- `src/header.rs`: No direct unit tests for `PersistentHeader::from_bytes`, `PersistentHeader::from_path`, or validation logic (tested indirectly through extractor tests)
- `src/metadata.rs`: No direct unit tests for `load_chroma_metadata` (tested indirectly through extractor metadata join test)
- `src/error.rs`: No tests for error Display formatting
- `src/ffi.rs`: No tests for FFI entry points, null pointer handling, panic catching, or error slot behavior
- Go FFI integration: No tests that actually call the Rust library through purego
- Go input validation: No tests for `Init("")`, `ExtractIndex` with empty fields, `Close` idempotency
- 32-bit header parsing: All fixtures use 64-bit word size; no tests for `HeaderWordSize::U32` path

---

*Testing analysis: 2026-03-26*
