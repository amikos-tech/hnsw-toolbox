use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, FixedSizeListBuilder, Float32Builder, StringBuilder, UInt64Builder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_ipc::writer::FileWriter as ArrowIpcFileWriter;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use serde::{Deserialize, Serialize};

use crate::error::ExtractError;
use crate::header::{HeaderWordSize, PersistentHeader};
use crate::metadata::MetadataEntry;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    ArrowIpc,
    #[default]
    Parquet,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    pub include_deleted: bool,
    pub metadata: Option<HashMap<u64, MetadataEntry>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractIndexProperties {
    pub m: u64,
    pub ef_construction: u64,
    pub cur_element_count: u64,
    pub max_elements: u64,
    pub persisted_version: i32,
    pub word_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractSummary {
    pub scanned: u64,
    pub emitted: u64,
    pub deleted_skipped: u64,
    pub dimension: usize,
    pub index_properties: ExtractIndexProperties,
}

#[derive(Debug, Clone)]
pub struct ExtractedRecord {
    pub internal_id: u64,
    pub label: u64,
    pub deleted: bool,
    pub vector: Vec<f32>,
    pub user_id: Option<String>,
    pub seq_id: Option<u64>,
}

pub fn extract_index<F>(
    index_dir: &Path,
    options: &ExtractOptions,
    mut on_record: F,
) -> Result<ExtractSummary, ExtractError>
where
    F: FnMut(ExtractedRecord) -> Result<(), ExtractError>,
{
    let paths = IndexPaths::resolve(index_dir)?;
    let header = PersistentHeader::from_path(&paths.header_path)?;
    extract_index_with_header(&header, &paths.data_level0_path, options, &mut on_record)
}

pub fn extract_index_to_columnar(
    index_dir: &Path,
    output_path: &Path,
    output_format: OutputFormat,
    options: &ExtractOptions,
    batch_size: usize,
) -> Result<ExtractSummary, ExtractError> {
    let paths = IndexPaths::resolve(index_dir)?;
    let header = PersistentHeader::from_path(&paths.header_path)?;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let schema = build_schema(header.dimension());
    let mut writer = ColumnarWriter::new(output_path, schema, output_format, batch_size.max(1))?;
    let summary =
        extract_index_with_header(&header, &paths.data_level0_path, options, &mut |record| {
            writer.append_record(record)
        })?;
    writer.finish()?;
    Ok(summary)
}

fn extract_index_with_header<F>(
    header: &PersistentHeader,
    data_level0_path: &Path,
    options: &ExtractOptions,
    on_record: &mut F,
) -> Result<ExtractSummary, ExtractError>
where
    F: FnMut(ExtractedRecord) -> Result<(), ExtractError>,
{
    let record_size = usize::try_from(header.size_data_per_element).map_err(|_| {
        ExtractError::CorruptIndex(format!(
            "record size {} cannot fit into usize",
            header.size_data_per_element
        ))
    })?;

    let metadata = std::fs::metadata(data_level0_path)?;
    let min_required = header
        .cur_element_count
        .checked_mul(header.size_data_per_element)
        .ok_or_else(|| {
            ExtractError::CorruptIndex(
                "cur_element_count * size_data_per_element overflows u64".to_string(),
            )
        })?;

    if metadata.len() < min_required {
        return Err(ExtractError::CorruptIndex(format!(
            "data_level0.bin too small: {} bytes found, need at least {}",
            metadata.len(),
            min_required
        )));
    }

    let file = File::open(data_level0_path)?;
    let mut reader = BufReader::with_capacity(record_size.saturating_mul(8).max(8 * 1024), file);
    let mut raw_record = vec![0_u8; record_size];

    let mut summary = ExtractSummary {
        scanned: 0,
        emitted: 0,
        deleted_skipped: 0,
        dimension: header.dimension(),
        index_properties: ExtractIndexProperties {
            m: header.m,
            ef_construction: header.ef_construction,
            cur_element_count: header.cur_element_count,
            max_elements: header.max_elements,
            persisted_version: header.persisted_version,
            word_size_bytes: header.word_size.bytes(),
        },
    };

    let delete_marker_offset = header.delete_marker_offset();
    let vector_start = header.offset_data as usize;
    let vector_end = header.label_offset as usize;
    let label_offset = header.label_offset as usize;

    for internal_id in 0..header.cur_element_count {
        reader.read_exact(&mut raw_record)?;
        summary.scanned = summary.scanned.saturating_add(1);

        let deleted = (raw_record[delete_marker_offset] & 0x01) != 0;
        if deleted && !options.include_deleted {
            summary.deleted_skipped = summary.deleted_skipped.saturating_add(1);
            continue;
        }

        let label = parse_label(raw_record.as_slice(), label_offset, header.word_size)?;
        let vector = parse_vector(raw_record.as_slice(), vector_start, vector_end)?;

        let metadata_entry = options
            .metadata
            .as_ref()
            .and_then(|m| m.get(&label).cloned());
        let record = ExtractedRecord {
            internal_id,
            label,
            deleted,
            vector,
            user_id: metadata_entry.as_ref().map(|m| m.user_id.clone()),
            seq_id: metadata_entry.and_then(|m| m.seq_id),
        };
        on_record(record)?;
        summary.emitted = summary.emitted.saturating_add(1);
    }
    Ok(summary)
}

fn parse_label(
    raw_record: &[u8],
    label_offset: usize,
    word_size: HeaderWordSize,
) -> Result<u64, ExtractError> {
    match word_size {
        HeaderWordSize::U32 => {
            let bytes = raw_record
                .get(label_offset..label_offset + 4)
                .ok_or_else(|| {
                    ExtractError::CorruptIndex("label field exceeds record bounds".into())
                })?;
            let mut tmp = [0_u8; 4];
            tmp.copy_from_slice(bytes);
            Ok(u32::from_le_bytes(tmp) as u64)
        }
        HeaderWordSize::U64 => {
            let bytes = raw_record
                .get(label_offset..label_offset + 8)
                .ok_or_else(|| {
                    ExtractError::CorruptIndex("label field exceeds record bounds".into())
                })?;
            let mut tmp = [0_u8; 8];
            tmp.copy_from_slice(bytes);
            Ok(u64::from_le_bytes(tmp))
        }
    }
}

fn parse_vector(
    raw_record: &[u8],
    vector_start: usize,
    vector_end: usize,
) -> Result<Vec<f32>, ExtractError> {
    let vector_bytes = raw_record
        .get(vector_start..vector_end)
        .ok_or_else(|| ExtractError::CorruptIndex("vector field exceeds record bounds".into()))?;

    if vector_bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(ExtractError::CorruptIndex(format!(
            "vector byte count {} is not divisible by 4",
            vector_bytes.len()
        )));
    }

    let mut vector = Vec::with_capacity(vector_bytes.len() / 4);
    for chunk in vector_bytes.chunks_exact(4) {
        // Decode raw lanes directly to preserve exact f32 bit patterns
        // (including signed zero and NaN payloads).
        let mut tmp = [0_u8; 4];
        tmp.copy_from_slice(chunk);
        vector.push(f32::from_le_bytes(tmp));
    }
    Ok(vector)
}

