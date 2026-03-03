use std::fs::File;
use std::path::Path;

use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, Float64Array, Int16Array, Int32Array,
    Int64Array, Int8Array, LargeListArray, ListArray, RecordBatch, UInt16Array, UInt32Array,
    UInt64Array, UInt8Array,
};
use arrow_ipc::reader::FileReader as ArrowIpcFileReader;
use arrow_schema::DataType;
use fast_hnsw::distance::{Cosine, Distance, DotProduct, Euclidean, Manhattan, SquaredEuclidean};
use fast_hnsw::labeled::LabeledIndex;
use fast_hnsw::Builder as HnswBuilder;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};

use crate::error::ExtractError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InputFormat {
    ArrowIpc,
    #[default]
    Parquet,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DistanceMetric {
    #[default]
    Euclidean,
    SquaredEuclidean,
    Cosine,
    DotProduct,
    Manhattan,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildSummary {
    pub scanned: u64,
    pub inserted: u64,
    pub deleted_skipped: u64,
    pub dimension: usize,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub include_deleted: bool,
    pub input_format: InputFormat,
    pub metric: DistanceMetric,
    pub m: usize,
    pub m0: Option<usize>,
    pub ef_construction: usize,
    pub batch_size: usize,
    pub capacity: Option<usize>,
    pub seed: Option<u64>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            include_deleted: false,
            input_format: InputFormat::Parquet,
            metric: DistanceMetric::Euclidean,
            m: 16,
            m0: None,
            ef_construction: 200,
            batch_size: 1024,
            capacity: None,
            seed: None,
        }
    }
}

pub fn build_index_from_columnar(
    input_path: &Path,
    output_path: &Path,
    options: &BuildOptions,
) -> Result<BuildSummary, ExtractError> {
    validate_options(options)?;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    match options.metric {
        DistanceMetric::Euclidean => build_with_metric(Euclidean, input_path, output_path, options),
        DistanceMetric::SquaredEuclidean => {
            build_with_metric(SquaredEuclidean, input_path, output_path, options)
        }
        DistanceMetric::Cosine => build_with_metric(Cosine, input_path, output_path, options),
        DistanceMetric::DotProduct => {
            build_with_metric(DotProduct, input_path, output_path, options)
        }
        DistanceMetric::Manhattan => build_with_metric(Manhattan, input_path, output_path, options),
    }
}

fn validate_options(options: &BuildOptions) -> Result<(), ExtractError> {
    if options.m < 2 {
        return Err(ExtractError::InvalidInput(format!(
            "m must be >= 2, got {}",
            options.m
        )));
    }
    if let Some(m0) = options.m0 {
        if m0 < 2 {
            return Err(ExtractError::InvalidInput(format!(
                "m0 must be >= 2, got {}",
                m0
            )));
        }
    }
    if options.ef_construction < options.m {
        return Err(ExtractError::InvalidInput(format!(
            "ef_construction {} must be >= m {}",
            options.ef_construction, options.m
        )));
    }
    if options.batch_size == 0 {
        return Err(ExtractError::InvalidInput(
            "batch_size must be >= 1".to_string(),
        ));
    }
    Ok(())
}

fn build_with_metric<D: Distance>(
    metric: D,
    input_path: &Path,
    output_path: &Path,
    options: &BuildOptions,
) -> Result<BuildSummary, ExtractError> {
    let mut builder = HnswBuilder::new()
        .m(options.m)
        .ef_construction(options.ef_construction)
        .capacity(options.capacity.unwrap_or_default());
    if let Some(m0) = options.m0 {
        builder = builder.m0(m0);
    }
    if let Some(seed) = options.seed {
        builder = builder.seed(seed);
    }

    let mut index: LabeledIndex<D, u64> = builder.build_labeled(metric);
    let mut state = BuildState::default();

    match options.input_format {
        InputFormat::Parquet => {
            let file = File::open(input_path)?;
            let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)?
                .with_batch_size(options.batch_size)
                .build()?;
            for maybe_batch in &mut reader {
                let batch = maybe_batch?;
                ingest_batch(&batch, options, &mut state, &mut index)?;
            }
        }
        InputFormat::ArrowIpc => {
            let file = File::open(input_path)?;
            let mut reader = ArrowIpcFileReader::try_new(file, None)?;
            for maybe_batch in &mut reader {
                let batch = maybe_batch?;
                ingest_batch(&batch, options, &mut state, &mut index)?;
            }
        }
    }

    index.save(output_path)?;
    Ok(state.into_summary())
}

