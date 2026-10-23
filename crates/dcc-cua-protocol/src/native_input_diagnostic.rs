//! Closed, content-free measurements of a rejected native input fence.
use serde::{Deserialize, Serialize};

pub const MAX_NATIVE_INPUT_DIAGNOSTIC_REGIONS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeInputFenceDiagnostic {
    pub schema_version: u8,
    #[serde(deserialize_with = "no_input_authority")]
    pub authorizes_input: bool,
    pub target_process_id: u32,
    pub target_window_handle: u64,
    pub point: Option<[i32; 2]>,
    pub unobscured: bool,
    pub point_proof_complete: bool,
    pub point_visible: Option<bool>,
    pub hit_window_handle: Option<u64>,
    pub hit_process_id: Option<u32>,
    pub hit_root_window_handle: Option<u64>,
    pub hit_is_target_or_child: Option<bool>,
    pub redacted_rectangles: Vec<[i32; 4]>,
    pub redaction_count: u32,
    pub redactions_complete: bool,
}

fn no_input_authority<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    if bool::deserialize(deserializer)? {
        return Err(serde::de::Error::custom(
            "native input diagnostics grant no authority",
        ));
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
