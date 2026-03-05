package hnswtoolbox

import (
	"encoding/json"
	"testing"
)

func TestExtractResponseUnmarshalIncludesIndexProperties(t *testing.T) {
	raw := []byte(`{
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
	}`)

	var response ExtractResponse
	if err := json.Unmarshal(raw, &response); err != nil {
		t.Fatalf("unmarshal extract response: %v", err)
	}

	if response.Summary.IndexProperties.M != 16 {
		t.Errorf("M mismatch: got %d", response.Summary.IndexProperties.M)
	}
	if response.Summary.IndexProperties.EfConstruction != 200 {
		t.Errorf("EfConstruction mismatch: got %d", response.Summary.IndexProperties.EfConstruction)
	}
	if response.Summary.IndexProperties.CurElementCount != 10000 {
		t.Errorf("CurElementCount mismatch: got %d", response.Summary.IndexProperties.CurElementCount)
	}
	if response.Summary.IndexProperties.MaxElements != 12000 {
		t.Errorf("MaxElements mismatch: got %d", response.Summary.IndexProperties.MaxElements)
	}
	if response.Summary.IndexProperties.PersistedVersion != 1 {
		t.Errorf("PersistedVersion mismatch: got %d", response.Summary.IndexProperties.PersistedVersion)
	}
	if response.Summary.IndexProperties.WordSizeBytes != 8 {
		t.Errorf("WordSizeBytes mismatch: got %d", response.Summary.IndexProperties.WordSizeBytes)
	}
}
