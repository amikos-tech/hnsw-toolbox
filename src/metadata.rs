use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::ExtractError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    pub user_id: String,
    pub seq_id: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawMetadataU64 {
    #[serde(default)]
    label_to_id: HashMap<u64, String>,
    #[serde(default)]
    id_to_seq_id: HashMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct RawMetadataI64 {
    #[serde(default)]
    label_to_id: HashMap<i64, String>,
    #[serde(default)]
    id_to_seq_id: HashMap<String, i64>,
}

pub fn load_chroma_metadata(path: &Path) -> Result<HashMap<u64, MetadataEntry>, ExtractError> {
    let bytes = fs::read(path)?;
    let options = serde_pickle::de::DeOptions::default();

    if let Ok(parsed) = serde_pickle::from_slice::<RawMetadataU64>(&bytes, options.clone()) {
        return Ok(join_metadata(parsed.label_to_id, parsed.id_to_seq_id));
    }

    let parsed_i64: RawMetadataI64 = serde_pickle::from_slice(&bytes, options)?;
    let mut label_to_id = HashMap::with_capacity(parsed_i64.label_to_id.len());
    for (label, user_id) in parsed_i64.label_to_id {
        if label < 0 {
            return Err(ExtractError::Metadata(format!(
                "label_to_id contains negative label {label}"
            )));
        }
        label_to_id.insert(label as u64, user_id);
    }

    let mut id_to_seq_id = HashMap::with_capacity(parsed_i64.id_to_seq_id.len());
    for (user_id, seq_id) in parsed_i64.id_to_seq_id {
        if seq_id < 0 {
            return Err(ExtractError::Metadata(format!(
                "id_to_seq_id contains negative seq_id {seq_id} for id {user_id}"
            )));
        }
        id_to_seq_id.insert(user_id, seq_id as u64);
    }

    Ok(join_metadata(label_to_id, id_to_seq_id))
}

fn join_metadata(
    label_to_id: HashMap<u64, String>,
    id_to_seq_id: HashMap<String, u64>,
) -> HashMap<u64, MetadataEntry> {
    let mut out = HashMap::with_capacity(label_to_id.len());
    for (label, user_id) in label_to_id {
        let seq_id = id_to_seq_id.get(&user_id).copied();
        out.insert(label, MetadataEntry { user_id, seq_id });
    }
    out
}
