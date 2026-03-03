use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),

    #[error("pickle decode error: {0}")]
    Pickle(#[from] serde_pickle::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("invalid header: {0}")]
    InvalidHeader(String),

    #[error("unsupported host endianness; persisted hnsw format is native-endian and currently requires little-endian hosts")]
    UnsupportedEndianness,

    #[error("unsupported hnsw size_t width in header: {0} bytes")]
    UnsupportedWordSize(u8),

    #[error("persisted index is inconsistent: {0}")]
    CorruptIndex(String),

    #[error("metadata error: {0}")]
    Metadata(String),

    #[error("required file missing: {0}")]
    MissingFile(PathBuf),
}
