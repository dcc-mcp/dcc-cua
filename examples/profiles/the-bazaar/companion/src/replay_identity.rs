use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use flate2::read::GzDecoder;
use rmpv::Value as MessagePackValue;
use serde::de::{DeserializeOwned, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::TemplateIndex;

const MESSAGEPACK_CSHARP_LZ4_BLOCK_ARRAY_TYPE: i8 = 98;
const MAX_LZ4_BLOCKS: usize = 64;
const MAX_DESPAWN_BYTES: usize = 8 * 1024 * 1024;
const MAX_EVENTS: usize = 4096;
const MAX_CHOICE_IDS: usize = 32;
const MAX_REPLAY_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_REPLAY_MESSAGEPACK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REPLAY_FILES_TO_INSPECT: usize = 32;

fn default_replay_max_file_age_seconds() -> u64 {
    12 * 60 * 60
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CandidateIdentityProviderConfig {
    BppCombatReplay {
        #[serde(rename = "directoryPath")]
        directory_path: PathBuf,
        #[serde(
            rename = "maxFileAgeSeconds",
            default = "default_replay_max_file_age_seconds"
        )]
        max_file_age_seconds: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateIdentityFence {
    pub(crate) run_id: Option<String>,
    pub(crate) state_tick_id: u64,
    pub(crate) app_state: String,
    pub(crate) source_message_id: Option<String>,
    pub(crate) selection_instance_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateIdentityProvenance {
    pub provider: String,
    pub status: String,
    pub reason: String,
    pub source_path: Option<PathBuf>,
    pub source_battle_id: Option<String>,
    pub source_run_id: Option<String>,
    pub source_state_tick_id: Option<u64>,
    pub source_message_id: Option<String>,
    pub mapped_instance_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateIdentitySnapshot {
    mappings: BTreeMap<String, String>,
    source_fence: Option<CandidateIdentityFence>,
    pub(crate) provenance: CandidateIdentityProvenance,
}

impl CandidateIdentitySnapshot {
    pub(crate) fn disabled() -> Self {
        Self {
            mappings: BTreeMap::new(),
            source_fence: None,
            provenance: CandidateIdentityProvenance {
                provider: "none".into(),
                status: "disabled".into(),
                reason: "candidate_identity_provider_not_configured".into(),
                source_path: None,
                source_battle_id: None,
                source_run_id: None,
                source_state_tick_id: None,
                source_message_id: None,
                mapped_instance_ids: Vec::new(),
            },
        }
    }

    fn unavailable(
        fence: &CandidateIdentityFence,
        reason: &'static str,
        source_path: Option<PathBuf>,
        source_battle_id: Option<String>,
    ) -> Self {
        Self {
            mappings: BTreeMap::new(),
            source_fence: Some(fence.clone()),
            provenance: CandidateIdentityProvenance {
                provider: "bpp_combat_replay_v1".into(),
                status: "unavailable".into(),
                reason: reason.into(),
                source_path,
                source_battle_id,
                source_run_id: fence.run_id.clone(),
                source_state_tick_id: Some(fence.state_tick_id),
                source_message_id: fence.source_message_id.clone(),
                mapped_instance_ids: Vec::new(),
            },
        }
    }

    fn resolved(
        fence: &CandidateIdentityFence,
        source_path: PathBuf,
        source_battle_id: String,
        mappings: BTreeMap<String, String>,
    ) -> Self {
        let mapped_instance_ids = mappings.keys().cloned().collect();
        Self {
            mappings,
            source_fence: Some(fence.clone()),
            provenance: CandidateIdentityProvenance {
                provider: "bpp_combat_replay_v1".into(),
                status: "resolved".into(),
                reason: "exact_current_choice_fence_matched".into(),
                source_path: Some(source_path),
                source_battle_id: Some(source_battle_id),
                source_run_id: fence.run_id.clone(),
                source_state_tick_id: Some(fence.state_tick_id),
                source_message_id: fence.source_message_id.clone(),
                mapped_instance_ids,
            },
        }
    }

    pub(crate) fn template_id<'a>(
        &'a self,
        instance_id: &str,
        current_fence: &CandidateIdentityFence,
    ) -> Option<&'a str> {
        (self.provenance.status == "resolved" && self.source_fence.as_ref() == Some(current_fence))
            .then(|| self.mappings.get(instance_id).map(String::as_str))
            .flatten()
    }

    pub(crate) fn provenance_for(
        &self,
        current_fence: &CandidateIdentityFence,
    ) -> CandidateIdentityProvenance {
        if self.provenance.status != "resolved" || self.source_fence.as_ref() == Some(current_fence)
        {
            return self.provenance.clone();
        }
        let mut provenance = self.provenance.clone();
        provenance.status = "unavailable".into();
        provenance.reason = "candidate_identity_snapshot_fence_mismatch".into();
        provenance.mapped_instance_ids.clear();
        provenance
    }
}

trait CandidateIdentityProvider {
    fn resolve(
        &mut self,
        fence: &CandidateIdentityFence,
        index: &TemplateIndex,
    ) -> CandidateIdentitySnapshot;
}

pub(crate) struct CandidateIdentityResolver {
    provider: Option<Box<dyn CandidateIdentityProvider>>,
}

impl CandidateIdentityResolver {
    pub(crate) fn new(config: Option<&CandidateIdentityProviderConfig>) -> Self {
        let provider = config.map(|config| match config {
            CandidateIdentityProviderConfig::BppCombatReplay {
                directory_path,
                max_file_age_seconds,
            } => Box::new(BppCombatReplayIdentityProvider {
                directory_path: directory_path.clone(),
                max_file_age: Duration::from_secs(*max_file_age_seconds),
                replay_cache: BTreeMap::new(),
            }) as Box<dyn CandidateIdentityProvider>,
        });
        Self { provider }
    }

    pub(crate) fn resolve(
        &mut self,
        fence: &CandidateIdentityFence,
        index: &TemplateIndex,
    ) -> CandidateIdentitySnapshot {
        self.provider
            .as_mut()
            .map_or_else(CandidateIdentitySnapshot::disabled, |provider| {
                provider.resolve(fence, index)
            })
    }
}

struct BppCombatReplayIdentityProvider {
    directory_path: PathBuf,
    max_file_age: Duration,
    replay_cache: BTreeMap<PathBuf, CachedReplay>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReplayFileStamp {
    path: PathBuf,
    modified: SystemTime,
    length: u64,
}

struct CachedReplay {
    stamp: ReplayFileStamp,
    decoded: Option<DecodedReplay>,
}

impl BppCombatReplayIdentityProvider {
    fn decoded_replay(&mut self, stamp: &ReplayFileStamp) -> Option<DecodedReplay> {
        if let Some(cached) = self.replay_cache.get(&stamp.path)
            && cached.stamp == *stamp
        {
            return cached.decoded.clone();
        }

        let decoded = decode_replay_file(&stamp.path).ok();
        self.replay_cache.insert(
            stamp.path.clone(),
            CachedReplay {
                stamp: stamp.clone(),
                decoded: decoded.clone(),
            },
        );
        decoded
    }
}

impl CandidateIdentityProvider for BppCombatReplayIdentityProvider {
    fn resolve(
        &mut self,
        fence: &CandidateIdentityFence,
        index: &TemplateIndex,
    ) -> CandidateIdentitySnapshot {
        if fence.run_id.is_none()
            || fence.source_message_id.is_none()
            || fence.app_state != "ChoiceState"
            || fence.selection_instance_ids.is_empty()
            || fence
                .selection_instance_ids
                .iter()
                .any(|instance_id| !valid_instance_id(instance_id))
        {
            return CandidateIdentitySnapshot::unavailable(
                fence,
                "run_state_or_message_fence_unavailable",
                None,
                None,
            );
        }

        let files = match replay_files(&self.directory_path, self.max_file_age, SystemTime::now()) {
            Ok(files) => files,
            Err(reason) => {
                return CandidateIdentitySnapshot::unavailable(fence, reason, None, None);
            }
        };
        let current_stamps = files
            .iter()
            .map(|stamp| (&stamp.path, stamp))
            .collect::<BTreeMap<_, _>>();
        self.replay_cache.retain(|path, cached| {
            current_stamps
                .get(path)
                .is_some_and(|stamp| cached.stamp == **stamp)
        });
        let mut exact_matches = Vec::new();
        for stamp in files {
            let Some(replay) = self.decoded_replay(&stamp) else {
                continue;
            };
            if replay.despawn.message_id == fence.source_message_id.as_deref().unwrap_or_default()
                && replay.despawn.choice_instance_ids == fence.selection_instance_ids
            {
                exact_matches.push((stamp.path, replay));
            }
        }
        if exact_matches.is_empty() {
            return CandidateIdentitySnapshot::unavailable(
                fence,
                "no_replay_matches_current_choice_fence",
                None,
                None,
            );
        }
        if exact_matches.len() != 1 {
            return CandidateIdentitySnapshot::unavailable(
                fence,
                "ambiguous_replay_identity_match",
                None,
                None,
            );
        }
        let (path, replay) = exact_matches.pop().expect("one exact replay match");
        let mut mappings = BTreeMap::new();
        for instance_id in &fence.selection_instance_ids {
            let Some(template_id) = replay.despawn.instance_to_template.get(instance_id) else {
                return CandidateIdentitySnapshot::unavailable(
                    fence,
                    "replay_choice_mapping_incomplete",
                    Some(path),
                    Some(replay.battle_id),
                );
            };
            if index.get(template_id).is_none() {
                return CandidateIdentitySnapshot::unavailable(
                    fence,
                    "replay_template_missing_from_gamedata",
                    Some(path),
                    Some(replay.battle_id),
                );
            }
            mappings.insert(instance_id.clone(), template_id.clone());
        }
        CandidateIdentitySnapshot::resolved(fence, path, replay.battle_id, mappings)
    }
}

fn replay_files(
    directory: &Path,
    max_age: Duration,
    now: SystemTime,
) -> Result<Vec<ReplayFileStamp>, &'static str> {
    let entries = fs::read_dir(directory).map_err(|_| "replay_directory_unavailable")?;
    let mut files = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.ends_with(".payload.mpack.gz"))
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_REPLAY_FILE_BYTES {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age <= max_age {
            files.push(ReplayFileStamp {
                path,
                modified,
                length: metadata.len(),
            });
        }
    }
    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    files.truncate(MAX_REPLAY_FILES_TO_INSPECT);
    Ok(files)
}

#[derive(Clone)]
struct DecodedReplay {
    battle_id: String,
    despawn: DecodedDespawnIdentities,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PvpReplayPayloadV1 {
    battle_id: String,
    version: u8,
    #[serde(with = "serde_bytes")]
    despawn_message_bytes: Vec<u8>,
}

fn decode_replay_file(path: &Path) -> Result<DecodedReplay, ReplayIdentityError> {
    let metadata = path
        .metadata()
        .map_err(|_| ReplayIdentityError::InvalidReplayFile)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_REPLAY_FILE_BYTES {
        return Err(ReplayIdentityError::ReplayTooLarge);
    }
    let file = File::open(path).map_err(|_| ReplayIdentityError::InvalidReplayFile)?;
    let mut bytes = Vec::new();
    GzDecoder::new(file)
        .take(MAX_REPLAY_MESSAGEPACK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReplayIdentityError::InvalidReplayFile)?;
    if bytes.len() as u64 > MAX_REPLAY_MESSAGEPACK_BYTES {
        return Err(ReplayIdentityError::ReplayTooLarge);
    }
    let payload = deserialize_exact::<PvpReplayPayloadV1>(&bytes)
        .map_err(|()| ReplayIdentityError::InvalidReplayFile)?;
    if payload.version != 1
        || payload.battle_id.len() != 32
        || !payload
            .battle_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || path.file_name().and_then(|value| value.to_str())
            != Some(&format!("{}.payload.mpack.gz", payload.battle_id))
    {
        return Err(ReplayIdentityError::InvalidReplayFile);
    }
    let despawn = decode_despawn_identity_mappings(&payload.despawn_message_bytes)?;
    Ok(DecodedReplay {
        battle_id: payload.battle_id,
        despawn,
    })
}

#[derive(Debug, Error)]
enum ReplayIdentityError {
    #[error("invalid replay file")]
    InvalidReplayFile,
    #[error("invalid MessagePack-CSharp LZ4 envelope")]
    InvalidLz4Envelope,
    #[error("compressed replay exceeds the bounded decode contract")]
    ReplayTooLarge,
    #[error("LZ4 replay block failed to decompress")]
    Lz4Decompression,
    #[error("despawn MessagePack does not match the pinned v1 schema")]
    InvalidDespawnSchema,
    #[error("despawn replay contains an invalid identity")]
    InvalidIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DecodedDespawnIdentities {
    message_id: String,
    choice_instance_ids: Vec<String>,
    instance_to_template: BTreeMap<String, String>,
}

fn decode_despawn_identity_mappings(
    bytes: &[u8],
) -> Result<DecodedDespawnIdentities, ReplayIdentityError> {
    let decompressed = decode_lz4_block_array(bytes)?;
    decode_despawn_messagepack(&decompressed)
}

fn decode_despawn_messagepack(
    bytes: &[u8],
) -> Result<DecodedDespawnIdentities, ReplayIdentityError> {
    let message = deserialize_exact::<DespawnNetMessage>(bytes)
        .map_err(|()| ReplayIdentityError::InvalidDespawnSchema)?;
    let DespawnNetMessage(data, message_id) = message;
    let DespawnData(events, _, _, _, _, choice_state, _) = data;
    let PostCombatChoiceState(_, _, _, _, choice_instance_ids, _, _) = choice_state;

    if message_id.is_empty()
        || message_id.len() > 64
        || events.len() > MAX_EVENTS
        || choice_instance_ids.is_empty()
        || choice_instance_ids.len() > MAX_CHOICE_IDS
        || choice_instance_ids
            .iter()
            .any(|instance_id| !valid_instance_id(instance_id))
        || choice_instance_ids.iter().collect::<BTreeSet<_>>().len() != choice_instance_ids.len()
    {
        return Err(ReplayIdentityError::InvalidIdentity);
    }

    let mut instance_to_template = BTreeMap::new();
    for event in events {
        let Some(mapping) = event.mapping else {
            continue;
        };
        if !valid_instance_id(&mapping.instance_id)
            || !is_canonical_guid(&mapping.template_id)
            || instance_to_template
                .insert(mapping.instance_id, mapping.template_id)
                .is_some()
        {
            return Err(ReplayIdentityError::InvalidIdentity);
        }
    }

    Ok(DecodedDespawnIdentities {
        message_id,
        choice_instance_ids,
        instance_to_template,
    })
}

fn deserialize_exact<T>(bytes: &[u8]) -> Result<T, ()>
where
    T: DeserializeOwned,
{
    let mut deserializer = rmp_serde::Deserializer::new(Cursor::new(bytes));
    let value = T::deserialize(&mut deserializer).map_err(|_| ())?;
    if deserializer.position() != bytes.len() as u64 {
        return Err(());
    }
    Ok(value)
}

fn decode_lz4_block_array(bytes: &[u8]) -> Result<Vec<u8>, ReplayIdentityError> {
    if bytes.len() > MAX_DESPAWN_BYTES {
        return Err(ReplayIdentityError::ReplayTooLarge);
    }
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|_| ReplayIdentityError::InvalidLz4Envelope)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    }
    let MessagePackValue::Array(mut envelope) = value else {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    };
    if envelope.len() < 2 || envelope.len() - 1 > MAX_LZ4_BLOCKS {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    }
    let metadata = envelope.remove(0);
    let MessagePackValue::Ext(extension_type, metadata) = metadata else {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    };
    if extension_type != MESSAGEPACK_CSHARP_LZ4_BLOCK_ARRAY_TYPE {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    }
    let expected_lengths = decode_block_lengths(&metadata)?;
    if expected_lengths.len() != envelope.len() {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    }

    let total_length = expected_lengths
        .iter()
        .try_fold(0_usize, |total, length| total.checked_add(*length))
        .filter(|total| *total <= MAX_DESPAWN_BYTES)
        .ok_or(ReplayIdentityError::ReplayTooLarge)?;
    let mut decompressed = Vec::with_capacity(total_length);
    for (value, expected_length) in envelope.into_iter().zip(expected_lengths) {
        let MessagePackValue::Binary(compressed) = value else {
            return Err(ReplayIdentityError::InvalidLz4Envelope);
        };
        let block = lz4_flex::block::decompress(&compressed, expected_length)
            .map_err(|_| ReplayIdentityError::Lz4Decompression)?;
        decompressed.extend_from_slice(&block);
    }
    Ok(decompressed)
}

fn decode_block_lengths(metadata: &[u8]) -> Result<Vec<usize>, ReplayIdentityError> {
    let mut cursor = Cursor::new(metadata);
    let mut lengths = Vec::new();
    while cursor.position() < metadata.len() as u64 {
        if lengths.len() == MAX_LZ4_BLOCKS {
            return Err(ReplayIdentityError::InvalidLz4Envelope);
        }
        let value = rmpv::decode::read_value(&mut cursor)
            .map_err(|_| ReplayIdentityError::InvalidLz4Envelope)?;
        let length = value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0 && *value <= MAX_DESPAWN_BYTES)
            .ok_or(ReplayIdentityError::InvalidLz4Envelope)?;
        lengths.push(length);
    }
    if lengths.is_empty() {
        return Err(ReplayIdentityError::InvalidLz4Envelope);
    }
    Ok(lengths)
}