fn build_schema(vector_dim: usize) -> SchemaRef {
    let fields = vec![
        Field::new("internal_id", DataType::UInt64, false),
        Field::new("label", DataType::UInt64, false),
        Field::new("deleted", DataType::Boolean, false),
        Field::new("user_id", DataType::Utf8, true),
        Field::new("seq_id", DataType::UInt64, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                vector_dim as i32,
            ),
            false,
        ),
    ];
    Arc::new(Schema::new(fields))
}

struct IndexPaths {
    header_path: PathBuf,
    data_level0_path: PathBuf,
}

impl IndexPaths {
    fn resolve(index_dir: &Path) -> Result<Self, ExtractError> {
        let header_path = index_dir.join("header.bin");
        let data_level0_path = index_dir.join("data_level0.bin");

        if !header_path.exists() {
            return Err(ExtractError::MissingFile(header_path));
        }
        if !data_level0_path.exists() {
            return Err(ExtractError::MissingFile(data_level0_path));
        }
        Ok(Self {
            header_path,
            data_level0_path,
        })
    }
}

enum ColumnarSink {
    Parquet(ArrowWriter<BufWriter<File>>),
    ArrowIpc(ArrowIpcFileWriter<BufWriter<File>>),
}

impl ColumnarSink {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ExtractError> {
        match self {
            ColumnarSink::Parquet(writer) => writer.write(batch)?,
            ColumnarSink::ArrowIpc(writer) => writer.write(batch)?,
        }
        Ok(())
    }

