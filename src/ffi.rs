use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::extractor::{extract_index_to_columnar, ExtractOptions, ExtractSummary, OutputFormat};
use crate::importer::{
    build_index_from_columnar, BuildOptions, BuildSummary, DistanceMetric, InputFormat,
};
use crate::metadata::load_chroma_metadata;

static LAST_ERROR: Mutex<Option<CString>> = Mutex::new(None);
static VERSION: &[u8] = b"0.1.0\0";

#[derive(Debug, Deserialize)]
struct ExtractRequest {
    index_dir: String,
    output_path: String,
    #[serde(default)]
    output_format: Option<OutputFormat>,
    #[serde(default)]
    metadata_path: Option<String>,
    #[serde(default)]
    include_deleted: Option<bool>,
    #[serde(default)]
    batch_size: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ExtractResponse {
    output_path: String,
    output_format: OutputFormat,
    summary: ExtractSummary,
}

#[derive(Debug, Deserialize)]
struct BuildRequest {
    input_path: String,
    output_path: String,
    #[serde(default)]
    input_format: Option<InputFormat>,
    #[serde(default)]
    metric: Option<DistanceMetric>,
    #[serde(default)]
    include_deleted: Option<bool>,
    #[serde(default)]
    m: Option<usize>,
    #[serde(default)]
    m0: Option<usize>,
    #[serde(default)]
    ef_construction: Option<usize>,
    #[serde(default)]
    batch_size: Option<usize>,
    #[serde(default)]
    capacity: Option<usize>,
    #[serde(default)]
    seed: Option<u64>,
}

#[derive(Debug, Serialize)]
struct BuildResponse {
    input_path: String,
    output_path: String,
    input_format: InputFormat,
    metric: DistanceMetric,
    summary: BuildSummary,
}

/// Returns the library semantic version.
///
/// # Safety
/// The returned pointer is a static, null-terminated string and is always valid for reads.
#[no_mangle]
pub unsafe extern "C" fn hnsw_toolbox_version() -> *const c_char {
    VERSION.as_ptr() as *const c_char
}

/// Returns the last error message captured by the FFI layer.
///
/// # Safety
/// The returned pointer is owned by the library and remains valid until the next call that
/// mutates the last-error slot.
#[no_mangle]
pub unsafe extern "C" fn hnsw_toolbox_get_last_error() -> *const c_char {
    match LAST_ERROR.lock() {
        Ok(slot) => slot
            .as_ref()
            .map_or(ptr::null(), |error_message| error_message.as_ptr()),
        Err(_) => ptr::null(),
    }
}

/// Frees a heap-allocated C string returned by this library.
///
/// # Safety
/// `ptr` must be a pointer previously returned by this library via `CString::into_raw`,
/// and it must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn hnsw_toolbox_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    let _ = CString::from_raw(ptr);
}

/// Executes persisted HNSW extraction into Arrow IPC or Parquet.
///
/// # Safety
/// `request_json` must be a valid, null-terminated UTF-8 JSON string for the duration of this
/// call.
#[no_mangle]
pub unsafe extern "C" fn hnsw_toolbox_extract_index(request_json: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| extract_index_impl(request_json));
    match result {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(error_message)) => {
            set_last_error_message(error_message);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error_message("panic during hnsw_toolbox_extract_index");
            ptr::null_mut()
        }
    }
}

/// Builds a new HNSW index from a columnar Arrow IPC or Parquet export.
///
/// # Safety
/// `request_json` must be a valid, null-terminated UTF-8 JSON string for the duration of this
/// call.
#[no_mangle]
pub unsafe extern "C" fn hnsw_toolbox_build_index(request_json: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| build_index_impl(request_json));
    match result {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(error_message)) => {
            set_last_error_message(error_message);
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error_message("panic during hnsw_toolbox_build_index");
            ptr::null_mut()
        }
    }
}