#[derive(Debug, Default)]
struct BuildState {
    scanned: u64,
    inserted: u64,
    deleted_skipped: u64,
    expected_dim: Option<usize>,
    row_index: u64,
}

impl BuildState {
    fn into_summary(self) -> BuildSummary {
        BuildSummary {
            scanned: self.scanned,
            inserted: self.inserted,
            deleted_skipped: self.deleted_skipped,
            dimension: self.expected_dim.unwrap_or(0),
        }
    }
}

struct BatchColumns<'a> {
    vector: &'a dyn Array,
    label: Option<&'a dyn Array>,
    internal_id: Option<&'a dyn Array>,
    deleted: Option<&'a dyn Array>,
}

fn ingest_batch<D: Distance>(
    batch: &RecordBatch,
    options: &BuildOptions,
    state: &mut BuildState,
    index: &mut LabeledIndex<D, u64>,
) -> Result<(), ExtractError> {
    let columns = resolve_batch_columns(batch)?;
    for row in 0..batch.num_rows() {
        let row_id = state.row_index;
        state.row_index = state.row_index.saturating_add(1);
        state.scanned = state.scanned.saturating_add(1);

        let vector = read_vector(columns.vector, row, &mut state.expected_dim)?;
        let deleted = read_deleted(columns.deleted, row)?;
        let label = resolve_label(columns.label, columns.internal_id, row, row_id)?;

        if deleted && !options.include_deleted {
            state.deleted_skipped = state.deleted_skipped.saturating_add(1);
            continue;
        }

        index.insert(vector, label);
        state.inserted = state.inserted.saturating_add(1);
    }
    Ok(())
}

fn resolve_batch_columns(batch: &RecordBatch) -> Result<BatchColumns<'_>, ExtractError> {
    let schema = batch.schema();
    let vector_ix = schema
        .index_of("vector")
        .map_err(|_| ExtractError::InvalidInput("required column 'vector' is missing".into()))?;

    let label_ix = schema.index_of("label").ok();
    let internal_id_ix = schema.index_of("internal_id").ok();
    let deleted_ix = schema.index_of("deleted").ok();

    Ok(BatchColumns {
        vector: batch.column(vector_ix).as_ref(),
        label: label_ix.map(|ix| batch.column(ix).as_ref()),
        internal_id: internal_id_ix.map(|ix| batch.column(ix).as_ref()),
        deleted: deleted_ix.map(|ix| batch.column(ix).as_ref()),
    })
}

fn resolve_label(
    label_col: Option<&dyn Array>,
    internal_id_col: Option<&dyn Array>,
    row: usize,
    fallback_row_id: u64,
) -> Result<u64, ExtractError> {
    if let Some(col) = label_col {
        return read_u64(col, row, "label");
    }
    if let Some(col) = internal_id_col {
        return read_u64(col, row, "internal_id");
    }
    Ok(fallback_row_id)
}

fn read_deleted(col: Option<&dyn Array>, row: usize) -> Result<bool, ExtractError> {
    let Some(col) = col else {
        return Ok(false);
    };
    if let Some(array) = col.as_any().downcast_ref::<BooleanArray>() {
        if array.is_null(row) {
            return Err(ExtractError::InvalidInput(format!(
                "deleted[{row}] is null"
            )));
        }
        return Ok(array.value(row));
    }

    let numeric = read_u64(col, row, "deleted")?;
    Ok(numeric != 0)
}