    fn finish(self) -> Result<(), ExtractError> {
        match self {
            ColumnarSink::Parquet(writer) => {
                let _ = writer.close()?;
            }
            ColumnarSink::ArrowIpc(mut writer) => {
                writer.finish()?;
            }
        }
        Ok(())
    }
}

struct ColumnarWriter {
    schema: SchemaRef,
    sink: ColumnarSink,
    batch: BatchBuffer,
    batch_size: usize,
}

impl ColumnarWriter {
    fn new(
        output_path: &Path,
        schema: SchemaRef,
        output_format: OutputFormat,
        batch_size: usize,
    ) -> Result<Self, ExtractError> {
        let file = File::create(output_path)?;
        let writer = BufWriter::new(file);
        let sink = match output_format {
            OutputFormat::Parquet => {
                ColumnarSink::Parquet(ArrowWriter::try_new(writer, schema.clone(), None)?)
            }
            OutputFormat::ArrowIpc => {
                ColumnarSink::ArrowIpc(ArrowIpcFileWriter::try_new(writer, &schema)?)
            }
        };
        let vector_dim = schema
            .field_with_name("vector")
            .ok()
            .and_then(|field| match field.data_type() {
                DataType::FixedSizeList(_, n) => usize::try_from(*n).ok(),
                _ => None,
            })
            .ok_or_else(|| ExtractError::CorruptIndex("vector field missing from schema".into()))?;

        Ok(Self {
            schema,
            sink,
            batch: BatchBuffer::new(vector_dim),
            batch_size,
        })
    }

