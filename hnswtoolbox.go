package hnswtoolbox

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

type OutputFormat string

const (
	OutputFormatParquet  OutputFormat = "parquet"
	OutputFormatArrowIPC OutputFormat = "arrow_ipc"
)

type InputFormat string

const (
	InputFormatParquet  InputFormat = "parquet"
	InputFormatArrowIPC InputFormat = "arrow_ipc"
)

type DistanceMetric string

const (
	DistanceMetricEuclidean        DistanceMetric = "euclidean"
	DistanceMetricSquaredEuclidean DistanceMetric = "squared_euclidean"
	DistanceMetricCosine           DistanceMetric = "cosine"
	DistanceMetricDotProduct       DistanceMetric = "dot_product"
	DistanceMetricManhattan        DistanceMetric = "manhattan"
)

type ExtractRequest struct {
	IndexDir       string
	OutputPath     string
	OutputFormat   OutputFormat
	MetadataPath   string
	IncludeDeleted bool
	BatchSize      int
}

type ExtractSummary struct {
	Scanned        uint64 `json:"scanned"`
	Emitted        uint64 `json:"emitted"`
	DeletedSkipped uint64 `json:"deleted_skipped"`
	Dimension      int    `json:"dimension"`
}

type ExtractResponse struct {
	OutputPath   string         `json:"output_path"`
	OutputFormat OutputFormat   `json:"output_format"`
	Summary      ExtractSummary `json:"summary"`
}

type BuildRequest struct {
	InputPath      string
	OutputPath     string
	InputFormat    InputFormat
	Metric         DistanceMetric
	IncludeDeleted bool
	M              int
	M0             *int
	EfConstruction int
	BatchSize      int
	Capacity       *int
	Seed           *uint64
}

type BuildSummary struct {
	Scanned        uint64 `json:"scanned"`
	Inserted       uint64 `json:"inserted"`
	DeletedSkipped uint64 `json:"deleted_skipped"`
	Dimension      int    `json:"dimension"`
}

type BuildResponse struct {
	InputPath   string         `json:"input_path"`
	OutputPath  string         `json:"output_path"`
	InputFormat InputFormat    `json:"input_format"`
	Metric      DistanceMetric `json:"metric"`
	Summary     BuildSummary   `json:"summary"`
}

type extractPayload struct {
	IndexDir       string       `json:"index_dir"`
	OutputPath     string       `json:"output_path"`
	OutputFormat   OutputFormat `json:"output_format,omitempty"`
	MetadataPath   *string      `json:"metadata_path,omitempty"`
	IncludeDeleted bool         `json:"include_deleted,omitempty"`
	BatchSize      int          `json:"batch_size,omitempty"`
}

type buildPayload struct {
	InputPath      string         `json:"input_path"`
	OutputPath     string         `json:"output_path"`
	InputFormat    InputFormat    `json:"input_format,omitempty"`
	Metric         DistanceMetric `json:"metric,omitempty"`
	IncludeDeleted bool           `json:"include_deleted,omitempty"`
	M              int            `json:"m,omitempty"`
	M0             *int           `json:"m0,omitempty"`
	EfConstruction int            `json:"ef_construction,omitempty"`
	BatchSize      int            `json:"batch_size,omitempty"`
	Capacity       *int           `json:"capacity,omitempty"`
	Seed           *uint64        `json:"seed,omitempty"`
}

var (
	stateMu sync.Mutex
	callMu  sync.Mutex

	libHandle uintptr
	loaded    bool

	fnExtractIndex func(*byte) *byte
	fnBuildIndex   func(*byte) *byte
	fnLastError    func() *byte
	fnFreeCString  func(*byte)
	fnVersion      func() *byte
)

func Init(libraryPath string) error {
	stateMu.Lock()
	defer stateMu.Unlock()

	if strings.TrimSpace(libraryPath) == "" {
		return errors.New("libraryPath is required")
	}
	if loaded {
		return nil
	}

	handle, err := purego.Dlopen(libraryPath, purego.RTLD_NOW|purego.RTLD_LOCAL)
	if err != nil {
		return fmt.Errorf("failed to load %q: %w", libraryPath, err)
	}

	if err := register(handle, &fnExtractIndex, "hnsw_toolbox_extract_index"); err != nil {
		_ = purego.Dlclose(handle)
		return err
	}
	if err := register(handle, &fnBuildIndex, "hnsw_toolbox_build_index"); err != nil {
		_ = purego.Dlclose(handle)
		return err
	}
	if err := register(handle, &fnLastError, "hnsw_toolbox_get_last_error"); err != nil {
		_ = purego.Dlclose(handle)
		return err
	}
	if err := register(handle, &fnFreeCString, "hnsw_toolbox_free_string"); err != nil {
		_ = purego.Dlclose(handle)
		return err
	}
	if err := register(handle, &fnVersion, "hnsw_toolbox_version"); err != nil {
		_ = purego.Dlclose(handle)
		return err
	}

	libHandle = handle
	loaded = true
	return nil
}