fn read_u64(col: &dyn Array, row: usize, name: &str) -> Result<u64, ExtractError> {
    if let Some(array) = col.as_any().downcast_ref::<UInt64Array>() {
        return read_unsigned(array.is_null(row), array.value(row), row, name);
    }
    if let Some(array) = col.as_any().downcast_ref::<UInt32Array>() {
        return read_unsigned(array.is_null(row), array.value(row) as u64, row, name);
    }
    if let Some(array) = col.as_any().downcast_ref::<UInt16Array>() {
        return read_unsigned(array.is_null(row), array.value(row) as u64, row, name);
    }
    if let Some(array) = col.as_any().downcast_ref::<UInt8Array>() {
        return read_unsigned(array.is_null(row), array.value(row) as u64, row, name);
    }
    if let Some(array) = col.as_any().downcast_ref::<Int64Array>() {
        if array.is_null(row) {
            return Err(ExtractError::InvalidInput(format!("{name}[{row}] is null")));
        }
        let value = array.value(row);
        if value < 0 {
            return Err(ExtractError::InvalidInput(format!(
                "{name}[{row}] must be non-negative, got {value}"
            )));
        }
        return Ok(value as u64);
    }
    if let Some(array) = col.as_any().downcast_ref::<Int32Array>() {
        if array.is_null(row) {
            return Err(ExtractError::InvalidInput(format!("{name}[{row}] is null")));
        }
        let value = array.value(row);
        if value < 0 {
            return Err(ExtractError::InvalidInput(format!(
                "{name}[{row}] must be non-negative, got {value}"
            )));
        }
        return Ok(value as u64);
    }
    if let Some(array) = col.as_any().downcast_ref::<Int16Array>() {
        if array.is_null(row) {
            return Err(ExtractError::InvalidInput(format!("{name}[{row}] is null")));
        }
        let value = array.value(row);
        if value < 0 {
            return Err(ExtractError::InvalidInput(format!(
                "{name}[{row}] must be non-negative, got {value}"
            )));
        }
        return Ok(value as u64);
    }
    if let Some(array) = col.as_any().downcast_ref::<Int8Array>() {
        if array.is_null(row) {
            return Err(ExtractError::InvalidInput(format!("{name}[{row}] is null")));
        }
        let value = array.value(row);
        if value < 0 {
            return Err(ExtractError::InvalidInput(format!(
                "{name}[{row}] must be non-negative, got {value}"
            )));
        }
        return Ok(value as u64);
    }

    Err(ExtractError::InvalidInput(format!(
        "column '{name}' must be an integer type, got {:?}",
        col.data_type()
    )))
}

fn read_unsigned(is_null: bool, value: u64, row: usize, name: &str) -> Result<u64, ExtractError> {
    if is_null {
        return Err(ExtractError::InvalidInput(format!("{name}[{row}] is null")));
    }
    Ok(value)
}

fn read_vector(
    col: &dyn Array,
    row: usize,
    expected_dim: &mut Option<usize>,
) -> Result<Vec<f32>, ExtractError> {
    let vector = match col.data_type() {
        DataType::FixedSizeList(_, _) => {
            let array = col
                .as_any()
                .downcast_ref::<FixedSizeListArray>()
                .ok_or_else(|| {
                    ExtractError::InvalidInput(
                        "vector column type mismatch for fixed-size list".into(),
                    )
                })?;
            if array.is_null(row) {
                return Err(ExtractError::InvalidInput(format!("vector[{row}] is null")));
            }
            read_float_values(array.value(row).as_ref(), row)?
        }
        DataType::List(_) => {
            let array = col.as_any().downcast_ref::<ListArray>().ok_or_else(|| {
                ExtractError::InvalidInput("vector column type mismatch for list".into())
            })?;
            if array.is_null(row) {
                return Err(ExtractError::InvalidInput(format!("vector[{row}] is null")));
            }
            read_float_values(array.value(row).as_ref(), row)?
        }
        DataType::LargeList(_) => {
            let array = col
                .as_any()
                .downcast_ref::<LargeListArray>()
                .ok_or_else(|| {
                    ExtractError::InvalidInput("vector column type mismatch for large list".into())
                })?;
            if array.is_null(row) {
                return Err(ExtractError::InvalidInput(format!("vector[{row}] is null")));
            }
            read_float_values(array.value(row).as_ref(), row)?
        }
        other => {
            return Err(ExtractError::InvalidInput(format!(
                "column 'vector' must be fixed_size_list/list/large_list of floats, got {other:?}"
            )));
        }
    };

    if vector.is_empty() {
        return Err(ExtractError::InvalidInput(format!(
            "vector[{row}] is empty"
        )));
    }

    match expected_dim {
        Some(dim) if *dim != vector.len() => {
            return Err(ExtractError::InvalidInput(format!(
                "vector[{row}] dimension {} does not match expected {}",
                vector.len(),
                *dim
            )));
        }
        Some(_) => {}
        None => *expected_dim = Some(vector.len()),
    }

    Ok(vector)
}