fn valid_instance_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn is_canonical_guid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|guid| guid.hyphenated().to_string() == value)
}

#[derive(Deserialize)]
struct DespawnNetMessage(DespawnData, String);

#[derive(Deserialize)]
struct DespawnData(
    Vec<ReplayEvent>,
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
    PostCombatChoiceState,
    IgnoredAny,
);

#[derive(Deserialize)]
struct PostCombatChoiceState(
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
    Vec<String>,
    IgnoredAny,
    IgnoredAny,
);

struct ReplayEvent {
    mapping: Option<ReplayIdentityMapping>,
}

struct ReplayIdentityMapping {
    instance_id: String,
    template_id: String,
}

impl<'de> Deserialize<'de> for ReplayEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ReplayEventVisitor)
    }
}

struct ReplayEventVisitor;

impl<'de> Visitor<'de> for ReplayEventVisitor {
    type Value = ReplayEvent;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a two-field Bazaar game-sim event union")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let tag = sequence
            .next_element::<u8>()?
            .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
        let mapping = match tag {
            // MessagePack union tags in the build-1.0.11894 NetMessageGameSim schema.
            1 => {
                let EncounterCardCreated(instance_id, template_id, _) = sequence
                    .next_element::<EncounterCardCreated>()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Some(ReplayIdentityMapping {
                    instance_id,
                    template_id,
                })
            }
            8 => {
                let ItemCardCreated(instance_id, template_id, _, _, _, _) = sequence
                    .next_element::<ItemCardCreated>()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                Some(ReplayIdentityMapping {
                    instance_id,
                    template_id,
                })
            }
            _ => {
                sequence
                    .next_element::<IgnoredAny>()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                None
            }
        };
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::invalid_length(3, &self));
        }
        Ok(ReplayEvent { mapping })
    }
}