    fn append_record(&mut self, record: ExtractedRecord) -> Result<(), ExtractError> {
        self.batch.append(record)?;
        if self.batch.rows() >= self.batch_size {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ExtractError> {
        if self.batch.rows() == 0 {
            return Ok(());
        }
        let batch = self.batch.finish_batch(self.schema.clone())?;
        self.sink.write_batch(&batch)?;
        Ok(())
    }

    fn finish(mut self) -> Result<(), ExtractError> {
        self.flush()?;
        self.sink.finish()
    }
}

struct BatchBuffer {
    internal_id: UInt64Builder,
    label: UInt64Builder,
    deleted: BooleanBuilder,
    user_id: StringBuilder,
    seq_id: UInt64Builder,
    vector: FixedSizeListBuilder<Float32Builder>,
    vector_dim: usize,
    rows: usize,
}

impl BatchBuffer {
    fn new(vector_dim: usize) -> Self {
        Self {
            internal_id: UInt64Builder::new(),
            label: UInt64Builder::new(),
            deleted: BooleanBuilder::new(),
            user_id: StringBuilder::new(),
            seq_id: UInt64Builder::new(),
            vector: FixedSizeListBuilder::new(Float32Builder::new(), vector_dim as i32),
            vector_dim,
            rows: 0,
        }
    }

    fn append(&mut self, record: ExtractedRecord) -> Result<(), ExtractError> {
        if record.vector.len() != self.vector_dim {
            return Err(ExtractError::CorruptIndex(format!(
                "record vector dimension {} does not match expected {}",
                record.vector.len(),
                self.vector_dim
            )));
        }

        self.internal_id.append_value(record.internal_id);
        self.label.append_value(record.label);
        self.deleted.append_value(record.deleted);

        match record.user_id {
            Some(user_id) => self.user_id.append_value(user_id),
            None => self.user_id.append_null(),
        }
        match record.seq_id {
            Some(seq_id) => self.seq_id.append_value(seq_id),
            None => self.seq_id.append_null(),
        }

        self.vector.values().append_slice(&record.vector);
        self.vector.append(true);

        self.rows += 1;
        Ok(())
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn finish_batch(&mut self, schema: SchemaRef) -> Result<RecordBatch, ExtractError> {
        let mut internal_id_builder =
            std::mem::replace(&mut self.internal_id, UInt64Builder::new());
        let mut label_builder = std::mem::replace(&mut self.label, UInt64Builder::new());
        let mut deleted_builder = std::mem::replace(&mut self.deleted, BooleanBuilder::new());
        let mut user_id_builder = std::mem::replace(&mut self.user_id, StringBuilder::new());
        let mut seq_id_builder = std::mem::replace(&mut self.seq_id, UInt64Builder::new());
        let mut vector_builder = std::mem::replace(
            &mut self.vector,
            FixedSizeListBuilder::new(Float32Builder::new(), self.vector_dim as i32),
        );

        let internal_id = Arc::new(internal_id_builder.finish()) as ArrayRef;
        let label = Arc::new(label_builder.finish()) as ArrayRef;
        let deleted = Arc::new(deleted_builder.finish()) as ArrayRef;
        let user_id = Arc::new(user_id_builder.finish()) as ArrayRef;
        let seq_id = Arc::new(seq_id_builder.finish()) as ArrayRef;
        let vector = Arc::new(vector_builder.finish()) as ArrayRef;

        self.internal_id = UInt64Builder::new();
        self.label = UInt64Builder::new();
        self.deleted = BooleanBuilder::new();
        self.user_id = StringBuilder::new();
        self.seq_id = UInt64Builder::new();
        self.vector = FixedSizeListBuilder::new(Float32Builder::new(), self.vector_dim as i32);
        self.rows = 0;

        Ok(RecordBatch::try_new(
            schema,
            vec![internal_id, label, deleted, user_id, seq_id, vector],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;

    use arrow_array::{BooleanArray, FixedSizeListArray, Float32Array, UInt64Array};
    use arrow_ipc::reader::FileReader as ArrowIpcFileReader;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use proptest::prelude::*;
    use serde::Serialize;
    use tempfile::tempdir;

    use super::*;
    use crate::metadata::load_chroma_metadata;

    #[derive(Clone)]
    struct FixtureRecord {
        label: u64,
        vector: Vec<f32>,
        deleted: bool,
    }

    #[test]
    fn extract_skips_deleted_by_default() {
        let tmp = tempdir().expect("tempdir");
        let records = vec![
            FixtureRecord {
                label: 10,
                vector: vec![1.0, 2.0, 3.0],
                deleted: false,
            },
            FixtureRecord {
                label: 20,
                vector: vec![4.0, 5.0, 6.0],
                deleted: true,
            },
            FixtureRecord {
                label: 30,
                vector: vec![7.0, 8.0, 9.0],
                deleted: false,
            },
        ];
        write_fixture(tmp.path(), &records).expect("fixture");

        let mut out = Vec::new();
        let summary = extract_index(tmp.path(), &ExtractOptions::default(), |record| {
            out.push(record);
            Ok(())
        })
        .expect("extract");

        assert_eq!(summary.scanned, 3);
        assert_eq!(summary.emitted, 2);
        assert_eq!(summary.deleted_skipped, 1);
        assert_eq!(summary.dimension, 3);
        assert_eq!(summary.index_properties.m, 4);
        assert_eq!(summary.index_properties.ef_construction, 100);
        assert_eq!(summary.index_properties.cur_element_count, 3);
        assert_eq!(summary.index_properties.max_elements, 3);
        assert_eq!(summary.index_properties.persisted_version, 1);
        assert_eq!(summary.index_properties.word_size_bytes, 8);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label, 10);
        assert_eq!(out[1].label, 30);
    }

    #[test]
    fn extract_includes_deleted_when_enabled() {
        let tmp = tempdir().expect("tempdir");
        let records = vec![
            FixtureRecord {
                label: 1,
                vector: vec![1.0, 1.0],
                deleted: false,
            },
            FixtureRecord {
                label: 2,
                vector: vec![2.0, 2.0],
                deleted: true,
            },
        ];
        write_fixture(tmp.path(), &records).expect("fixture");

        let mut out = Vec::new();
        let summary = extract_index(
            tmp.path(),
            &ExtractOptions {
                include_deleted: true,
                metadata: None,
            },
            |record| {
                out.push(record);
                Ok(())
            },
        )
        .expect("extract");

        assert_eq!(summary.scanned, 2);
        assert_eq!(summary.emitted, 2);
        assert_eq!(summary.deleted_skipped, 0);
        assert_eq!(out.len(), 2);
        assert!(out[1].deleted);
    }

    #[test]
    fn extract_joins_metadata() {
        let tmp = tempdir().expect("tempdir");
        let records = vec![
            FixtureRecord {
                label: 101,
                vector: vec![1.0, 2.0],
                deleted: false,
            },
            FixtureRecord {
                label: 202,
                vector: vec![3.0, 4.0],
                deleted: false,
            },
        ];
        write_fixture(tmp.path(), &records).expect("fixture");
        let metadata_path = tmp.path().join("index_metadata.pickle");
        write_metadata_pickle(&metadata_path).expect("metadata");
        let metadata = load_chroma_metadata(&metadata_path).expect("load metadata");

        let mut out = Vec::new();
        extract_index(
            tmp.path(),
            &ExtractOptions {
                include_deleted: false,
                metadata: Some(metadata),
            },
            |record| {
                out.push(record);
                Ok(())
            },
        )
        .expect("extract");

        assert_eq!(out[0].user_id.as_deref(), Some("doc-101"));
        assert_eq!(out[0].seq_id, Some(11));
        assert_eq!(out[1].user_id.as_deref(), Some("doc-202"));
        assert_eq!(out[1].seq_id, Some(22));
    }

    #[test]
    fn writes_parquet_and_arrow_ipc() {
        let tmp = tempdir().expect("tempdir");
        let records = vec![FixtureRecord {
            label: 7,
            vector: vec![1.0, 2.0, 3.0, 4.0],
            deleted: false,
        }];
        write_fixture(tmp.path(), &records).expect("fixture");

        let parquet_path = tmp.path().join("out.parquet");
        let arrow_path = tmp.path().join("out.arrow");

        let summary_parquet = extract_index_to_columnar(
            tmp.path(),
            &parquet_path,
            OutputFormat::Parquet,
            &ExtractOptions::default(),
            16,
        )
        .expect("parquet");
        let summary_arrow = extract_index_to_columnar(
            tmp.path(),
            &arrow_path,
            OutputFormat::ArrowIpc,
            &ExtractOptions::default(),
            16,
        )
        .expect("arrow");

        assert_eq!(summary_parquet.emitted, 1);
        assert_eq!(summary_arrow.emitted, 1);
        assert!(
            std::fs::metadata(parquet_path)
                .expect("parquet metadata")
                .len()
                > 0
        );
        assert!(std::fs::metadata(arrow_path).expect("arrow metadata").len() > 0);
    }

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
            let tmp = tempdir().expect("tempdir");
            let dimension = rows.first().map(|(v, _)| v.len()).expect("rows must not be empty");
            let fixture_records: Vec<FixtureRecord> = rows
                .into_iter()
                .enumerate()
                .map(|(idx, (vector_bits, deleted))| {
                    let mut vector: Vec<f32> = vector_bits.into_iter().map(f32::from_bits).collect();
                    // Force a stable fixed dimension per generated case.
                    if vector.len() > dimension {
                        vector.truncate(dimension);
                    } else if vector.len() < dimension {
                        vector.resize(dimension, 0.0);
                    }
                    FixtureRecord {
                        label: (idx as u64) + 1,
                        vector,
                        deleted,
                    }
                })
                .collect();
            let capacity = count + extra_capacity;
            write_fixture_with_capacity(tmp.path(), &fixture_records, capacity).expect("fixture");

            let mut exported_without_deleted = Vec::new();
            let summary_without_deleted = extract_index(
                tmp.path(),
                &ExtractOptions::default(),
                |record| {
                    exported_without_deleted.push(record);
                    Ok(())
                },
            )
            .expect("extract without deleted");

            let expected_non_deleted = fixture_records.iter().filter(|r| !r.deleted).count() as u64;
            prop_assert_eq!(summary_without_deleted.scanned, fixture_records.len() as u64);
            prop_assert_eq!(summary_without_deleted.emitted, expected_non_deleted);
            prop_assert_eq!(
                summary_without_deleted.deleted_skipped,
                fixture_records.len() as u64 - expected_non_deleted
            );
            prop_assert_eq!(summary_without_deleted.index_properties.m, 4);
            prop_assert_eq!(summary_without_deleted.index_properties.ef_construction, 100);
            prop_assert_eq!(
                summary_without_deleted.index_properties.cur_element_count,
                fixture_records.len() as u64
            );
            prop_assert_eq!(
                summary_without_deleted.index_properties.max_elements,
                capacity as u64
            );
            prop_assert_eq!(summary_without_deleted.index_properties.persisted_version, 1);
            prop_assert_eq!(summary_without_deleted.index_properties.word_size_bytes, 8);

            for exported in &exported_without_deleted {
                let source = &fixture_records[exported.internal_id as usize];
                prop_assert!(!source.deleted);
                prop_assert_eq!(exported.label, source.label);
                prop_assert_eq!(f32_bits(&exported.vector), f32_bits(&source.vector));
                prop_assert!(!exported.deleted);
            }

            let mut exported_with_deleted = Vec::new();
            let summary_with_deleted = extract_index(
                tmp.path(),
                &ExtractOptions {
                    include_deleted: true,
                    metadata: None,
                },
                |record| {
                    exported_with_deleted.push(record);
                    Ok(())
                },
            )
            .expect("extract with deleted");

            prop_assert_eq!(summary_with_deleted.scanned, fixture_records.len() as u64);
            prop_assert_eq!(summary_with_deleted.emitted, fixture_records.len() as u64);
            prop_assert_eq!(summary_with_deleted.deleted_skipped, 0);

            for exported in &exported_with_deleted {
                let source = &fixture_records[exported.internal_id as usize];
                prop_assert_eq!(exported.label, source.label);
                prop_assert_eq!(f32_bits(&exported.vector), f32_bits(&source.vector));
                prop_assert_eq!(exported.deleted, source.deleted);
            }
        }
    }

    #[test]
    fn columnar_exports_preserve_f32_bits_verbatim() {
        let tmp = tempdir().expect("tempdir");
        let records = vec![
            FixtureRecord {
                label: 10,
                vector: vec![
                    f32::from_bits(0x8000_0000), // -0.0
                    f32::from_bits(0x0000_0001), // smallest subnormal
                    f32::from_bits(0x7f80_0000), // +inf
                    f32::from_bits(0xff80_0000), // -inf
                    f32::from_bits(0x7fc0_0001), // NaN payload
                ],
                deleted: false,
            },
            FixtureRecord {
                label: 20,
                vector: vec![
                    f32::from_bits(0x3f80_0000), // 1.0
                    f32::from_bits(0xbf80_0000), // -1.0
                    f32::from_bits(0x7f7f_ffff), // max finite
                    f32::from_bits(0x0080_0000), // min normal
                    f32::from_bits(0x7fa0_0001), // signaling-ish NaN payload bits
                ],
                deleted: true,
            },
        ];
        write_fixture(tmp.path(), &records).expect("fixture");

        let parquet_path = tmp.path().join("verbatim.parquet");
        let arrow_path = tmp.path().join("verbatim.arrow");

        extract_index_to_columnar(
            tmp.path(),
            &parquet_path,
            OutputFormat::Parquet,
            &ExtractOptions {
                include_deleted: true,
                metadata: None,
            },
            16,
        )
        .expect("parquet export");
        extract_index_to_columnar(
            tmp.path(),
            &arrow_path,
            OutputFormat::ArrowIpc,
            &ExtractOptions {
                include_deleted: true,
                metadata: None,
            },
            16,
        )
        .expect("arrow export");

        let parquet_rows = read_parquet_export(&parquet_path).expect("read parquet");
        let arrow_rows = read_arrow_export(&arrow_path).expect("read arrow");

        assert_eq!(parquet_rows.len(), records.len());
        assert_eq!(arrow_rows.len(), records.len());

        for (idx, source) in records.iter().enumerate() {
            let expected_internal = idx as u64;

            let parquet = &parquet_rows[idx];
            assert_eq!(parquet.internal_id, expected_internal);
            assert_eq!(parquet.label, source.label);
            assert_eq!(parquet.deleted, source.deleted);
            assert_eq!(f32_bits(&parquet.vector), f32_bits(&source.vector));

            let arrow = &arrow_rows[idx];
            assert_eq!(arrow.internal_id, expected_internal);
            assert_eq!(arrow.label, source.label);
            assert_eq!(arrow.deleted, source.deleted);
            assert_eq!(f32_bits(&arrow.vector), f32_bits(&source.vector));
        }
    }

    #[derive(Debug)]
    struct ExportRow {
        internal_id: u64,
        label: u64,
        deleted: bool,
        vector: Vec<f32>,
    }

    fn read_parquet_export(path: &Path) -> Result<Vec<ExportRow>, ExtractError> {
        let file = File::open(path)?;
        let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
        let mut rows = Vec::new();
        for batch in &mut reader {
            rows.extend(read_export_batch(&batch?)?);
        }
        Ok(rows)
    }

    fn read_arrow_export(path: &Path) -> Result<Vec<ExportRow>, ExtractError> {
        let file = File::open(path)?;
        let mut reader = ArrowIpcFileReader::try_new(file, None)?;
        let mut rows = Vec::new();
        for batch in &mut reader {
            rows.extend(read_export_batch(&batch?)?);
        }
        Ok(rows)
    }

    fn read_export_batch(batch: &RecordBatch) -> Result<Vec<ExportRow>, ExtractError> {
        let schema = batch.schema();
        let internal_id_idx = schema
            .index_of("internal_id")
            .map_err(|_| ExtractError::CorruptIndex("missing internal_id column".to_string()))?;
        let label_idx = schema
            .index_of("label")
            .map_err(|_| ExtractError::CorruptIndex("missing label column".to_string()))?;
        let deleted_idx = schema
            .index_of("deleted")
            .map_err(|_| ExtractError::CorruptIndex("missing deleted column".to_string()))?;
        let vector_idx = schema
            .index_of("vector")
            .map_err(|_| ExtractError::CorruptIndex("missing vector column".to_string()))?;

        let internal_ids = batch
            .column(internal_id_idx)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| {
                ExtractError::CorruptIndex("internal_id column must be uint64".to_string())
            })?;
        let labels = batch
            .column(label_idx)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| ExtractError::CorruptIndex("label column must be uint64".to_string()))?;
        let deleted = batch
            .column(deleted_idx)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .ok_or_else(|| ExtractError::CorruptIndex("deleted column must be bool".to_string()))?;
        let vectors = batch
            .column(vector_idx)
            .as_any()
            .downcast_ref::<FixedSizeListArray>()
            .ok_or_else(|| {
                ExtractError::CorruptIndex("vector column must be fixed_size_list".to_string())
            })?;
        let values = vectors
            .values()
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| {
                ExtractError::CorruptIndex("vector values must be float32".to_string())
            })?;

        let dim = usize::try_from(vectors.value_length())
            .map_err(|_| ExtractError::CorruptIndex("invalid vector dimension".to_string()))?;
        let mut out = Vec::with_capacity(batch.num_rows());
        for row in 0..batch.num_rows() {
            let start = row * dim;
            let end = start + dim;
            let mut vector = Vec::with_capacity(dim);
            for idx in start..end {
                vector.push(values.value(idx));
            }
            out.push(ExportRow {
                internal_id: internal_ids.value(row),
                label: labels.value(row),
                deleted: deleted.value(row),
                vector,
            });
        }
        Ok(out)
    }