fn read_float_values(values: &dyn Array, row: usize) -> Result<Vec<f32>, ExtractError> {
    if let Some(array) = values.as_any().downcast_ref::<Float32Array>() {
        let mut out = Vec::with_capacity(array.len());
        for i in 0..array.len() {
            if array.is_null(i) {
                return Err(ExtractError::InvalidInput(format!(
                    "vector[{row}] contains null at position {i}"
                )));
            }
            out.push(array.value(i));
        }
        return Ok(out);
    }

    if let Some(array) = values.as_any().downcast_ref::<Float64Array>() {
        let mut out = Vec::with_capacity(array.len());
        for i in 0..array.len() {
            if array.is_null(i) {
                return Err(ExtractError::InvalidInput(format!(
                    "vector[{row}] contains null at position {i}"
                )));
            }
            out.push(array.value(i) as f32);
        }
        return Ok(out);
    }

    Err(ExtractError::InvalidInput(format!(
        "vector[{row}] values must be float32/float64, got {:?}",
        values.data_type()
    )))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::builder::{
        BooleanBuilder, FixedSizeListBuilder, Float32Builder, UInt64Builder,
    };
    use arrow_array::{ArrayRef, RecordBatch, UInt64Array};
    use arrow_ipc::writer::FileWriter as ArrowIpcFileWriter;
    use arrow_schema::{DataType, Field, Schema};
    use fast_hnsw::distance::Euclidean;
    use parquet::arrow::ArrowWriter;
    use tempfile::tempdir;

    use super::*;

    fn fixture_batch() -> RecordBatch {
        let mut internal_id_builder = UInt64Builder::new();
        let mut label_builder = UInt64Builder::new();
        let mut deleted_builder = BooleanBuilder::new();
        let mut vector_builder = FixedSizeListBuilder::new(Float32Builder::new(), 2);

        let records = vec![
            (0_u64, 100_u64, false, vec![1.0_f32, 0.0]),
            (1_u64, 200_u64, true, vec![0.0_f32, 1.0]),
            (2_u64, 300_u64, false, vec![0.8_f32, 0.2]),
        ];

        for (internal_id, label, deleted, vector) in records {
            internal_id_builder.append_value(internal_id);
            label_builder.append_value(label);
            deleted_builder.append_value(deleted);
            vector_builder.values().append_slice(vector.as_slice());
            vector_builder.append(true);
        }

        let schema = Arc::new(Schema::new(vec![
            Field::new("internal_id", DataType::UInt64, false),
            Field::new("label", DataType::UInt64, false),
            Field::new("deleted", DataType::Boolean, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 2),
                false,
            ),
        ]));

        let arrays: Vec<ArrayRef> = vec![
            Arc::new(internal_id_builder.finish()) as ArrayRef,
            Arc::new(label_builder.finish()) as ArrayRef,
            Arc::new(deleted_builder.finish()) as ArrayRef,
            Arc::new(vector_builder.finish()) as ArrayRef,
        ];
        RecordBatch::try_new(schema, arrays).expect("fixture record batch")
    }

    #[test]
    fn builds_hnsw_from_parquet() {
        let temp = tempdir().expect("tempdir");
        let input_path = temp.path().join("export.parquet");
        let output_path = temp.path().join("rebuilt.hnsw");
        let batch = fixture_batch();

        {
            let file = File::create(&input_path).expect("create parquet");
            let mut writer =
                ArrowWriter::try_new(file, batch.schema(), None).expect("arrow parquet writer");
            writer.write(&batch).expect("write parquet batch");
            writer.close().expect("close parquet writer");
        }

        let summary = build_index_from_columnar(
            &input_path,
            &output_path,
            &BuildOptions {
                include_deleted: false,
                input_format: InputFormat::Parquet,
                metric: DistanceMetric::Euclidean,
                m: 8,
                m0: Some(16),
                ef_construction: 32,
                batch_size: 2,
                capacity: Some(8),
                seed: Some(42),
            },
        )
        .expect("build from parquet");

        assert_eq!(summary.scanned, 3);
        assert_eq!(summary.inserted, 2);
        assert_eq!(summary.deleted_skipped, 1);
        assert_eq!(summary.dimension, 2);
        assert!(output_path.exists());

        let loaded = LabeledIndex::<Euclidean, u64>::load(&output_path, Euclidean)
            .expect("load rebuilt index");
        assert_eq!(loaded.len(), 2);
        let hits = loaded.search(&[0.95, 0.05], 1, 20);
        assert_eq!(*hits[0].payload, 100);
    }

    #[test]
    fn builds_hnsw_from_arrow_ipc() {
        let temp = tempdir().expect("tempdir");
        let input_path = temp.path().join("export.arrow");
        let output_path = temp.path().join("rebuilt.hnsw");
        let batch = fixture_batch();

        {
            let file = File::create(&input_path).expect("create arrow file");
            let mut writer =
                ArrowIpcFileWriter::try_new(file, &batch.schema()).expect("arrow ipc writer");
            writer.write(&batch).expect("write arrow batch");
            writer.finish().expect("finish arrow writer");
        }

        let summary = build_index_from_columnar(
            &input_path,
            &output_path,
            &BuildOptions {
                include_deleted: true,
                input_format: InputFormat::ArrowIpc,
                metric: DistanceMetric::Euclidean,
                m: 8,
                m0: None,
                ef_construction: 32,
                batch_size: 2,
                capacity: Some(8),
                seed: Some(7),
            },
        )
        .expect("build from arrow");

        assert_eq!(summary.scanned, 3);
        assert_eq!(summary.inserted, 3);
        assert_eq!(summary.deleted_skipped, 0);
        assert_eq!(summary.dimension, 2);

        let loaded = LabeledIndex::<Euclidean, u64>::load(&output_path, Euclidean)
            .expect("load rebuilt index");
        assert_eq!(loaded.len(), 3);
        let hits = loaded.search(&[0.0, 1.0], 1, 20);
        assert_eq!(*hits[0].payload, 200);
    }

    #[test]
    fn falls_back_to_internal_id_when_label_missing() {
        let temp = tempdir().expect("tempdir");
        let input_path = temp.path().join("export.parquet");
        let output_path = temp.path().join("rebuilt.hnsw");
        let batch = fixture_batch();

        let schema = Arc::new(Schema::new(vec![
            Field::new("internal_id", DataType::UInt64, false),
            Field::new("deleted", DataType::Boolean, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 2),
                false,
            ),
        ]));
        let arrays: Vec<ArrayRef> = vec![
            batch.column(0).clone(),
            batch.column(2).clone(),
            batch.column(3).clone(),
        ];
        let rewritten = RecordBatch::try_new(schema, arrays).expect("rewritten batch");

        {
            let file = File::create(&input_path).expect("create parquet");
            let mut writer =
                ArrowWriter::try_new(file, rewritten.schema(), None).expect("parquet writer");
            writer.write(&rewritten).expect("write rewritten batch");
            writer.close().expect("close parquet writer");
        }

        build_index_from_columnar(
            &input_path,
            &output_path,
            &BuildOptions {
                include_deleted: true,
                input_format: InputFormat::Parquet,
                metric: DistanceMetric::Euclidean,
                m: 8,
                m0: None,
                ef_construction: 32,
                batch_size: 2,
                capacity: Some(8),
                seed: Some(1),
            },
        )
        .expect("build index");

        let loaded = LabeledIndex::<Euclidean, u64>::load(&output_path, Euclidean)
            .expect("load rebuilt index");
        assert_eq!(*loaded.get_payload(0), 0);
        assert_eq!(*loaded.get_payload(1), 1);
        assert_eq!(*loaded.get_payload(2), 2);

        let ids = UInt64Array::from(vec![0_u64, 1_u64, 2_u64]);
        assert_eq!(ids.len(), 3);
    }
}