#[derive(Deserialize)]
struct EncounterCardCreated(String, String, IgnoredAny);

#[derive(Deserialize)]
struct ItemCardCreated(
    String,
    String,
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
);

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write as _;

    use base64::Engine as _;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use serde::Serialize;

    use super::*;
    use crate::{CompanionConfig, RunModel, TemplateIndex, TemplateSummary};

    #[test]
    fn actual_bpp_v1_despawn_decodes_typed_choice_mappings() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../fixtures/day-3-choice-despawn.mpack.b64").trim())
            .expect("fixture is valid base64");

        let decoded = decode_despawn_identity_mappings(&bytes).expect("typed replay decode");

        assert_eq!(decoded.message_id, "H5B");
        assert_eq!(
            decoded.choice_instance_ids,
            ["enc_6PMgGlG", "enc_Kr4eMFa", "enc_W13jGim"]
        );
        assert_eq!(
            decoded.instance_to_template.get("enc_6PMgGlG"),
            Some(&"dc12f4bd-6c33-41f0-b7ad-d62a0be09a47".to_owned())
        );
        assert_eq!(
            decoded.instance_to_template.get("enc_Kr4eMFa"),
            Some(&"956e9c74-7d98-4a2b-b500-def577914945".to_owned())
        );
        assert_eq!(
            decoded.instance_to_template.get("enc_W13jGim"),
            Some(&"90e47f27-241c-4769-a6c6-a100d2a33421".to_owned())
        );
    }

    #[test]
    fn typed_despawn_decoder_rejects_trailing_messagepack_values() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../fixtures/day-3-choice-despawn.mpack.b64").trim())
            .expect("fixture is valid base64");
        let mut messagepack = decode_lz4_block_array(&bytes).expect("valid typed LZ4 envelope");
        messagepack.push(0xc0);

        assert!(matches!(
            decode_despawn_messagepack(&messagepack),
            Err(ReplayIdentityError::InvalidDespawnSchema)
        ));
    }

    #[test]
    fn exact_run_state_and_message_fence_resolves_all_current_choices() {
        let directory = tempfile::tempdir().expect("temporary replay directory");
        let despawn_message_bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../fixtures/day-3-choice-despawn.mpack.b64").trim())
            .expect("fixture is valid base64");
        write_replay(directory.path(), &despawn_message_bytes);
        let config = CandidateIdentityProviderConfig::BppCombatReplay {
            directory_path: directory.path().to_path_buf(),
            max_file_age_seconds: 60,
        };
        let mut resolver = CandidateIdentityResolver::new(Some(&config));
        let fence = CandidateIdentityFence {
            run_id: Some("player-log-run-1-test".into()),
            state_tick_id: 42,
            app_state: "ChoiceState".into(),
            source_message_id: Some("H5B".into()),
            selection_instance_ids: vec![
                "enc_6PMgGlG".into(),
                "enc_Kr4eMFa".into(),
                "enc_W13jGim".into(),
            ],
        };
        let index = TemplateIndex::from_templates([
            template("dc12f4bd-6c33-41f0-b7ad-d62a0be09a47"),
            template("956e9c74-7d98-4a2b-b500-def577914945"),
            template("90e47f27-241c-4769-a6c6-a100d2a33421"),
        ]);

        let snapshot = resolver.resolve(&fence, &index);

        assert_eq!(snapshot.provenance.status, "resolved");
        assert_eq!(snapshot.provenance.source_run_id, fence.run_id);
        assert_eq!(snapshot.provenance.source_state_tick_id, Some(42));
        assert_eq!(
            snapshot.provenance.source_message_id.as_deref(),
            Some("H5B")
        );
        assert_eq!(
            snapshot.template_id("enc_6PMgGlG", &fence),
            Some("dc12f4bd-6c33-41f0-b7ad-d62a0be09a47")
        );
        assert_eq!(snapshot.provenance.mapped_instance_ids.len(), 3);
    }

    #[test]
    fn resolved_replay_snapshot_flows_through_the_profile_context() {
        let directory = tempfile::tempdir().expect("temporary replay directory");
        let despawn_message_bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../fixtures/day-3-choice-despawn.mpack.b64").trim())
            .expect("fixture is valid base64");
        write_replay(directory.path(), &despawn_message_bytes);
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[02:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[02:32:02.506] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [H5B]\n",
            "[02:32:02.507] [AppState] State changed from [ReplayState] to [ChoiceState]\n",
            "[02:32:02.507] [GameSimHandler] Cards Dealt: [enc_6PMgGlG [Medium] | [enc_Kr4eMFa [Medium] | [enc_W13jGim [Medium] | \n",
            "[02:32:02.507] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [H5B]\n",
        ));
        let index = TemplateIndex::from_templates([
            template("dc12f4bd-6c33-41f0-b7ad-d62a0be09a47"),
            template("956e9c74-7d98-4a2b-b500-def577914945"),
            template("90e47f27-241c-4769-a6c6-a100d2a33421"),
        ]);
        let config = CandidateIdentityProviderConfig::BppCombatReplay {
            directory_path: directory.path().to_path_buf(),
            max_file_age_seconds: 60,
        };
        let mut resolver = CandidateIdentityResolver::new(Some(&config));
        let fence = model.candidate_identity_fence();
        let identity = resolver.resolve(&fence, &index);

        let context = model.context_with_candidate_identity(
            &index,
            &CompanionConfig::test_default(),
            123,
            None,
            None,
            &identity,
        );

        assert!(
            context
                .selection_options
                .iter()
                .all(|card| card.identity_status == crate::IdentityStatus::Resolved)
        );
        assert_eq!(
            context.selection_options[0].identity_provenance.as_deref(),
            Some("bpp_combat_replay_despawn_v1")
        );
        assert_eq!(context.provenance.candidate_identity.status, "resolved");
        assert!(context.unresolved_regions.is_empty());
    }

    #[test]
    fn a_followup_choice_message_cannot_reuse_the_previous_replay_snapshot() {
        let directory = tempfile::tempdir().expect("temporary replay directory");
        let despawn_message_bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../fixtures/day-3-choice-despawn.mpack.b64").trim())
            .expect("fixture is valid base64");
        write_replay(directory.path(), &despawn_message_bytes);
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[02:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[02:32:02.506] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [H5B]\n",
            "[02:32:02.507] [AppState] State changed from [ReplayState] to [ChoiceState]\n",
            "[02:32:02.507] [GameSimHandler] Cards Dealt: [enc_6PMgGlG [Medium] | [enc_Kr4eMFa [Medium] | [enc_W13jGim [Medium] | \n",
            "[02:32:02.507] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [H5B]\n",
        ));
        let index = TemplateIndex::from_templates([
            template("dc12f4bd-6c33-41f0-b7ad-d62a0be09a47"),
            template("956e9c74-7d98-4a2b-b500-def577914945"),
            template("90e47f27-241c-4769-a6c6-a100d2a33421"),
        ]);
        let config = CandidateIdentityProviderConfig::BppCombatReplay {
            directory_path: directory.path().to_path_buf(),
            max_file_age_seconds: 60,
        };
        let mut resolver = CandidateIdentityResolver::new(Some(&config));
        let first_fence = model.candidate_identity_fence();
        let first_identity = resolver.resolve(&first_fence, &index);
        assert_eq!(first_identity.provenance.status, "resolved");

        model.ingest_text(concat!(
            "[02:44:09.666] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [cBj]\n",
            "[02:44:09.667] [GameSimHandler] Cards Dealt: [enc_TWLXZnw [Medium] | [enc_7ugGhQo [Medium] | [enc_2iVAawo [Medium] | \n",
            "[02:44:09.667] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [cBj]\n",
        ));
        let context = model.context_with_candidate_identity(
            &index,
            &CompanionConfig::test_default(),
            124,
            None,
            None,
            &first_identity,
        );

        assert!(
            context
                .selection_options
                .iter()
                .all(|card| card.identity_status == crate::IdentityStatus::Unresolved)
        );
        assert_eq!(context.provenance.candidate_identity.status, "unavailable");
        assert_eq!(
            context.provenance.candidate_identity.reason,
            "candidate_identity_snapshot_fence_mismatch"
        );
    }

    #[test]
    fn one_template_missing_from_gamedata_rejects_the_entire_choice_mapping() {
        let directory = tempfile::tempdir().expect("temporary replay directory");
        let despawn_message_bytes = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../fixtures/day-3-choice-despawn.mpack.b64").trim())
            .expect("fixture is valid base64");
        write_replay(directory.path(), &despawn_message_bytes);
        let config = CandidateIdentityProviderConfig::BppCombatReplay {
            directory_path: directory.path().to_path_buf(),
            max_file_age_seconds: 60,
        };
        let mut resolver = CandidateIdentityResolver::new(Some(&config));
        let fence = CandidateIdentityFence {
            run_id: Some("player-log-run-1-test".into()),
            state_tick_id: 42,
            app_state: "ChoiceState".into(),
            source_message_id: Some("H5B".into()),
            selection_instance_ids: vec![
                "enc_6PMgGlG".into(),
                "enc_Kr4eMFa".into(),
                "enc_W13jGim".into(),
            ],
        };
        let index = TemplateIndex::from_templates([
            template("dc12f4bd-6c33-41f0-b7ad-d62a0be09a47"),
            template("956e9c74-7d98-4a2b-b500-def577914945"),
        ]);

        let snapshot = resolver.resolve(&fence, &index);

        assert_eq!(snapshot.provenance.status, "unavailable");
        assert_eq!(
            snapshot.provenance.reason,
            "replay_template_missing_from_gamedata"
        );
        assert!(snapshot.provenance.mapped_instance_ids.is_empty());
        assert_eq!(snapshot.template_id("enc_6PMgGlG", &fence), None);
    }

    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct TestReplayPayload<'a> {
        battle_id: &'a str,
        version: u8,
        #[serde(with = "serde_bytes")]
        despawn_message_bytes: &'a [u8],
    }

    fn write_replay(directory: &std::path::Path, despawn_message_bytes: &[u8]) {
        const BATTLE_ID: &str = "64f52def28884a46a1b9724ed42ade85";
        let message_pack = rmp_serde::to_vec_named(&TestReplayPayload {
            battle_id: BATTLE_ID,
            version: 1,
            despawn_message_bytes,
        })
        .expect("serialize test replay");
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&message_pack).expect("compress test replay");
        let bytes = gzip.finish().expect("finish test replay");
        fs::write(
            directory.join(format!("{BATTLE_ID}.payload.mpack.gz")),
            bytes,
        )
        .expect("write test replay");
    }

    fn template(id: &str) -> TemplateSummary {
        TemplateSummary {
            id: id.into(),
            version: "1.0.11894".into(),
            name: id.into(),
            starting_tier: "Bronze".into(),
            size: "Medium".into(),
            tags: Vec::new(),
            hidden_tags: Vec::new(),
            tooltips: Vec::new(),
            tier_attributes: BTreeMap::new(),
        }
    }
}