    fn f32_bits(values: &[f32]) -> Vec<u32> {
        values.iter().map(|v| v.to_bits()).collect()
    }

    fn write_fixture(dir: &Path, records: &[FixtureRecord]) -> Result<(), ExtractError> {
        write_fixture_with_capacity(dir, records, records.len())
    }

    fn write_fixture_with_capacity(
        dir: &Path,
        records: &[FixtureRecord],
        max_elements: usize,
    ) -> Result<(), ExtractError> {
        let dim = records
            .first()
            .map(|r| r.vector.len())
            .ok_or_else(|| ExtractError::CorruptIndex("records must not be empty".to_string()))?;
        for record in records {
            if record.vector.len() != dim {
                return Err(ExtractError::CorruptIndex(
                    "all records in fixture must share dimension".to_string(),
                ));
            }
        }
        if max_elements < records.len() {
            return Err(ExtractError::CorruptIndex(format!(
                "max_elements {} is smaller than record count {}",
                max_elements,
                records.len()
            )));
        }

        let max_m0: u64 = 16;
        let size_links_level0 = max_m0 * 4 + 4;
        let offset_data = size_links_level0;
        let label_offset = offset_data + (dim as u64 * 4);
        let size_data_per_element = label_offset + 8;
        let max_elements_u64 = max_elements as u64;
        let cur_elements = records.len() as u64;

        let header_path = dir.join("header.bin");
        let mut header = File::create(header_path)?;
        header.write_all(&1_i32.to_le_bytes())?;
        header.write_all(&0_u64.to_le_bytes())?;
        header.write_all(&max_elements_u64.to_le_bytes())?;
        header.write_all(&cur_elements.to_le_bytes())?;
        header.write_all(&size_data_per_element.to_le_bytes())?;
        header.write_all(&label_offset.to_le_bytes())?;
        header.write_all(&offset_data.to_le_bytes())?;
        header.write_all(&0_i32.to_le_bytes())?;
        header.write_all(&0_u32.to_le_bytes())?;
        header.write_all(&8_u64.to_le_bytes())?;
        header.write_all(&max_m0.to_le_bytes())?;
        header.write_all(&4_u64.to_le_bytes())?;
        header.write_all(&1.0_f64.to_le_bytes())?;
        header.write_all(&100_u64.to_le_bytes())?;
        header.flush()?;

        let data_path = dir.join("data_level0.bin");
        let mut data = File::create(data_path)?;
        for i in 0..max_elements {
            let mut row = vec![0_u8; size_data_per_element as usize];
            if let Some(record) = records.get(i) {
                if record.deleted {
                    row[2] |= 0x01;
                }
                let mut offset = offset_data as usize;
                for value in &record.vector {
                    row[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                    offset += 4;
                }
                row[label_offset as usize..label_offset as usize + 8]
                    .copy_from_slice(&record.label.to_le_bytes());
            }
            data.write_all(&row)?;
        }
        data.flush()?;
        Ok(())
    }

    #[derive(Serialize)]
    struct RawMetadataFixture {
        label_to_id: HashMap<u64, String>,
        id_to_seq_id: HashMap<String, u64>,
    }

    fn write_metadata_pickle(path: &Path) -> Result<(), ExtractError> {
        let mut label_to_id = HashMap::new();
        label_to_id.insert(101_u64, "doc-101".to_string());
        label_to_id.insert(202_u64, "doc-202".to_string());

        let mut id_to_seq_id = HashMap::new();
        id_to_seq_id.insert("doc-101".to_string(), 11_u64);
        id_to_seq_id.insert("doc-202".to_string(), 22_u64);

        let fixture = RawMetadataFixture {
            label_to_id,
            id_to_seq_id,
        };
        let bytes = serde_pickle::to_vec(&fixture, serde_pickle::ser::SerOptions::default())?;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}