func Close() error {
	stateMu.Lock()
	defer stateMu.Unlock()

	if !loaded {
		return nil
	}
	if err := purego.Dlclose(libHandle); err != nil {
		return fmt.Errorf("failed to close dynamic library: %w", err)
	}
	libHandle = 0
	loaded = false
	return nil
}

func Version() (string, error) {
	if err := ensureLoaded(); err != nil {
		return "", err
	}
	callMu.Lock()
	defer callMu.Unlock()

	ptr := fnVersion()
	if ptr == nil {
		return "", errors.New("version symbol returned nil")
	}
	return goStringFromPtr(ptr), nil
}

func LastError() string {
	if !isLoaded() {
		return ""
	}
	callMu.Lock()
	defer callMu.Unlock()
	return goStringFromPtr(fnLastError())
}

func ExtractIndex(request ExtractRequest) (*ExtractResponse, error) {
	if err := ensureLoaded(); err != nil {
		return nil, err
	}
	if strings.TrimSpace(request.IndexDir) == "" {
		return nil, errors.New("IndexDir is required")
	}
	if strings.TrimSpace(request.OutputPath) == "" {
		return nil, errors.New("OutputPath is required")
	}

	payload := extractPayload{
		IndexDir:       request.IndexDir,
		OutputPath:     request.OutputPath,
		OutputFormat:   request.OutputFormat,
		IncludeDeleted: request.IncludeDeleted,
		BatchSize:      request.BatchSize,
	}
	if request.MetadataPath != "" {
		metadataPath := request.MetadataPath
		payload.MetadataPath = &metadataPath
	}

	rawPayload, err := json.Marshal(payload)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal extract request: %w", err)
	}
	cPayload := append(rawPayload, 0)

	callMu.Lock()
	responsePtr := fnExtractIndex(&cPayload[0])
	if responsePtr == nil {
		errorMessage := goStringFromPtr(fnLastError())
		callMu.Unlock()
		if strings.TrimSpace(errorMessage) == "" {
			errorMessage = "hnsw_toolbox_extract_index failed without error message"
		}
		return nil, errors.New(errorMessage)
	}

	responseJSON := goStringFromPtr(responsePtr)
	fnFreeCString(responsePtr)
	callMu.Unlock()

	var response ExtractResponse
	if err := json.Unmarshal([]byte(responseJSON), &response); err != nil {
		return nil, fmt.Errorf("failed to parse extract response: %w", err)
	}
	return &response, nil
}

func BuildIndex(request BuildRequest) (*BuildResponse, error) {
	if err := ensureLoaded(); err != nil {
		return nil, err
	}
	if strings.TrimSpace(request.InputPath) == "" {
		return nil, errors.New("InputPath is required")
	}
	if strings.TrimSpace(request.OutputPath) == "" {
		return nil, errors.New("OutputPath is required")
	}

	payload := buildPayload(request)

	rawPayload, err := json.Marshal(payload)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal build request: %w", err)
	}
	cPayload := append(rawPayload, 0)

	callMu.Lock()
	responsePtr := fnBuildIndex(&cPayload[0])
	if responsePtr == nil {
		errorMessage := goStringFromPtr(fnLastError())
		callMu.Unlock()
		if strings.TrimSpace(errorMessage) == "" {
			errorMessage = "hnsw_toolbox_build_index failed without error message"
		}
		return nil, errors.New(errorMessage)
	}

	responseJSON := goStringFromPtr(responsePtr)
	fnFreeCString(responsePtr)
	callMu.Unlock()

	var response BuildResponse
	if err := json.Unmarshal([]byte(responseJSON), &response); err != nil {
		return nil, fmt.Errorf("failed to parse build response: %w", err)
	}
	return &response, nil
}

func isLoaded() bool {
	stateMu.Lock()
	defer stateMu.Unlock()
	return loaded
}

func ensureLoaded() error {
	if !isLoaded() {
		return errors.New("library is not initialized; call Init first")
	}
	return nil
}

func register(handle uintptr, fn any, name string) (err error) {
	defer func() {
		if recovered := recover(); recovered != nil {
			err = fmt.Errorf("failed to bind symbol %q: %v", name, recovered)
		}
	}()
	purego.RegisterLibFunc(fn, handle, name)
	return nil
}

func goStringFromPtr(ptr *byte) string {
	if ptr == nil {
		return ""
	}
	buf := make([]byte, 0, 64)
	for i := uintptr(0); ; i++ {
		b := *(*byte)(unsafe.Add(unsafe.Pointer(ptr), i))
		if b == 0 {
			break
		}
		buf = append(buf, b)
	}
	return string(buf)
}
