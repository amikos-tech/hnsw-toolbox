use std::fs;
use std::path::Path;

use crate::error::ExtractError;

pub const HNSW_PERSISTENCE_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum HeaderWordSize {
    U32,
    U64,
}

impl HeaderWordSize {
    pub fn bytes(self) -> usize {
        match self {
            HeaderWordSize::U32 => 4,
            HeaderWordSize::U64 => 8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PersistentHeader {
    pub persisted_version: i32,
    pub offset_level0: u64,
    pub max_elements: u64,
    pub cur_element_count: u64,
    pub size_data_per_element: u64,
    pub label_offset: u64,
    pub offset_data: u64,
    pub max_level: i32,
    pub enterpoint_node: u32,
    pub max_m: u64,
    pub max_m0: u64,
    pub m: u64,
    pub mult: f64,
    pub ef_construction: u64,
    pub word_size: HeaderWordSize,
}

impl PersistentHeader {
    pub fn from_path(path: &Path) -> Result<Self, ExtractError> {
        if !cfg!(target_endian = "little") {
            return Err(ExtractError::UnsupportedEndianness);
        }
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ExtractError> {
        if !cfg!(target_endian = "little") {
            return Err(ExtractError::UnsupportedEndianness);
        }
        match bytes.len() {
            100 => Self::parse_with_word_size(bytes, HeaderWordSize::U64),
            60 => Self::parse_with_word_size(bytes, HeaderWordSize::U32),
            other => Err(ExtractError::InvalidHeader(format!(
                "unexpected header length {other} (expected 100 for 64-bit or 60 for 32-bit)"
            ))),
        }
    }

    pub fn vector_byte_len(&self) -> usize {
        (self.label_offset - self.offset_data) as usize
    }

    pub fn dimension(&self) -> usize {
        self.vector_byte_len() / std::mem::size_of::<f32>()
    }

    pub fn label_width_bytes(&self) -> usize {
        self.word_size.bytes()
    }

    pub fn delete_marker_offset(&self) -> usize {
        (self.offset_level0 as usize) + 2
    }

    pub fn expected_data_level0_bytes(&self) -> Result<u64, ExtractError> {
        self.max_elements
            .checked_mul(self.size_data_per_element)
            .ok_or_else(|| {
                ExtractError::CorruptIndex(
                    "max_elements * size_data_per_element overflows u64".to_string(),
                )
            })
    }

    fn parse_with_word_size(bytes: &[u8], word_size: HeaderWordSize) -> Result<Self, ExtractError> {
        let mut cursor = HeaderCursor::new(bytes);

        let persisted_version = cursor.read_i32()?;
        let offset_level0 = cursor.read_word(word_size)?;
        let max_elements = cursor.read_word(word_size)?;
        let cur_element_count = cursor.read_word(word_size)?;
        let size_data_per_element = cursor.read_word(word_size)?;
        let label_offset = cursor.read_word(word_size)?;
        let offset_data = cursor.read_word(word_size)?;
        let max_level = cursor.read_i32()?;
        let enterpoint_node = cursor.read_u32()?;
        let max_m = cursor.read_word(word_size)?;
        let max_m0 = cursor.read_word(word_size)?;
        let m = cursor.read_word(word_size)?;
        let mult = cursor.read_f64()?;
        let ef_construction = cursor.read_word(word_size)?;
        cursor.ensure_eof()?;

        let header = PersistentHeader {
            persisted_version,
            offset_level0,
            max_elements,
            cur_element_count,
            size_data_per_element,
            label_offset,
            offset_data,
            max_level,
            enterpoint_node,
            max_m,
            max_m0,
            m,
            mult,
            ef_construction,
            word_size,
        };
        header.validate()?;
        Ok(header)
    }

    fn validate(&self) -> Result<(), ExtractError> {
        if self.persisted_version != HNSW_PERSISTENCE_VERSION {
            return Err(ExtractError::InvalidHeader(format!(
                "unsupported persistence version {}; expected {}",
                self.persisted_version, HNSW_PERSISTENCE_VERSION
            )));
        }
        if self.cur_element_count > self.max_elements {
            return Err(ExtractError::InvalidHeader(format!(
                "cur_element_count {} exceeds max_elements {}",
                self.cur_element_count, self.max_elements
            )));
        }
        if self.offset_data >= self.label_offset {
            return Err(ExtractError::InvalidHeader(format!(
                "offset_data {} must be smaller than label_offset {}",
                self.offset_data, self.label_offset
            )));
        }
        if self.label_offset + self.label_width_bytes() as u64 > self.size_data_per_element {
            return Err(ExtractError::InvalidHeader(format!(
                "label field exceeds record size: label_offset={}, label_width={}, size_data_per_element={}",
                self.label_offset,
                self.label_width_bytes(),
                self.size_data_per_element
            )));
        }
        let vector_bytes = self.label_offset - self.offset_data;
        if vector_bytes == 0 || !vector_bytes.is_multiple_of(std::mem::size_of::<f32>() as u64) {
            return Err(ExtractError::InvalidHeader(format!(
                "invalid vector payload size {}; must be a positive multiple of 4",
                vector_bytes
            )));
        }
        if self.delete_marker_offset() >= self.size_data_per_element as usize {
            return Err(ExtractError::InvalidHeader(format!(
                "delete marker offset {} is outside record bounds {}",
                self.delete_marker_offset(),
                self.size_data_per_element
            )));
        }
        Ok(())
    }
}

struct HeaderCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> HeaderCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_i32(&mut self) -> Result<i32, ExtractError> {
        let slice = self.take(4)?;
        let mut tmp = [0_u8; 4];
        tmp.copy_from_slice(slice);
        Ok(i32::from_le_bytes(tmp))
    }

    fn read_u32(&mut self) -> Result<u32, ExtractError> {
        let slice = self.take(4)?;
        let mut tmp = [0_u8; 4];
        tmp.copy_from_slice(slice);
        Ok(u32::from_le_bytes(tmp))
    }

    fn read_f64(&mut self) -> Result<f64, ExtractError> {
        let slice = self.take(8)?;
        let mut tmp = [0_u8; 8];
        tmp.copy_from_slice(slice);
        Ok(f64::from_le_bytes(tmp))
    }

    fn read_word(&mut self, word_size: HeaderWordSize) -> Result<u64, ExtractError> {
        match word_size {
            HeaderWordSize::U32 => Ok(self.read_u32()? as u64),
            HeaderWordSize::U64 => {
                let slice = self.take(8)?;
                let mut tmp = [0_u8; 8];
                tmp.copy_from_slice(slice);
                Ok(u64::from_le_bytes(tmp))
            }
        }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ExtractError> {
        if self.offset + len > self.bytes.len() {
            return Err(ExtractError::InvalidHeader(format!(
                "header ended early at offset {} (need {} more bytes)",
                self.offset, len
            )));
        }
        let out = &self.bytes[self.offset..self.offset + len];
        self.offset += len;
        Ok(out)
    }

    fn ensure_eof(&self) -> Result<(), ExtractError> {
        if self.offset == self.bytes.len() {
            return Ok(());
        }
        Err(ExtractError::InvalidHeader(format!(
            "header has {} trailing bytes",
            self.bytes.len() - self.offset
        )))
    }
}