unsafe fn extract_index_impl(request_json: *const c_char) -> Result<*mut c_char, String> {
    if request_json.is_null() {
        return Err("request_json is null".to_string());
    }

    let request_json = CStr::from_ptr(request_json)
        .to_str()
        .map_err(|e| format!("request_json is not valid UTF-8: {e}"))?;

    let request: ExtractRequest = serde_json::from_str(request_json)
        .map_err(|e| format!("request_json is not valid JSON: {e}"))?;

    if request.index_dir.trim().is_empty() {
        return Err("index_dir is required".to_string());
    }
    if request.output_path.trim().is_empty() {
        return Err("output_path is required".to_string());
    }

    let metadata = if let Some(metadata_path) = request.metadata_path.as_ref() {
        Some(
            load_chroma_metadata(Path::new(metadata_path))
                .map_err(|e| format!("failed to load metadata_path {}: {e}", metadata_path))?,
        )
    } else {
        None
    };

    let options = ExtractOptions {
        include_deleted: request.include_deleted.unwrap_or(false),
        metadata,
    };

    let output_format = request.output_format.unwrap_or_default();
    let summary = extract_index_to_columnar(
        Path::new(&request.index_dir),
        Path::new(&request.output_path),
        output_format,
        &options,
        request.batch_size.unwrap_or(1024),
    )
    .map_err(|e| e.to_string())?;

    let response = ExtractResponse {
        output_path: request.output_path,
        output_format,
        summary,
    };
    let response_json = serde_json::to_string(&response).map_err(|e| e.to_string())?;

    clear_last_error();
    CString::new(response_json)
        .map(CString::into_raw)
        .map_err(|e| format!("failed to build CString response: {e}"))
}

unsafe fn build_index_impl(request_json: *const c_char) -> Result<*mut c_char, String> {
    if request_json.is_null() {
        return Err("request_json is null".to_string());
    }

    let request_json = CStr::from_ptr(request_json)
        .to_str()
        .map_err(|e| format!("request_json is not valid UTF-8: {e}"))?;

    let request: BuildRequest = serde_json::from_str(request_json)
        .map_err(|e| format!("request_json is not valid JSON: {e}"))?;

    if request.input_path.trim().is_empty() {
        return Err("input_path is required".to_string());
    }
    if request.output_path.trim().is_empty() {
        return Err("output_path is required".to_string());
    }

    let input_format = request.input_format.unwrap_or_default();
    let metric = request.metric.unwrap_or_default();
    let options = BuildOptions {
        include_deleted: request.include_deleted.unwrap_or(false),
        input_format,
        metric,
        m: request.m.unwrap_or(16),
        m0: request.m0,
        ef_construction: request.ef_construction.unwrap_or(200),
        batch_size: request.batch_size.unwrap_or(1024),
        capacity: request.capacity,
        seed: request.seed,
    };

    let summary = build_index_from_columnar(
        Path::new(&request.input_path),
        Path::new(&request.output_path),
        &options,
    )
    .map_err(|e| e.to_string())?;

    let response = BuildResponse {
        input_path: request.input_path,
        output_path: request.output_path,
        input_format,
        metric,
        summary,
    };
    let response_json = serde_json::to_string(&response).map_err(|e| e.to_string())?;

    clear_last_error();
    CString::new(response_json)
        .map(CString::into_raw)
        .map_err(|e| format!("failed to build CString response: {e}"))
}

fn clear_last_error() {
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = None;
    }
}

fn set_last_error_message(message: impl Into<String>) {
    let mut message = message.into();
    if message.is_empty() {
        message = "unknown error".to_string();
    }
    let sanitized = message.replace('\0', "\\0");
    let c_error = CString::new(sanitized).unwrap_or_else(|_| {
        CString::new("unknown error")
            .expect("CString::new on constant without interior nulls should not fail")
    });
    if let Ok(mut slot) = LAST_ERROR.lock() {
        *slot = Some(c_error);
    }
}
