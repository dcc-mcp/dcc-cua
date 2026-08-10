use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod choice_flow;
mod inventory_mutation;
mod replay_identity;
mod reward_flow;
mod ten_win;

use choice_flow::ChoiceFlowTracker;
pub use choice_flow::{ChoiceFence, DecisionEvidence, DecisionReceipt};
pub use inventory_mutation::InventoryMutationReceipt;
use inventory_mutation::{InventoryMutationTracker, MutationLocation};
use replay_identity::{
    CandidateIdentityFence, CandidateIdentityResolver, CandidateIdentitySnapshot,
};
pub use replay_identity::{CandidateIdentityProvenance, CandidateIdentityProviderConfig};
use reward_flow::{RewardFlowTracker, RewardOutcome};
use ten_win::{BuildEvidence, TenWinCorpus};

const PHYSICAL_BOARD_SLOTS: u8 = 10;
const PHYSICAL_BOARD_MASK: u16 = (1 << PHYSICAL_BOARD_SLOTS) - 1;
const PHYSICAL_STORAGE_SLOTS: u8 = 10;
const VERIFIED_LEVEL_FALLBACK_BUILD: &str = "1.0.11894";
const MAX_PLACEMENT_RECEIPTS: usize = 64;
const MAX_PURCHASE_RECEIPTS: usize = 64;
const PURCHASE_CORRELATION_WINDOW_MS: u64 = 2_000;
const MILLIS_PER_DAY: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Error)]
pub enum CompanionError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("GameData database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("card JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ten-win corpus failed validation: {0}")]
    InvalidTenWinCorpus(String),
    #[error("card identity cache failed validation: {0}")]
    InvalidCardIdentityCache(String),
    #[error("runtime config reload is invalid: {0}")]
    InvalidConfigReload(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionConfig {
    pub log_path: PathBuf,
    pub database_path: PathBuf,
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default)]
    pub instance_overrides: BTreeMap<String, InstanceOverride>,
    #[serde(default)]
    pub skill_names: Vec<String>,
    #[serde(default)]
    pub skill_instance_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub candidate_regions: Vec<NormalizedRegion>,
    pub ten_win_corpus_path: Option<PathBuf>,
    pub card_identity_cache_path: Option<PathBuf>,
    #[serde(default = "default_hero")]
    pub hero: String,
    #[serde(default)]
    pub current_progress: Option<ObservedProgress>,
    #[serde(default)]
    pub current_storage_observation: Option<ObservedStorageObservation>,
    #[serde(default)]
    pub current_inventory_mutation_observations: Vec<ObservedInventoryMutation>,
    #[serde(default)]
    pub decision_policy: DecisionPolicyConfig,
    #[serde(default)]
    pub candidate_identity_provider: Option<CandidateIdentityProviderConfig>,
}

fn default_listen() -> String {
    "127.0.0.1:47900".into()
}

fn default_hero() -> String {
    "Vanessa".into()
}

#[cfg(test)]
impl CompanionConfig {
    fn test_default() -> Self {
        Self {
            log_path: PathBuf::new(),
            database_path: PathBuf::new(),
            listen: default_listen(),
            instance_overrides: BTreeMap::new(),
            skill_names: Vec::new(),
            skill_instance_overrides: BTreeMap::new(),
            candidate_regions: Vec::new(),
            ten_win_corpus_path: None,
            card_identity_cache_path: None,
            hero: default_hero(),
            current_progress: None,
            current_storage_observation: None,
            current_inventory_mutation_observations: Vec::new(),
            decision_policy: DecisionPolicyConfig::default(),
            candidate_identity_provider: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedProgress {
    /// Identity of the exact run that the visual observation belongs to.
    ///
    /// Progress without a matching run id is still useful as operator evidence,
    /// but must never drive automated decisions after the Player.log starts a
    /// different run.
    #[serde(default)]
    pub run_id: Option<String>,
    pub day: Option<u8>,
    pub hour: Option<u8>,
    pub level: Option<u8>,
    #[serde(default)]
    pub game_build: Option<String>,
    #[serde(default)]
    pub source_state_tick_id: Option<u64>,
    #[serde(default)]
    pub verification: ObservationVerification,
    pub health: Option<u32>,
    pub max_health: Option<u32>,
    pub gold: Option<u32>,
    pub wins: Option<u8>,
    pub losses: Option<u8>,
    pub loss_streak: Option<u8>,
    pub prestige: Option<u8>,
    pub max_prestige: Option<u8>,
    pub source_observation_id: Option<String>,
    pub provenance: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationVerification {
    #[default]
    Unverified,
    Verified,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedStorageObservation {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub source_state_tick_id: Option<u64>,
    #[serde(default)]
    pub source_observation_id: Option<String>,
    #[serde(default)]
    pub verification: ObservationVerification,
    #[serde(default)]
    pub usable_slot_ids: Vec<u8>,
    #[serde(default)]
    pub occupied_spans: Vec<ObservedStorageSpan>,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedStorageSpan {
    pub left_socket: u8,
    pub width: u8,
    #[serde(default)]
    pub slot_ids: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedInventoryMutation {
    pub run_id: Option<String>,
    pub source_state_tick_id: Option<u64>,
    pub receipt_log_cursor: Option<u64>,
    pub source_observation_id: Option<String>,
    #[serde(default)]
    pub verification: ObservationVerification,
    pub operation: String,
    #[serde(default)]
    pub exact_instance_ids: Vec<String>,
    #[serde(default)]
    pub locations: Vec<ObservedMutationLocation>,
    pub observed_gold_delta: Option<u32>,
    pub effect_status: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedMutationLocation {
    pub instance_id: String,
    pub section: Option<String>,
    pub socket: Option<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionPolicyConfig {
    #[serde(default = "default_gold_floor")]
    pub gold_floor: u32,
    #[serde(default = "default_shop_reroll_cap")]
    pub shop_reroll_cap: u8,
    #[serde(default = "default_safe_opponent_level_gap")]
    pub safe_opponent_level_gap: u8,
}

impl Default for DecisionPolicyConfig {
    fn default() -> Self {
        Self {
            gold_floor: default_gold_floor(),
            shop_reroll_cap: default_shop_reroll_cap(),
            safe_opponent_level_gap: default_safe_opponent_level_gap(),
        }
    }
}

fn default_gold_floor() -> u32 {
    30
}

fn default_shop_reroll_cap() -> u8 {
    1
}

fn default_safe_opponent_level_gap() -> u8 {
    2
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceOverride {
    pub template_id: Option<String>,
    pub tier: Option<String>,
    pub enchantment: Option<String>,
    #[serde(default = "default_override_provenance")]
    pub provenance: String,
}

fn default_override_provenance() -> String {
    "verified_local_override".into()
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSummary {
    pub id: String,
    pub version: String,
    pub name: String,
    pub starting_tier: String,
    pub size: String,
    pub tags: Vec<String>,
    pub hidden_tags: Vec<String>,
    pub tooltips: Vec<String>,
    pub tier_attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default)]
pub struct TemplateIndex {
    templates: BTreeMap<String, TemplateSummary>,
}

impl TemplateIndex {
    pub fn load(path: &Path) -> Result<Self, CompanionError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let mut statement = connection.prepare("SELECT Id, Data FROM cards")?;
        let mut rows = statement.query([])?;
        let mut templates = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let id = row.get::<_, String>(0)?;
            let bytes = row.get::<_, Vec<u8>>(1)?;
            let value = serde_json::from_slice::<Value>(&bytes)?;
            templates.insert(id.clone(), summarize_template(id, &value));
        }
        Ok(Self { templates })
    }

    pub fn get(&self, id: &str) -> Option<&TemplateSummary> {
        self.templates.get(id)
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    #[cfg(test)]
    fn from_templates(templates: impl IntoIterator<Item = TemplateSummary>) -> Self {
        Self {
            templates: templates
                .into_iter()
                .map(|template| (template.id.clone(), template))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardIdentityCache {
    pub schema_version: u8,
    pub provider: String,
    pub records: BTreeMap<String, CachedExternalCardIdentity>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedExternalCardIdentity {
    pub external_card_id: String,
    pub canonical_name: String,
    pub card_type: String,
    pub url: String,
    pub source_patch: String,
    pub verified_at: String,
    pub match_basis: Vec<String>,
}

impl CardIdentityCache {
    pub fn load(path: &Path, index: &TemplateIndex) -> Result<Self, CompanionError> {
        let cache = serde_json::from_slice::<Self>(&fs::read(path)?)?;
        cache.validate(index)?;
        Ok(cache)
    }

    fn validate(&self, index: &TemplateIndex) -> Result<(), CompanionError> {
        if self.schema_version != 1 {
            return Err(CompanionError::InvalidCardIdentityCache(format!(
                "unsupported schemaVersion {}",
                self.schema_version
            )));
        }
        if self.provider != "bazaardb" {
            return Err(CompanionError::InvalidCardIdentityCache(format!(
                "unsupported provider {}",
                self.provider
            )));
        }
        for (template_id, record) in &self.records {
            let Some(template) = index.get(template_id) else {
                return Err(CompanionError::InvalidCardIdentityCache(format!(
                    "unknown local template {template_id}"
                )));
            };
            if record.canonical_name != template.name {
                return Err(CompanionError::InvalidCardIdentityCache(format!(
                    "name mismatch for {template_id}: cache={}, local={}",
                    record.canonical_name, template.name
                )));
            }
            if record.external_card_id.is_empty()
                || !record
                    .external_card_id
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
            {
                return Err(CompanionError::InvalidCardIdentityCache(format!(
                    "invalid BazaarDB card id for {template_id}"
                )));
            }
            let expected_prefix = format!("https://bazaardb.gg/card/{}/", record.external_card_id);
            if !record.url.starts_with(&expected_prefix) {
                return Err(CompanionError::InvalidCardIdentityCache(format!(
                    "non-canonical BazaarDB URL for {template_id}"
                )));
            }
            if record.source_patch.is_empty()
                || record.verified_at.is_empty()
                || record.match_basis.is_empty()
            {
                return Err(CompanionError::InvalidCardIdentityCache(format!(
                    "missing provenance for {template_id}"
                )));
            }
        }
        Ok(())
    }

    fn reference_for(&self, template_id: &str) -> Option<CardExternalReference> {
        self.records
            .get(template_id)
            .map(|record| CardExternalReference {
                provider: self.provider.clone(),
                external_card_id: record.external_card_id.clone(),
                card_type: record.card_type.clone(),
                url: record.url.clone(),
                source_patch: record.source_patch.clone(),
                verified_at: record.verified_at.clone(),
                match_basis: record.match_basis.clone(),
            })
    }
}

fn summarize_template(id: String, value: &Value) -> TemplateSummary {
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let list = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let name = value
        .pointer("/Localization/Title/Text")
        .and_then(Value::as_str)
        .or_else(|| value.get("InternalName").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let tooltips = value
        .pointer("/Localization/Tooltips")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tooltip| tooltip.pointer("/Content/Text").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let tier_attributes = value
        .get("Tiers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(tier, data)| {
            (
                tier.clone(),
                data.get("Attributes").cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    TemplateSummary {
        id,
        version: string("Version"),
        name,
        starting_tier: string("StartingTier"),
        size: string("Size"),
        tags: list("Tags"),
        hidden_tags: list("HiddenTags"),
        tooltips,
        tier_attributes,
    }
}

#[derive(Clone, Debug, Default)]
struct CardLocation {
    section: String,
    socket: Option<u8>,
    size: String,
}

#[derive(Clone, Debug, Default)]
struct Candidate {
    instance_id: String,
    size: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BoardUnlockEvidence {
    bitmask: u16,
    provenance: &'static str,
}

#[derive(Clone, Debug)]
struct BoardUnlockResolution {
    evidence: Option<BoardUnlockEvidence>,
    reason: &'static str,
}

fn verified_level_unlock_bitmask(level: u8) -> Option<u16> {
    match level {
        1 => Some(0x078),
        2 => Some(0x0fc),
        3 => Some(0x1fe),
        4.. => Some(0x3ff),
        _ => None,
    }
}

fn resolve_board_unlock(
    authoritative: Option<&BoardUnlockEvidence>,
    progress: Option<&ObservedProgress>,
    current_tick_id: u64,
) -> BoardUnlockResolution {
    if let Some(evidence) = authoritative {
        return BoardUnlockResolution {
            evidence: Some(evidence.clone()),
            reason: evidence.provenance,
        };
    }
    let Some(progress) = progress else {
        return BoardUnlockResolution {
            evidence: None,
            reason: "unverified_no_authoritative_mask_or_current_progress",
        };
    };
    if progress.verification != ObservationVerification::Verified {
        return BoardUnlockResolution {
            evidence: None,
            reason: "unverified_level_observation",
        };
    }
    if progress.source_state_tick_id != Some(current_tick_id) {
        return BoardUnlockResolution {
            evidence: None,
            reason: "stale_level_observation_tick",
        };
    }
    if progress.game_build.as_deref() != Some(VERIFIED_LEVEL_FALLBACK_BUILD) {
        return BoardUnlockResolution {
            evidence: None,
            reason: "unsupported_game_build_for_level_fallback",
        };
    }
    if progress
        .source_observation_id
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return BoardUnlockResolution {
            evidence: None,
            reason: "unbound_level_observation",
        };
    }
    let Some(bitmask) = progress.level.and_then(verified_level_unlock_bitmask) else {
        return BoardUnlockResolution {
            evidence: None,
            reason: "invalid_level_for_unlock_fallback",
        };
    };
    BoardUnlockResolution {
        evidence: Some(BoardUnlockEvidence {
            bitmask,
            provenance: "verified_level_fallback_build_1.0.11894",
        }),
        reason: "verified_level_fallback_build_1.0.11894",
    }
}

fn parse_unlock_bitmask_value(text: &str) -> Option<u16> {
    let value = text.trim_start_matches(|character: char| {
        character.is_whitespace() || matches!(character, ':' | '=' | '[' | '{')
    });
    let token = value
        .split(|character: char| character.is_whitespace() || matches!(character, ',' | ']' | '}'))
        .next()?;
    let bitmask = if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        u16::from_str_radix(hex, 16).ok()?
    } else {
        token.parse::<u16>().ok()?
    };
    (bitmask != 0 && bitmask & !PHYSICAL_BOARD_MASK == 0).then_some(bitmask)
}

fn parse_authoritative_board_unlock(line: &str) -> Option<BoardUnlockEvidence> {
    let (marker, provenance) = if line.contains("GameSimEventSocketsUnlocked") {
        (
            "UnlockedSocketsBitmask",
            "game_sim_event_sockets_unlocked_bitmask",
        )
    } else if line.contains("PlayerSnapshotDTO") || line.contains("TPlayerInventory") {
        ("UnlockedSlots", "player_snapshot_unlocked_slots_bitmask")
    } else {
        return None;
    };
    let bitmask = line
        .split_once(marker)
        .and_then(|(_, value)| parse_unlock_bitmask_value(value))?;
    Some(BoardUnlockEvidence {
        bitmask,
        provenance,
    })
}

fn start_run_id(line: &str, generation: u64) -> String {
    // Stable FNV-1a keeps the contract deterministic across companion restarts
    // without introducing a crypto dependency. The generation disambiguates
    // duplicate timestamp text within one append-only Player.log.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in line.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("player-log-run-{generation}-{hash:016x}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacementReceipt {
    pub instance_id: String,
    pub desired_socket: Option<u8>,
    pub clamp: String,
    pub final_socket: u8,
    pub provenance: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurchaseReceipt {
    pub state_tick_id: u64,
    pub instance_id: String,
    pub template_id: String,
    pub target_section: String,
    pub final_socket: u8,
    pub desired_socket: Option<u8>,
    pub clamp: String,
    pub select_item_command_seen_in_window: bool,
    pub correlation_window_ms: u64,
    pub provenance: String,
}

#[derive(Clone, Debug)]
struct PendingPurchaseMovement {
    instance_id: String,
    section: String,
    final_socket: u8,
    timestamp_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RunModel {
    app_state: String,
    tick: u64,
    run_generation: u64,
    run_id: Option<String>,
    ingested_log_cursor: u64,
    template_ids: BTreeMap<String, String>,
    player_items: BTreeMap<String, CardLocation>,
    selection_options: Vec<Candidate>,
    selection_message_id: Option<String>,
    selection_awaiting_message_finish: bool,
    selection_batch_started_message_id: Option<String>,
    selection_batch_message_ownership_conflicted: bool,
    instance_upgrade_counts: BTreeMap<String, u8>,
    selected_skill_instance_ids: Vec<String>,
    board_snapshot_seen: bool,
    stash_delta_seen: bool,
    pending_new_purchase_instances: BTreeSet<String>,
    board_unlock_evidence: Option<BoardUnlockEvidence>,
    placement_receipts: Vec<PlacementReceipt>,
    pending_purchase_movement: Option<PendingPurchaseMovement>,
    last_select_item_command_ms: Option<u64>,
    purchase_receipts: Vec<PurchaseReceipt>,
    inventory_mutations: InventoryMutationTracker,
    choice_flow: ChoiceFlowTracker,
    reward_flow: RewardFlowTracker,
}

impl RunModel {
    fn record_final_socket_receipt(&mut self, instance_id: &str, final_socket: u8) {
        if final_socket >= PHYSICAL_BOARD_SLOTS {
            return;
        }
        if self.placement_receipts.len() == MAX_PLACEMENT_RECEIPTS {
            self.placement_receipts.remove(0);
        }
        self.placement_receipts.push(PlacementReceipt {
            instance_id: instance_id.into(),
            desired_socket: None,
            clamp: "unknown".into(),
            final_socket,
            provenance: "card_operation_utility_final_socket_only".into(),
        });
    }

    fn record_purchase_receipt(
        &mut self,
        instance_id: &str,
        template_id: &str,
        target: &CardLocation,
        purchase_timestamp_ms: Option<u64>,
    ) {
        if self.run_id.is_none() {
            return;
        }
        let movement = self.pending_purchase_movement.clone();
        let select_item_command_ms = self.last_select_item_command_ms;
        let (Some(movement), Some(select_item_command_ms), Some(purchase_timestamp_ms)) =
            (movement, select_item_command_ms, purchase_timestamp_ms)
        else {
            return;
        };
        if movement.instance_id != instance_id
            || movement.section != target.section
            || target.socket != Some(movement.final_socket)
            || !is_physical_item_socket(&movement.section, movement.final_socket)
            || !is_within_log_window(
                movement.timestamp_ms,
                select_item_command_ms,
                PURCHASE_CORRELATION_WINDOW_MS,
            )
            || !is_within_log_window(
                select_item_command_ms,
                purchase_timestamp_ms,
                PURCHASE_CORRELATION_WINDOW_MS,
            )
            || !is_within_log_window(
                movement.timestamp_ms,
                purchase_timestamp_ms,
                PURCHASE_CORRELATION_WINDOW_MS,
            )
        {
            return;
        }
        self.pending_purchase_movement = None;
        self.last_select_item_command_ms = None;
        if self.purchase_receipts.len() == MAX_PURCHASE_RECEIPTS {
            self.purchase_receipts.remove(0);
        }
        self.purchase_receipts.push(PurchaseReceipt {
            state_tick_id: self.tick.saturating_add(1),
            instance_id: instance_id.into(),
            template_id: template_id.into(),
            target_section: target.section.clone(),
            final_socket: movement.final_socket,
            desired_socket: None,
            clamp: "unknown".into(),
            select_item_command_seen_in_window: true,
            correlation_window_ms: PURCHASE_CORRELATION_WINDOW_MS,
            provenance: "player_log_exact_purchase_commit".into(),
        });
    }

    pub fn ingest_text(&mut self, text: &str) {
        self.ingest_text_at(self.ingested_log_cursor, text);
    }

    fn ingest_text_at(&mut self, start_cursor: u64, text: &str) {
        let mut log_cursor = start_cursor;
        for chunk in text.split_inclusive('\n') {
            log_cursor = log_cursor.saturating_add(chunk.len() as u64);
            let line = chunk.trim_end_matches(['\r', '\n']);
            self.ingest_line(line, log_cursor);
        }
        self.ingested_log_cursor = log_cursor;
    }

    fn ingest_line(&mut self, line: &str, log_cursor: u64) {
        let mut changed = false;
        let timestamp_ms = parse_log_timestamp_ms(line);
        for command in ["SellCardCommand", "MoveItemCommand"] {
            if line.contains(&format!("Sending {command} to /commands")) {
                changed |= self.inventory_mutations.observe_command_sent(
                    command,
                    self.tick.saturating_add(1),
                    timestamp_ms,
                    log_cursor,
                );
            }
            if line.contains(&format!("/commands | {command} response")) {
                changed |= self.inventory_mutations.observe_command_response(
                    command,
                    self.tick.saturating_add(1),
                    timestamp_ms,
                    log_cursor,
                );
            }
        }
        if line.contains("/commands requires session re-establish after HTTP 410") {
            changed |= self.inventory_mutations.observe_retryable_recovery(
                "SellCardCommand",
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
            ) || self.inventory_mutations.observe_retryable_recovery(
                "MoveItemCommand",
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
            );
        }
        if let Some(request_id) = parse_captured_request_id(line) {
            changed |= self.inventory_mutations.observe_request_id(
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
                request_id,
            );
        }
        if line.contains("Sending SelectItemCommand to /commands") {
            self.last_select_item_command_ms = timestamp_ms;
        }
        if line.contains("Sending SelectSkillCommand to /commands") {
            self.reward_flow.observe_select_skill_command_sent();
        }
        if line.contains("/commands | SelectSkillCommand response") {
            self.reward_flow.observe_select_skill_command_response();
        }
        if line.contains("Sending ExitCurrentStateCommand to /commands") {
            self.reward_flow.observe_exit_command_sent();
            if let Some(choice_fence) = self.current_choice_fence()
                && choice_fence.choice_kind == "event_option"
            {
                self.choice_flow.observe_discard_requested(choice_fence);
            }
        }
        if line.contains("/commands | ExitCurrentStateCommand response") {
            self.reward_flow.observe_exit_command_response();
            self.choice_flow.observe_exit_command_response();
        }
        if let Some(message_id) = parse_processing_game_sim_message_id(line) {
            changed |= self.inventory_mutations.observe_message_started(
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
                message_id,
            );
            self.choice_flow.observe_message_started(message_id);
            if self.selection_batch_started_message_id.is_some()
                || self.selection_awaiting_message_finish
            {
                self.selection_batch_message_ownership_conflicted = true;
            } else {
                self.selection_batch_started_message_id = Some(message_id.to_owned());
            }
        }
        if let Some((reported_previous_state, state)) = parse_app_state_transition(line) {
            if self.app_state != state {
                if state == "StartRunAppState" {
                    self.run_generation = self.run_generation.saturating_add(1);
                    self.run_id = Some(start_run_id(line, self.run_generation));
                    self.player_items.clear();
                    self.selection_options.clear();
                    self.selection_message_id = None;
                    self.selection_awaiting_message_finish = false;
                    self.selection_batch_started_message_id = None;
                    self.selection_batch_message_ownership_conflicted = false;
                    self.instance_upgrade_counts.clear();
                    self.selected_skill_instance_ids.clear();
                    self.board_snapshot_seen = false;
                    self.stash_delta_seen = false;
                    self.pending_new_purchase_instances.clear();
                    self.board_unlock_evidence = None;
                    self.placement_receipts.clear();
                    self.pending_purchase_movement = None;
                    self.last_select_item_command_ms = None;
                    self.purchase_receipts.clear();
                    self.inventory_mutations.reset();
                    self.choice_flow.reset();
                    self.reward_flow.reset();
                }
                let previous_state = self.app_state.clone();
                self.choice_flow
                    .observe_state_transition(reported_previous_state, state);
                self.reward_flow
                    .observe_state_transition(&previous_state, state);
                if state != "StartRunAppState"
                    && choice_kind_for_app_state(state) != "none"
                    && self.selection_batch_started_message_id.is_none()
                {
                    self.selection_batch_message_ownership_conflicted = true;
                }
                self.app_state = state.to_owned();
                changed = true;
            } else {
                self.choice_flow
                    .observe_state_transition(reported_previous_state, state);
            }
        }

        if let Some(evidence) = parse_authoritative_board_unlock(line)
            && self.board_unlock_evidence.as_ref() != Some(&evidence)
        {
            self.board_unlock_evidence = Some(evidence);
            changed = true;
        }

        if let Some(rest) = line.split("Card Purchased: InstanceId: ").nth(1)
            && let Some((instance_id, rest)) = rest.split_once(" - TemplateId")
            && let Some((template_id, rest)) = rest.split_once(" - Target:")
        {
            let instance_id = instance_id.trim();
            let template_id = template_id.trim();
            self.choice_flow.observe_candidate_purchase(instance_id);
            let target = rest.split(" - Section").next().unwrap_or_default().trim();
            let is_player_target = target.starts_with("Player");
            let is_new_drag_purchase = self.pending_new_purchase_instances.contains(instance_id);
            let has_distinct_item_offer = self.selection_options.iter().any(|candidate| {
                candidate.instance_id.starts_with("itm_") && candidate.instance_id != instance_id
            });
            if is_player_target
                && self.player_items.contains_key(instance_id)
                && !is_new_drag_purchase
                && has_distinct_item_offer
                && self
                    .template_ids
                    .get(instance_id)
                    .is_none_or(|known| known == template_id)
            {
                let next_count = self
                    .instance_upgrade_counts
                    .get(instance_id)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(1);
                self.instance_upgrade_counts
                    .insert(instance_id.to_owned(), next_count);
            }
            self.template_ids
                .insert(instance_id.to_owned(), template_id.to_owned());
            let location = target_location(target, None);
            if let Some(location) = location.clone() {
                self.player_items.insert(instance_id.to_owned(), location);
            }
            if let Some(location) = location.as_ref()
                && let Some(socket) = location.socket
            {
                self.reward_flow
                    .observe_item_purchase(instance_id, &location.section, socket);
            }
            self.selection_options
                .retain(|candidate| candidate.instance_id != instance_id);
            if is_player_target {
                if let Some(location) = location.as_ref() {
                    self.record_purchase_receipt(instance_id, template_id, location, timestamp_ms);
                }
                self.pending_new_purchase_instances.remove(instance_id);
            }
            changed = true;
        }

        if let Some(instance_id) = line
            .split("Selected skill ")
            .nth(1)
            .and_then(|rest| rest.split(" to socket").next())
            .map(str::trim)
            .filter(|instance_id| !instance_id.is_empty())
            && !self
                .selected_skill_instance_ids
                .iter()
                .any(|known| known == instance_id)
        {
            self.reward_flow.observe_selected_skill(instance_id);
            self.selected_skill_instance_ids
                .push(instance_id.to_owned());
            changed = true;
        }

        if let Some(rest) = line.split("Cards Dealt: ").nth(1) {
            let dealt = parse_card_segments(rest);
            self.choice_flow.observe_candidates_dealt();
            self.reward_flow.observe_candidates(
                &self.app_state,
                dealt
                    .iter()
                    .map(|(instance_id, _)| instance_id.clone())
                    .collect(),
            );
            self.selection_options = dealt
                .into_iter()
                .map(|(instance_id, size)| Candidate { instance_id, size })
                .collect();
            self.selection_message_id = None;
            if self.selection_awaiting_message_finish {
                self.selection_batch_message_ownership_conflicted = true;
            }
            self.selection_awaiting_message_finish = !self.selection_options.is_empty();
            changed = true;
        }

        if let Some(message_id) = parse_finished_game_sim_message_id(line) {
            changed |= self.inventory_mutations.observe_message_finished(
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
                message_id,
            );
            let choice_finish = self.choice_flow.observe_message_finished(message_id);
            changed |= choice_finish.changed();
            if self.selection_awaiting_message_finish {
                let exact_message_owner = !self.selection_batch_message_ownership_conflicted
                    && self.selection_batch_started_message_id.as_deref() == Some(message_id);
                self.selection_message_id = exact_message_owner.then(|| message_id.to_owned());
                self.selection_awaiting_message_finish = false;
                changed = true;
            }
            self.selection_batch_started_message_id = None;
            self.selection_batch_message_ownership_conflicted = false;
        }

        if let Some(rest) = line.split("Cards Spawned: ").nth(1) {
            let mut spawned = Vec::new();
            for segment in rest.split('|') {
                let Some((instance_id, size)) = parse_card_segment(segment) else {
                    continue;
                };
                if !segment.contains("[Player]") {
                    continue;
                }
                let section = if segment.contains("[Stash]") {
                    "stash"
                } else if segment.contains("[Hand]") {
                    "board"
                } else {
                    continue;
                };
                let socket = parse_socket(segment);
                spawned.push((
                    instance_id,
                    CardLocation {
                        section: section.into(),
                        socket,
                        size,
                    },
                ));
            }
            if spawned
                .iter()
                .filter(|(_, location)| location.section == "board")
                .count()
                >= 2
            {
                self.player_items
                    .retain(|_, location| location.section != "board");
                self.board_snapshot_seen = true;
            }
            for (instance_id, location) in spawned {
                if location.section == "stash" {
                    self.stash_delta_seen = true;
                }
                self.player_items.insert(instance_id, location);
                changed = true;
            }
        }

        if let Some(rest) = line.split("Cards Disposed: ").nth(1) {
            let disposed = rest
                .split('|')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            self.choice_flow.observe_disposed(&disposed);
            self.reward_flow.observe_disposed(&disposed);
            for instance_id in disposed {
                self.player_items.remove(&instance_id);
                self.selection_options
                    .retain(|candidate| candidate.instance_id != instance_id);
            }
            changed = true;
        }

        if let Some(instance_id) = line
            .split("Successfully removed item ")
            .nth(1)
            .and_then(|rest| rest.split(" from player's inventory").next())
            .map(str::trim)
            .filter(|instance_id| !instance_id.is_empty())
        {
            let before = self
                .player_items
                .get(instance_id)
                .map(|location| MutationLocation::known(&location.section, location.socket));
            let template_id = self.template_ids.get(instance_id).cloned();
            changed |= self.inventory_mutations.observe_sell_removed(
                self.run_id.as_deref(),
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
                instance_id,
                before,
                template_id,
            );
            changed |= self.player_items.remove(instance_id).is_some();
            self.selection_options
                .retain(|candidate| candidate.instance_id != instance_id);
        } else if let Some((instance_id, value_gold)) = parse_sold_card(line) {
            changed |= self.inventory_mutations.observe_sell_commit(
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
                instance_id,
                value_gold,
            );
            changed |= self.player_items.remove(instance_id).is_some();
            self.selection_options
                .retain(|candidate| candidate.instance_id != instance_id);
        }

        if let Some(rest) = line.split("Successfully moved card to: [").nth(1) {
            let instance_id = rest.split_whitespace().next().unwrap_or_default();
            if !instance_id.is_empty() {
                self.pending_new_purchase_instances
                    .insert(instance_id.into());
                let section = if rest.contains("[Stash]") {
                    "stash"
                } else {
                    "board"
                };
                let size = ["Small", "Medium", "Large"]
                    .into_iter()
                    .find(|size| rest.contains(&format!("[{size}]")))
                    .unwrap_or_default()
                    .to_owned();
                let socket = parse_socket(rest);
                if socket.is_some_and(|socket| !is_physical_item_socket(section, socket)) {
                    return;
                }
                self.player_items.insert(
                    instance_id.into(),
                    CardLocation {
                        section: section.into(),
                        socket,
                        size,
                    },
                );
                if let Some(socket) = socket {
                    self.record_final_socket_receipt(instance_id, socket);
                    self.pending_purchase_movement =
                        timestamp_ms.map(|timestamp_ms| PendingPurchaseMovement {
                            instance_id: instance_id.into(),
                            section: section.into(),
                            final_socket: socket,
                            timestamp_ms,
                        });
                }
                changed = true;
            }
        } else if let Some(rest) = line.split("Successfully moved card ").nth(1)
            && let Some((instance_id, socket)) = rest.split_once(" to Socket_")
            && let Ok(socket) = socket.trim().parse::<u8>()
            && socket < PHYSICAL_BOARD_SLOTS
        {
            let instance_id = instance_id.trim();
            let before = self
                .player_items
                .get(instance_id)
                .map(|location| MutationLocation::known(&location.section, location.socket));
            let _ = self.inventory_mutations.observe_reorder(
                self.run_id.as_deref(),
                self.tick.saturating_add(1),
                timestamp_ms,
                log_cursor,
                instance_id,
                before,
                socket,
            );
            if let Some(location) = self.player_items.get_mut(instance_id) {
                location.socket = Some(socket);
            }
            self.record_final_socket_receipt(instance_id, socket);
            changed = true;
        }

        if changed {
            self.tick = self.tick.saturating_add(1);
        }
    }

    pub fn context(
        &self,
        index: &TemplateIndex,
        config: &CompanionConfig,
        log_offset: u64,
        ten_win_corpus: Option<&TenWinCorpus>,
        card_identity_cache: Option<&CardIdentityCache>,
    ) -> Context {
        let candidate_identity = CandidateIdentitySnapshot::disabled();
        self.context_with_candidate_identity(
            index,
            config,
            log_offset,
            ten_win_corpus,
            card_identity_cache,
            &candidate_identity,
        )
    }

    fn candidate_identity_fence(&self) -> CandidateIdentityFence {
        CandidateIdentityFence {
            run_id: self.run_id.clone(),
            state_tick_id: self.tick,
            app_state: self.app_state.clone(),
            source_message_id: self.selection_message_id.clone(),
            selection_instance_ids: self
                .selection_options
                .iter()
                .map(|candidate| candidate.instance_id.clone())
                .collect(),
        }
    }

    fn current_choice_fence(&self) -> Option<ChoiceFence> {
        let run_id = self.run_id.clone()?;
        let selection_message_id = self.selection_message_id.clone()?;
        let choice_kind = choice_kind_for_app_state(&self.app_state);
        if choice_kind == "none" || self.selection_options.is_empty() {
            return None;
        }
        Some(ChoiceFence {
            run_id,
            state_tick_id: self.tick,
            selection_message_id,
            choice_kind: choice_kind.into(),
            candidate_instance_ids: self
                .selection_options
                .iter()
                .map(|candidate| candidate.instance_id.clone())
                .collect(),
        })
    }

    pub(crate) fn context_with_candidate_identity(
        &self,
        index: &TemplateIndex,
        config: &CompanionConfig,
        log_offset: u64,
        ten_win_corpus: Option<&TenWinCorpus>,
        card_identity_cache: Option<&CardIdentityCache>,
        candidate_identity: &CandidateIdentitySnapshot,
    ) -> Context {
        let candidate_identity_fence = self.candidate_identity_fence();
        let mut board_items = Vec::new();
        let mut chest_items = Vec::new();
        for (instance_id, location) in &self.player_items {
            let card = self.resolve_card(
                instance_id,
                location,
                index,
                config,
                card_identity_cache,
                None,
            );
            if location.section == "stash" {
                chest_items.push(card);
            } else {
                board_items.push(card);
            }
        }
        board_items.sort_by_key(|card| (card.socket.unwrap_or(u8::MAX), card.instance_id.clone()));
        chest_items.sort_by_key(|card| (card.socket.unwrap_or(u8::MAX), card.instance_id.clone()));
        let selection_section = selection_section_for_app_state(&self.app_state);
        let selection_options = self
            .selection_options
            .iter()
            .map(|candidate| {
                self.resolve_card(
                    &candidate.instance_id,
                    &CardLocation {
                        section: selection_section.into(),
                        socket: None,
                        size: candidate.size.clone(),
                    },
                    index,
                    config,
                    card_identity_cache,
                    Some((&candidate_identity_fence, candidate_identity)),
                )
            })
            .collect::<Vec<_>>();
        let unresolved_regions = selection_options
            .iter()
            .enumerate()
            .filter(|(_, card)| card.identity_status == IdentityStatus::Unresolved)
            .map(|(index, card)| UnresolvedRegion {
                instance_id: card.instance_id.clone(),
                reason: "instance_has_no_template_mapping".into(),
                visual_scope: "localized_card_only".into(),
                region_hint: format!("selection_option_{index}"),
                crop: config
                    .candidate_regions
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| default_candidate_region(index)),
            })
            .collect::<Vec<_>>();
        let candidate_actionable_regions = selection_options
            .iter()
            .enumerate()
            .map(|(index, card)| {
                let is_single_loot_candidate =
                    self.app_state == "LootState" && selection_options.len() == 1;
                CandidateActionableRegion {
                    instance_id: card.instance_id.clone(),
                    identity_status: card.identity_status.clone(),
                    primary_gesture: "left_click".into(),
                    region_hint: if is_single_loot_candidate {
                        "loot_single_candidate_center".into()
                    } else {
                        format!("selection_option_{index}")
                    },
                    region_source: if is_single_loot_candidate
                        || config.candidate_regions.get(index).is_none()
                    {
                        "unverified_layout_hint".into()
                    } else {
                        "configured_layout_hint".into()
                    },
                    requires_fresh_observation_fence: true,
                    crop: if is_single_loot_candidate {
                        single_loot_candidate_center_region()
                    } else {
                        config
                            .candidate_regions
                            .get(index)
                            .copied()
                            .unwrap_or_else(|| default_candidate_region(index))
                    },
                }
            })
            .collect::<Vec<_>>();
        let player_skill_instances = self
            .selected_skill_instance_ids
            .iter()
            .map(|instance_id| SkillInstance {
                instance_id: instance_id.clone(),
                name: config.skill_instance_overrides.get(instance_id).cloned(),
                identity_status: if config.skill_instance_overrides.contains_key(instance_id) {
                    IdentityStatus::Resolved
                } else {
                    IdentityStatus::Unresolved
                },
                identity_provenance: config
                    .skill_instance_overrides
                    .contains_key(instance_id)
                    .then(|| "verified_local_instance_mapping".into()),
            })
            .collect::<Vec<_>>();
        let mut player_skills = config.skill_names.clone();
        for skill in player_skill_instances
            .iter()
            .filter_map(|skill| skill.name.clone())
        {
            if !player_skills.iter().any(|known| known == &skill) {
                player_skills.push(skill);
            }
        }
        let relationship_graph =
            build_relationship_graph(&board_items, &chest_items, &player_skills);
        let template_ids = |cards: &[CardInstance]| {
            cards
                .iter()
                .filter_map(|card| card.template_id.clone())
                .collect::<Vec<_>>()
        };
        let build_evidence = ten_win_corpus.and_then(|corpus| {
            corpus.evaluate(
                &config.hero,
                &template_ids(&board_items),
                &template_ids(&chest_items),
                &template_ids(&selection_options),
                |template_id| index.get(template_id).map(|card| card.name.clone()),
            )
        });
        let (current_progress, progress_status, progress_reason) =
            match (self.run_id.as_deref(), config.current_progress.as_ref()) {
                (Some(current_run_id), Some(progress))
                    if progress.run_id.as_deref() == Some(current_run_id) =>
                {
                    (
                        Some(progress),
                        "current",
                        "observation_is_bound_to_current_player_log_run",
                    )
                }
                (Some(_), Some(progress)) if progress.run_id.is_none() => (
                    None,
                    "unbound",
                    "observation_has_no_run_id_and_cannot_drive_decisions",
                ),
                (Some(_), Some(_)) => (
                    None,
                    "stale",
                    "observation_run_id_does_not_match_current_player_log_run",
                ),
                (None, Some(_)) => (
                    None,
                    "unverified",
                    "player_log_has_not_established_a_current_run_id",
                ),
                (_, None) => (
                    None,
                    "missing",
                    "no_visual_progress_observation_is_configured",
                ),
            };
        let state_completeness = build_state_completeness(
            self,
            &board_items,
            &chest_items,
            &selection_options,
            &player_skill_instances,
            current_progress,
        );
        let board_unlock = resolve_board_unlock(
            self.board_unlock_evidence.as_ref(),
            current_progress,
            self.tick,
        );
        let board_capacity = build_board_capacity(
            &board_items,
            &chest_items,
            &state_completeness.board,
            &board_unlock,
        );
        let storage_capacity = build_storage_capacity(
            &selection_options,
            config.current_storage_observation.as_ref(),
            self.run_id.as_deref(),
            self.tick,
        );
        let upgrade_opportunity = build_upgrade_opportunity(
            &self.app_state,
            &board_items,
            &chest_items,
            &selection_options,
            &state_completeness.board,
            &state_completeness.stash,
        );
        let choice_fence = self.current_choice_fence();
        let allow_discard_current_choice = choice_fence
            .as_ref()
            .is_some_and(|fence| !self.choice_flow.has_receipt_for(fence));
        let available_actions = build_available_actions(
            &self.app_state,
            &selection_options,
            &unresolved_regions,
            &board_capacity,
            allow_discard_current_choice,
        );
        let decision_support = build_decision_support(
            &relationship_graph,
            &selection_options,
            current_progress,
            &config.decision_policy,
            board_capacity,
            storage_capacity,
            upgrade_opportunity,
        );
        let mut inventory_mutation_receipts = self.inventory_mutations.receipts();
        for receipt in &mut inventory_mutation_receipts {
            let Some(template_id) = receipt.sell_template_id.as_deref() else {
                continue;
            };
            let Some(template) = index.get(template_id) else {
                receipt.set_sell_effect("unknown_template_or_on_sell_effect", Vec::new());
                continue;
            };
            let descriptions = template
                .tooltips
                .iter()
                .filter(|tooltip| tooltip.to_ascii_lowercase().contains("when you sell this"))
                .cloned()
                .collect::<Vec<_>>();
            let status = if descriptions.is_empty() {
                "no_on_sell_effect_in_resolved_template"
            } else {
                "resolved_on_sell_tooltip"
            };
            receipt.set_sell_effect(status, descriptions);
        }
        for receipt in &mut inventory_mutation_receipts {
            apply_verified_inventory_mutation_observation(
                receipt,
                &config.current_inventory_mutation_observations,
            );
        }
        Context {
            schema_version: "3.6.0".into(),
            tick_id: self.tick,
            provenance: Provenance {
                player_log: SourceProvenance {
                    path: config.log_path.clone(),
                    mode: "incremental_tail".into(),
                    cursor: log_offset,
                },
                game_data: GameDataProvenance {
                    path: config.database_path.clone(),
                    mode: "read_only_one_time_index".into(),
                    indexed_templates: index.len(),
                },
                card_identity_cache: config.card_identity_cache_path.as_ref().map(|path| {
                    CardIdentityCacheProvenance {
                        path: path.clone(),
                        mode: "read_only_versioned_snapshot".into(),
                        loaded_records: card_identity_cache
                            .map(|cache| cache.records.len())
                            .unwrap_or_default(),
                    }
                }),
                candidate_identity: candidate_identity.provenance_for(&candidate_identity_fence),
                visual_fallback: "localized_unknown_regions_only".into(),
            },
            run: RunState {
                app_state: self.app_state.clone(),
                run_id: self.run_id.clone(),
                state_tick_id: self.tick,
                progress: current_progress.cloned(),
                progress_status: progress_status.into(),
                progress_reason: progress_reason.into(),
                pending_choice: !selection_options.is_empty(),
                choice_kind: choice_kind_for_app_state(&self.app_state).into(),
            },
            board_items,
            chest_items,
            selection_options,
            player_skills,
            player_skill_instances,
            relationship_graph,
            build_evidence,
            state_completeness,
            decision_support,
            unresolved_regions,
            candidate_actionable_regions,
            choice_fence,
            available_actions,
            placement_receipts: self.placement_receipts.clone(),
            purchase_receipts: self.purchase_receipts.clone(),
            inventory_mutation_receipts,
            decision_outcomes: self.choice_flow.outcomes(),
            reward_outcomes: self.reward_flow.outcomes(),
        }
    }

    fn resolve_card(
        &self,
        instance_id: &str,
        location: &CardLocation,
        index: &TemplateIndex,
        config: &CompanionConfig,
        card_identity_cache: Option<&CardIdentityCache>,
        candidate_identity: Option<(&CandidateIdentityFence, &CandidateIdentitySnapshot)>,
    ) -> CardInstance {
        let override_data = config.instance_overrides.get(instance_id);
        let replay_template_id = candidate_identity
            .and_then(|(fence, snapshot)| snapshot.template_id(instance_id, fence));
        let template_id = override_data
            .and_then(|value| value.template_id.as_deref())
            .or_else(|| self.template_ids.get(instance_id).map(String::as_str))
            .or(replay_template_id);
        let template = template_id.and_then(|id| index.get(id));
        let provenance = if override_data
            .and_then(|value| value.template_id.as_ref())
            .is_some()
        {
            override_data.map(|value| value.provenance.clone())
        } else if template_id.is_some() {
            Some(
                if replay_template_id.is_some() && !self.template_ids.contains_key(instance_id) {
                    "bpp_combat_replay_despawn_v1"
                } else {
                    "player_log_template_id"
                }
                .into(),
            )
        } else {
            None
        };
        let log_upgrade_count = self
            .instance_upgrade_counts
            .get(instance_id)
            .copied()
            .unwrap_or_default();
        let log_tier =
            template.and_then(|value| advance_tier(&value.starting_tier, log_upgrade_count));
        let tier = log_tier
            .clone()
            .or_else(|| override_data.and_then(|value| value.tier.clone()))
            .or_else(|| template.map(|value| value.starting_tier.clone()));
        let tooltips = template
            .map(|value| value.tooltips.clone())
            .unwrap_or_default();
        let effect_activation = classify_effect_activation(&location.section, &tooltips);
        CardInstance {
            instance_id: instance_id.into(),
            template_id: template_id.map(str::to_owned),
            identity_status: if template.is_some() {
                IdentityStatus::Resolved
            } else {
                IdentityStatus::Unresolved
            },
            identity_provenance: provenance,
            name: template.map(|value| value.name.clone()),
            section: location.section.clone(),
            selection_category: selection_category(&location.section, instance_id)
                .map(str::to_owned),
            primary_gesture: location
                .section
                .ends_with("_option")
                .then(|| "left_click".to_owned()),
            right_click_behavior: right_click_behavior(&location.section, instance_id)
                .map(str::to_owned),
            socket: location.socket,
            size: if location.size.is_empty() {
                template.map(|value| value.size.clone()).unwrap_or_default()
            } else {
                location.size.clone()
            },
            tier: tier.clone(),
            tier_provenance: if log_tier.is_some() {
                Some("player_log_repeat_purchase_upgrade".into())
            } else if override_data
                .and_then(|value| value.tier.as_ref())
                .is_some()
            {
                override_data.map(|value| value.provenance.clone())
            } else {
                template.map(|_| "gamedata_starting_tier".into())
            },
            enchantment: override_data.and_then(|value| value.enchantment.clone()),
            enchantment_provenance: override_data
                .filter(|value| value.enchantment.is_some())
                .map(|value| value.provenance.clone()),
            activation_scopes: effect_activation.activation_scopes,
            passive_triggers: effect_activation.passive_triggers,
            inventory_state: effect_activation.inventory_state,
            effect_scope_provenance: effect_activation.provenance,
            tags: template.map(|value| value.tags.clone()).unwrap_or_default(),
            hidden_tags: template
                .map(|value| value.hidden_tags.clone())
                .unwrap_or_default(),
            tooltips,
            attributes: template
                .and_then(|value| {
                    tier.as_deref()
                        .or(Some(value.starting_tier.as_str()))
                        .and_then(|resolved_tier| value.tier_attributes.get(resolved_tier))
                })
                .cloned()
                .unwrap_or(Value::Null),
            external_references: template_id
                .and_then(|id| card_identity_cache.and_then(|cache| cache.reference_for(id)))
                .into_iter()
                .collect(),
        }
    }
}

fn parse_card_segments(rest: &str) -> Vec<(String, String)> {
    rest.split('|').filter_map(parse_card_segment).collect()
}

fn parse_card_segment(segment: &str) -> Option<(String, String)> {
    let instance_id = segment
        .trim()
        .strip_prefix('[')?
        .split_whitespace()
        .next()?
        .to_owned();
    let size = ["Small", "Medium", "Large"]
        .into_iter()
        .find(|size| segment.contains(&format!("[{size}]")))
        .unwrap_or_default()
        .to_owned();
    Some((instance_id, size))
}

fn parse_socket(text: &str) -> Option<u8> {
    text.split("[Socket_")
        .nth(1)
        .and_then(|tail| tail.split(']').next())
        .and_then(|value| value.parse().ok())
}

fn parse_sold_card(line: &str) -> Option<(&str, u32)> {
    let rest = line.split_once("Sold Card ")?.1;
    let (instance_id, value) = rest.split_once(" for ")?;
    let value_gold = value.split_whitespace().next()?.parse().ok()?;
    let instance_id = instance_id.trim();
    (!instance_id.is_empty()).then_some((instance_id, value_gold))
}

fn parse_captured_request_id(line: &str) -> Option<&str> {
    let request_id = line.split_once("Captured request id:")?.1.trim();
    (!request_id.is_empty()).then_some(request_id)
}

fn apply_verified_inventory_mutation_observation(
    receipt: &mut InventoryMutationReceipt,
    observations: &[ObservedInventoryMutation],
) {
    if !receipt.log_committed {
        return;
    }
    let mut exact_fence = Vec::new();
    for observation in observations.iter().filter(|observation| {
        observation.verification == ObservationVerification::Verified
            && observation.run_id.as_deref() == Some(receipt.run_id.as_str())
            && observation.source_state_tick_id == Some(receipt.state_tick_id)
            && observation.receipt_log_cursor == Some(receipt.log_cursor)
    }) {
        if !exact_fence.contains(&observation) {
            exact_fence.push(observation);
        }
    }
    let [observation] = exact_fence.as_slice() else {
        if exact_fence.len() > 1 {
            receipt.status = "pending".into();
            receipt.reason = "ambiguous_verified_observation_evidence".into();
        }
        return;
    };
    if observation
        .source_observation_id
        .as_deref()
        .is_none_or(str::is_empty)
    {
        receipt.status = "pending".into();
        receipt.reason = "unbound_verified_observation".into();
        return;
    }
    if observation.operation != receipt.operation
        || observation.exact_instance_ids != receipt.exact_instance_ids
    {
        receipt.status = "pending".into();
        receipt.reason = "verified_observation_identity_mismatch".into();
        return;
    }

    let mut expected_locations = receipt
        .locations
        .iter()
        .map(|change| ObservedMutationLocation {
            instance_id: change.instance_id.clone(),
            section: change.after.section.clone(),
            socket: change.after.socket,
        })
        .collect::<Vec<_>>();
    let mut observed_locations = observation.locations.clone();
    expected_locations.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    observed_locations.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    let exact_unique_locations = observed_locations
        .windows(2)
        .all(|pair| pair[0].instance_id != pair[1].instance_id)
        && observed_locations == expected_locations;
    if !exact_unique_locations {
        receipt.status = "pending".into();
        receipt.reason = "verified_observation_location_mismatch".into();
        return;
    }

    let success_reason = if receipt.operation == "sell" {
        let Some(expectation) = receipt.sell_expectation.as_ref() else {
            receipt.status = "pending".into();
            receipt.reason = "missing_sell_expectation".into();
            return;
        };
        if observation.observed_gold_delta != expectation.value_gold {
            receipt.status = "pending".into();
            receipt.reason = "verified_sell_value_mismatch".into();
            return;
        }
        let effect_matches = match expectation.effect.status.as_str() {
            "resolved_on_sell_tooltip" => observation.effect_status == "verified",
            "no_on_sell_effect_in_resolved_template" => {
                observation.effect_status == "not_applicable"
            }
            _ => {
                receipt.status = "pending".into();
                receipt.reason = "unresolved_sell_effect_expectation".into();
                return;
            }
        };
        if !effect_matches {
            receipt.status = "pending".into();
            receipt.reason = "verified_sell_effect_mismatch".into();
            return;
        }
        "exact_sell_and_verified_value_effect_observation"
    } else {
        if observation.observed_gold_delta.is_some()
            || observation.effect_status != "not_applicable"
        {
            receipt.status = "pending".into();
            receipt.reason = "unexpected_swap_effect_observation".into();
            return;
        }
        "exact_atomic_reorder_and_verified_observation"
    };

    receipt.finalized = true;
    receipt.status = "succeeded".into();
    receipt.reason = success_reason.into();
    receipt.verified_observation_id = observation.source_observation_id.clone();
    receipt.verified_observation_provenance = Some(observation.provenance.clone());
}

fn parse_game_sim_message_id<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    if !line.contains(marker) {
        return None;
    }
    line.split("Id: [")
        .nth(1)
        .and_then(|tail| tail.split(']').next())
        .map(str::trim)
        .filter(|message_id| {
            !message_id.is_empty()
                && message_id.len() <= 64
                && message_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn parse_processing_game_sim_message_id(line: &str) -> Option<&str> {
    parse_game_sim_message_id(line, "Processing [NetMessageGameSim]")
}

fn parse_finished_game_sim_message_id(line: &str) -> Option<&str> {
    parse_game_sim_message_id(line, "Finished processing [NetMessageGameSim]")
}

fn parse_app_state_transition(line: &str) -> Option<(&str, &str)> {
    let transition = line.split("State changed from [").nth(1)?;
    let (from, to) = transition.split_once("] to [")?;
    Some((from, to.strip_suffix(']')?))
}

fn parse_log_timestamp_ms(line: &str) -> Option<u64> {
    let timestamp = line.strip_prefix('[')?.split_once(']')?.0;
    let mut parts = timestamp.split(':');
    let hours = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let (seconds, milliseconds) = parts.next()?.split_once('.')?;
    if parts.next().is_some() || hours >= 24 || minutes >= 60 || milliseconds.len() != 3 {
        return None;
    }
    let seconds = seconds.parse::<u64>().ok()?;
    let milliseconds = milliseconds.parse::<u64>().ok()?;
    if seconds >= 60 || milliseconds >= 1_000 {
        return None;
    }
    Some((((hours * 60 + minutes) * 60 + seconds) * 1_000) + milliseconds)
}

fn is_physical_item_socket(section: &str, socket: u8) -> bool {
    match section {
        "board" => socket < PHYSICAL_BOARD_SLOTS,
        "stash" => socket < PHYSICAL_STORAGE_SLOTS,
        _ => false,
    }
}

fn is_within_log_window(start_ms: u64, end_ms: u64, window_ms: u64) -> bool {
    let elapsed = if end_ms >= start_ms {
        end_ms - start_ms
    } else {
        end_ms + MILLIS_PER_DAY - start_ms
    };
    elapsed <= window_ms
}

fn target_location(target: &str, size: Option<String>) -> Option<CardLocation> {
    if let Some(socket) = target.strip_prefix("PlayerSocket_") {
        let socket = socket
            .parse()
            .ok()
            .filter(|socket| is_physical_item_socket("board", *socket))?;
        return Some(CardLocation {
            section: "board".into(),
            socket: Some(socket),
            size: size.unwrap_or_default(),
        });
    }
    let socket = target
        .strip_prefix("PlayerStorageSocket_")?
        .parse()
        .ok()
        .filter(|socket| is_physical_item_socket("stash", *socket))?;
    Some(CardLocation {
        section: "stash".into(),
        socket: Some(socket),
        size: size.unwrap_or_default(),
    })
}

fn advance_tier(starting_tier: &str, upgrade_count: u8) -> Option<String> {
    if upgrade_count == 0 {
        return None;
    }
    const TIERS: [&str; 4] = ["Bronze", "Silver", "Gold", "Diamond"];
    let starting_index = TIERS.iter().position(|tier| *tier == starting_tier)?;
    Some(TIERS[(starting_index + usize::from(upgrade_count)).min(TIERS.len() - 1)].into())
}

fn next_tier(tier: &str) -> Option<&'static str> {
    match tier {
        "Bronze" => Some("Silver"),
        "Silver" => Some("Gold"),
        "Gold" => Some("Diamond"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStatus {
    Resolved,
    Unresolved,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardInstance {
    pub instance_id: String,
    pub template_id: Option<String>,
    pub identity_status: IdentityStatus,
    pub identity_provenance: Option<String>,
    pub name: Option<String>,
    pub section: String,
    pub selection_category: Option<String>,
    pub primary_gesture: Option<String>,
    pub right_click_behavior: Option<String>,
    pub socket: Option<u8>,
    pub size: String,
    pub tier: Option<String>,
    pub tier_provenance: Option<String>,
    pub enchantment: Option<String>,
    pub enchantment_provenance: Option<String>,
    pub activation_scopes: Vec<String>,
    pub passive_triggers: Vec<String>,
    pub inventory_state: String,
    pub effect_scope_provenance: String,
    pub tags: Vec<String>,
    pub hidden_tags: Vec<String>,
    pub tooltips: Vec<String>,
    pub attributes: Value,
    pub external_references: Vec<CardExternalReference>,
}

struct EffectActivation {
    activation_scopes: Vec<String>,
    passive_triggers: Vec<String>,
    inventory_state: String,
    provenance: String,
}

fn classify_effect_activation(section: &str, tooltips: &[String]) -> EffectActivation {
    if section.ends_with("_option") {
        return EffectActivation {
            activation_scopes: Vec::new(),
            passive_triggers: Vec::new(),
            inventory_state: "not_owned_candidate".into(),
            provenance: "selection_only".into(),
        };
    }
    let text = tooltips.join(" ").to_ascii_lowercase();
    let mut triggers = Vec::new();
    if text.contains("at the start of each day") || text.contains("at the start of the day") {
        triggers.push("start_of_day".into());
    }
    if text.contains("while this is in your stash") || text.contains("while in your stash") {
        triggers.push("continuous_in_stash".into());
    }
    if text.contains("when you sell this") {
        triggers.push("on_sell".into());
    }
    let explicit_inventory_effect = !triggers.is_empty();
    let mut scopes = BTreeSet::new();
    if section == "board" {
        scopes.insert("board".to_owned());
    }
    if explicit_inventory_effect {
        scopes.insert("owned".to_owned());
        scopes.insert("stash".to_owned());
    }
    EffectActivation {
        activation_scopes: scopes.into_iter().collect(),
        passive_triggers: triggers,
        inventory_state: match (section, explicit_inventory_effect) {
            ("board", _) => "combat_active",
            ("stash", true) => "stash_passive_active",
            ("stash", false) => "inert_inventory_until_moved",
            _ => "owned_unplaced",
        }
        .into(),
        provenance: if explicit_inventory_effect {
            "verified_explicit_tooltip_scope"
        } else {
            "default_board_only_contract"
        }
        .into(),
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardExternalReference {
    pub provider: String,
    pub external_card_id: String,
    pub card_type: String,
    pub url: String,
    pub source_patch: String,
    pub verified_at: String,
    pub match_basis: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstance {
    pub instance_id: String,
    pub name: Option<String>,
    pub identity_status: IdentityStatus,
    pub identity_provenance: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipState {
    Active,
    Dormant,
    Broken,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipEdge {
    pub id: String,
    pub source_instance_id: String,
    pub trigger: String,
    pub target_selector: String,
    pub resolved_target_ids: Vec<String>,
    pub state: RelationshipState,
    pub reason: String,
}

fn build_relationship_graph(
    board: &[CardInstance],
    stash: &[CardInstance],
    skill_names: &[String],
) -> Vec<RelationshipEdge> {
    let weapons = board
        .iter()
        .filter(|card| card.tags.iter().any(|tag| tag == "Weapon"))
        .collect::<Vec<_>>();
    let mut edges = Vec::new();
    for card in board {
        if card.name.as_deref() == Some("Crow's Nest") {
            edges.push(RelationshipEdge {
                id: "crow_nest.exactly_one_weapon".into(),
                source_instance_id: card.instance_id.clone(),
                trigger: "continuous".into(),
                target_selector: "exactly_one_weapon".into(),
                resolved_target_ids: weapons
                    .iter()
                    .map(|weapon| weapon.instance_id.clone())
                    .collect(),
                state: if weapons.len() == 1 {
                    RelationshipState::Active
                } else {
                    RelationshipState::Broken
                },
                reason: format!("weapon_count={}", weapons.len()),
            });
        }
        if card.name.as_deref() == Some("Honing Steel") {
            let left = weapons
                .iter()
                .min_by_key(|weapon| weapon.socket.unwrap_or(u8::MAX));
            let right = weapons
                .iter()
                .max_by_key(|weapon| weapon.socket.unwrap_or(0));
            for (side, target) in [("leftmost", left), ("rightmost", right)] {
                edges.push(RelationshipEdge {
                    id: format!("honing_steel.{side}_weapon"),
                    source_instance_id: card.instance_id.clone(),
                    trigger: "on_use".into(),
                    target_selector: format!("{side}_weapon"),
                    resolved_target_ids: target
                        .into_iter()
                        .map(|weapon| weapon.instance_id.clone())
                        .collect(),
                    state: if target.is_some() {
                        RelationshipState::Active
                    } else {
                        RelationshipState::Dormant
                    },
                    reason: if target.is_some() {
                        "weapon_target_resolved".into()
                    } else {
                        "no_weapon_target".into()
                    },
                });
            }
        }
    }
    for card in stash {
        for trigger in &card.passive_triggers {
            edges.push(RelationshipEdge {
                id: format!("stash_passive.{}.{}", card.instance_id, trigger),
                source_instance_id: card.instance_id.clone(),
                trigger: trigger.clone(),
                target_selector: if trigger == "on_sell" {
                    "sale_resolution"
                } else {
                    "player_run_state"
                }
                .into(),
                resolved_target_ids: Vec::new(),
                state: RelationshipState::Active,
                reason: format!(
                    "source_section=stash;activation_scopes={}",
                    card.activation_scopes.join(",")
                ),
            });
        }
    }
    let has_haste_source = board.iter().any(|card| {
        card.hidden_tags.iter().any(|tag| tag == "Haste")
            || card
                .tooltips
                .iter()
                .any(|tooltip| tooltip.contains("Haste"))
    });
    for skill in skill_names
        .iter()
        .filter(|skill| skill.eq_ignore_ascii_case("Quick Freeze"))
    {
        edges.push(RelationshipEdge {
            id: "quick_freeze.first_haste".into(),
            source_instance_id: format!("skill:{skill}"),
            trigger: "first_haste_each_fight".into(),
            target_selector: "random_opponent_item".into(),
            resolved_target_ids: Vec::new(),
            state: if has_haste_source {
                RelationshipState::Active
            } else {
                RelationshipState::Dormant
            },
            reason: if has_haste_source {
                "board_has_haste_source".into()
            } else {
                "board_has_no_haste_source".into()
            },
        });
    }
    for skill in skill_names
        .iter()
        .filter(|skill| skill.eq_ignore_ascii_case("Diving Master"))
    {
        let leftmost = board
            .iter()
            .min_by_key(|card| card.socket.unwrap_or(u8::MAX));
        let target_ids = leftmost
            .into_iter()
            .map(|card| card.instance_id.clone())
            .collect::<Vec<_>>();
        let adds_aquatic =
            leftmost.is_some_and(|card| !card.tags.iter().any(|tag| tag == "Aquatic"));
        let reduces_cooldown = leftmost.is_some_and(|card| {
            card.attributes
                .get("CooldownMax")
                .and_then(Value::as_u64)
                .is_some_and(|cooldown| cooldown > 0)
        });
        edges.push(RelationshipEdge {
            id: "diving_master.leftmost_aquatic".into(),
            source_instance_id: format!("skill:{skill}"),
            trigger: "continuous".into(),
            target_selector: "leftmost_item".into(),
            resolved_target_ids: target_ids.clone(),
            state: if adds_aquatic {
                RelationshipState::Active
            } else {
                RelationshipState::Dormant
            },
            reason: if leftmost.is_none() {
                "no_leftmost_item".into()
            } else if adds_aquatic {
                "leftmost_item_gains_aquatic_tag".into()
            } else {
                "leftmost_item_already_aquatic".into()
            },
        });
        edges.push(RelationshipEdge {
            id: "diving_master.leftmost_cooldown".into(),
            source_instance_id: format!("skill:{skill}"),
            trigger: "continuous".into(),
            target_selector: "leftmost_item_with_cooldown".into(),
            resolved_target_ids: target_ids,
            state: if reduces_cooldown {
                RelationshipState::Active
            } else {
                RelationshipState::Dormant
            },
            reason: if leftmost.is_none() {
                "no_leftmost_item".into()
            } else if reduces_cooldown {
                "leftmost_item_has_active_cooldown".into()
            } else {
                "leftmost_item_has_no_active_cooldown".into()
            },
        });
    }
    edges
}

fn default_candidate_region(index: usize) -> NormalizedRegion {
    NormalizedRegion {
        x: 0.17 + (index.min(2) as f32 * 0.27),
        y: 0.18,
        width: 0.18,
        height: 0.34,
    }
}

fn single_loot_candidate_center_region() -> NormalizedRegion {
    NormalizedRegion {
        x: 0.41,
        y: 0.32,
        width: 0.18,
        height: 0.36,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Verified,
    Partial,
    Unknown,
    NotApplicable,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub status: CoverageStatus,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateCompleteness {
    pub board: Coverage,
    pub stash: Coverage,
    pub skills: Coverage,
    pub selection: Coverage,
    pub economy: Coverage,
}

fn build_state_completeness(
    model: &RunModel,
    board: &[CardInstance],
    _stash: &[CardInstance],
    selection: &[CardInstance],
    skills: &[SkillInstance],
    progress: Option<&ObservedProgress>,
) -> StateCompleteness {
    let board_unresolved = board
        .iter()
        .any(|card| card.identity_status == IdentityStatus::Unresolved);
    let skill_unresolved = skills
        .iter()
        .any(|skill| skill.identity_status == IdentityStatus::Unresolved);
    let selection_unresolved = selection
        .iter()
        .any(|card| card.identity_status == IdentityStatus::Unresolved);
    StateCompleteness {
        board: if model.board_snapshot_seen && !board_unresolved {
            Coverage {
                status: CoverageStatus::Verified,
                reason: "authoritative_multi_card_spawn_with_resolved_identities".into(),
            }
        } else {
            Coverage {
                status: CoverageStatus::Partial,
                reason: if board_unresolved {
                    "one_or_more_board_instances_are_unresolved"
                } else {
                    "no_authoritative_full_board_spawn_seen"
                }
                .into(),
            }
        },
        stash: Coverage {
            status: if model.stash_delta_seen {
                CoverageStatus::Partial
            } else {
                CoverageStatus::Unknown
            },
            reason: if model.stash_delta_seen {
                "player_log_exposes_incremental_stash_deltas_not_a_full_snapshot"
            } else {
                "no_stash_delta_observed"
            }
            .into(),
        },
        skills: Coverage {
            status: if skill_unresolved {
                CoverageStatus::Partial
            } else if skills.is_empty() {
                CoverageStatus::Unknown
            } else {
                CoverageStatus::Verified
            },
            reason: if skill_unresolved {
                "selected_skill_instance_requires_verified_name_mapping"
            } else if skills.is_empty() {
                "no_selected_skill_receipt_observed"
            } else {
                "selected_skill_receipts_have_verified_instance_mappings"
            }
            .into(),
        },
        selection: if selection.is_empty() {
            Coverage {
                status: CoverageStatus::NotApplicable,
                reason: "no_pending_selection".into(),
            }
        } else if selection_unresolved {
            Coverage {
                status: CoverageStatus::Partial,
                reason: "pending_candidates_require_localized_inspection".into(),
            }
        } else {
            Coverage {
                status: CoverageStatus::Verified,
                reason: "all_pending_candidates_have_exact_identities".into(),
            }
        },
        economy: Coverage {
            status: if progress.is_some() {
                CoverageStatus::Partial
            } else {
                CoverageStatus::Unknown
            },
            reason: if progress.is_some() {
                "volatile_observation_snapshot_not_reconciled_from_player_log"
            } else {
                "player_log_has_no_reliable_economy_fields"
            }
            .into(),
        },
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FusionPairEvidence {
    pub left_instance_id: String,
    pub right_instance_id: String,
    pub template_id: Option<String>,
    pub left_tier: Option<String>,
    pub right_tier: Option<String>,
    pub can_fuse: Option<bool>,
    pub status: String,
    pub resulting_tier: Option<String>,
    pub blocked_reason: Option<String>,
    pub provenance: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PedestalUpgradeCandidate {
    pub operation_instance_id: String,
    pub instance_id: String,
    pub template_id: String,
    pub from_tier: String,
    pub to_tier: String,
    pub preserves_instance_id: bool,
    pub enchantment: Option<String>,
    pub enchantment_provenance: Option<String>,
    pub provenance: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeMutationGuard {
    pub instance_id: String,
    pub upgrade_opportunity: String,
    pub blocked_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeOpportunityAssessment {
    pub inventory_status: CoverageStatus,
    pub fusion_pairs: Vec<FusionPairEvidence>,
    pub pedestal_candidates: Vec<PedestalUpgradeCandidate>,
    pub mutation_guards: Vec<UpgradeMutationGuard>,
}

fn inventory_coverage_status(board: &Coverage, stash: &Coverage) -> CoverageStatus {
    match (&board.status, &stash.status) {
        (CoverageStatus::Verified, CoverageStatus::Verified) => CoverageStatus::Verified,
        (CoverageStatus::Unknown, _) | (_, CoverageStatus::Unknown) => CoverageStatus::Unknown,
        _ => CoverageStatus::Partial,
    }
}

fn is_upgrade_item_phrase(text: &str) -> bool {
    text.trim()
        .trim_end_matches('.')
        .eq_ignore_ascii_case("upgrade an item")
}

fn is_supported_tier(tier: &str) -> bool {
    matches!(tier, "Bronze" | "Silver" | "Gold" | "Diamond")
}

fn upgrade_evidence_issue(card: &CardInstance) -> Option<&'static str> {
    if card.identity_status != IdentityStatus::Resolved
        || card.template_id.as_deref().is_none_or(str::is_empty)
        || card
            .identity_provenance
            .as_deref()
            .is_none_or(str::is_empty)
        || card.tier.as_deref().is_none_or(str::is_empty)
        || card.tier_provenance.as_deref().is_none_or(str::is_empty)
    {
        return Some("identity_template_or_tier_unresolved");
    }
    (!card.tier.as_deref().is_some_and(is_supported_tier)).then_some("unsupported_or_unknown_tier")
}

#[derive(Clone, Copy)]
struct CanonicalUpgradeItem<'a> {
    card: &'a CardInstance,
    owned: bool,
    conflicting: bool,
}

fn upgrade_evidence_conflicts(left: &CardInstance, right: &CardInstance) -> bool {
    left.template_id != right.template_id
        || left.identity_status != right.identity_status
        || left.identity_provenance != right.identity_provenance
        || left.tier != right.tier
        || left.tier_provenance != right.tier_provenance
        || left.enchantment != right.enchantment
        || left.enchantment_provenance != right.enchantment_provenance
}

fn canonical_upgrade_items<'a>(
    board: &'a [CardInstance],
    stash: &'a [CardInstance],
    selection: &'a [CardInstance],
) -> Vec<CanonicalUpgradeItem<'a>> {
    let mut items = Vec::<CanonicalUpgradeItem<'a>>::new();
    let mut indices = BTreeMap::<&str, usize>::new();
    let candidates = board
        .iter()
        .map(|card| (card, true))
        .chain(stash.iter().map(|card| (card, true)))
        .chain(
            selection
                .iter()
                .filter(|card| card.selection_category.as_deref() == Some("item"))
                .map(|card| (card, false)),
        );

    for (card, owned) in candidates {
        if let Some(index) = indices.get(card.instance_id.as_str()).copied() {
            let existing = &mut items[index];
            existing.owned |= owned;
            existing.conflicting |= upgrade_evidence_conflicts(existing.card, card);
        } else {
            indices.insert(card.instance_id.as_str(), items.len());
            items.push(CanonicalUpgradeItem {
                card,
                owned,
                conflicting: false,
            });
        }
    }
    items
}

fn fusion_pair_evidence(first: &CardInstance, second: &CardInstance) -> Option<FusionPairEvidence> {
    if first.instance_id == second.instance_id {
        return None;
    }
    let (left, right) = if first.instance_id < second.instance_id {
        (first, second)
    } else {
        (second, first)
    };
    let (Some(left_template), Some(right_template)) =
        (left.template_id.as_deref(), right.template_id.as_deref())
    else {
        return None;
    };
    if left_template != right_template {
        return None;
    }

    let left_issue = upgrade_evidence_issue(left);
    let right_issue = upgrade_evidence_issue(right);
    let (can_fuse, status, resulting_tier, blocked_reason, provenance) =
        if left_issue.is_some() || right_issue.is_some() {
            let reason = if left_issue == Some("unsupported_or_unknown_tier")
                || right_issue == Some("unsupported_or_unknown_tier")
            {
                "unsupported_or_unknown_tier"
            } else {
                "identity_template_or_tier_unresolved"
            };
            (
                None,
                "unknown",
                None,
                Some(reason.to_owned()),
                "incomplete_instance_evidence",
            )
        } else {
            let left_tier = left
                .tier
                .as_deref()
                .expect("complete upgrade evidence includes a tier");
            let right_tier = right
                .tier
                .as_deref()
                .expect("complete upgrade evidence includes a tier");
            if left_tier != right_tier {
                (
                    Some(false),
                    "not_fusible",
                    None,
                    Some("tier_mismatch".to_owned()),
                    "exact_template_match_with_tier_mismatch",
                )
            } else if left_tier == "Diamond" {
                (
                    Some(false),
                    "not_fusible",
                    None,
                    Some("maximum_tier".to_owned()),
                    "exact_template_and_tier_at_maximum",
                )
            } else {
                (
                    Some(true),
                    "direct_fusion_candidate",
                    next_tier(left_tier).map(str::to_owned),
                    None,
                    "exact_template_and_tier_match",
                )
            }
        };
    Some(FusionPairEvidence {
        left_instance_id: left.instance_id.clone(),
        right_instance_id: right.instance_id.clone(),
        template_id: Some(left_template.to_owned()),
        left_tier: left.tier.clone(),
        right_tier: right.tier.clone(),
        can_fuse,
        status: status.into(),
        resulting_tier,
        blocked_reason,
        provenance: provenance.into(),
    })
}

fn find_upgrade_operation<'a>(
    app_state: &str,
    selection: &'a [CardInstance],
) -> Option<&'a CardInstance> {
    if app_state != "PedestalState" {
        return None;
    }
    selection.iter().find(|card| {
        card.identity_status == IdentityStatus::Resolved
            && card.selection_category.as_deref() == Some("item")
            && (card.name.as_deref().is_some_and(is_upgrade_item_phrase)
                || card
                    .tooltips
                    .iter()
                    .any(|text| is_upgrade_item_phrase(text)))
    })
}

fn build_upgrade_opportunity(
    app_state: &str,
    board: &[CardInstance],
    stash: &[CardInstance],
    selection: &[CardInstance],
    board_coverage: &Coverage,
    stash_coverage: &Coverage,
) -> UpgradeOpportunityAssessment {
    let inventory_status = inventory_coverage_status(board_coverage, stash_coverage);
    let items = canonical_upgrade_items(board, stash, selection);
    let mut fusion_pairs = Vec::new();

    for left_index in 0..items.len() {
        for right_index in left_index + 1..items.len() {
            let left = items[left_index];
            let right = items[right_index];
            if (!left.owned && !right.owned) || left.conflicting || right.conflicting {
                continue;
            }
            fusion_pairs.extend(fusion_pair_evidence(left.card, right.card));
        }
    }

    let upgrade_operation = find_upgrade_operation(app_state, selection);
    let pedestal_candidates = upgrade_operation
        .map(|operation| {
            items
                .iter()
                .filter(|item| item.owned && !item.conflicting)
                .filter_map(|item| {
                    let card = item.card;
                    let template_id = card.template_id.as_ref()?;
                    let from_tier = card.tier.as_deref()?;
                    let to_tier = next_tier(from_tier)?;
                    upgrade_evidence_issue(card)
                        .is_none()
                        .then(|| PedestalUpgradeCandidate {
                            operation_instance_id: operation.instance_id.clone(),
                            instance_id: card.instance_id.clone(),
                            template_id: template_id.clone(),
                            from_tier: from_tier.into(),
                            to_tier: to_tier.into(),
                            preserves_instance_id: true,
                            enchantment: card.enchantment.clone(),
                            enchantment_provenance: card.enchantment_provenance.clone(),
                            provenance: "verified_item_operation_and_current_instance_tier".into(),
                        })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let has_conflicting_instance_evidence = items.iter().any(|item| item.conflicting);
    let has_unresolved_item_evidence = items
        .iter()
        .any(|item| upgrade_evidence_issue(item.card).is_some());
    let mutation_guards = items
        .iter()
        .map(|item| {
            let card = item.card;
            let has_pedestal_upgrade = pedestal_candidates
                .iter()
                .any(|candidate| candidate.instance_id == card.instance_id);
            let has_direct_fusion = fusion_pairs.iter().any(|pair| {
                pair.status == "direct_fusion_candidate"
                    && (pair.left_instance_id == card.instance_id
                        || pair.right_instance_id == card.instance_id)
            });
            let has_unknown_pair = fusion_pairs.iter().any(|pair| {
                pair.status == "unknown"
                    && (pair.left_instance_id == card.instance_id
                        || pair.right_instance_id == card.instance_id)
            });
            let (upgrade_opportunity, blocked_reason) = if has_conflicting_instance_evidence {
                ("unknown", Some("conflicting_duplicate_instance_evidence"))
            } else if let Some(reason) = upgrade_evidence_issue(card) {
                ("unknown", Some(reason))
            } else if has_unresolved_item_evidence {
                ("unknown", Some("peer_identity_or_tier_unresolved"))
            } else if has_pedestal_upgrade {
                (
                    "pedestal_upgrade_candidate",
                    Some("pending_pedestal_upgrade_must_be_resolved_before_sell_or_skip"),
                )
            } else if has_direct_fusion {
                (
                    "direct_fusion_candidate",
                    Some("direct_fusion_candidate_must_be_reviewed_before_sell_or_skip"),
                )
            } else if has_unknown_pair {
                (
                    "unknown",
                    Some("incomplete_pair_evidence_cannot_prove_no_fusion_candidate"),
                )
            } else {
                match stash_coverage.status {
                    CoverageStatus::Partial => (
                        "unknown",
                        Some("partial_stash_cannot_prove_no_fusion_candidate"),
                    ),
                    CoverageStatus::Unknown => (
                        "unknown",
                        Some("unknown_stash_cannot_prove_no_fusion_candidate"),
                    ),
                    _ if board_coverage.status != CoverageStatus::Verified => (
                        "unknown",
                        Some("partial_board_cannot_prove_no_fusion_candidate"),
                    ),
                    _ => ("none", None),
                }
            };
            UpgradeMutationGuard {
                instance_id: card.instance_id.clone(),
                upgrade_opportunity: upgrade_opportunity.into(),
                blocked_reason: blocked_reason.map(str::to_owned),
            }
        })
        .collect();

    UpgradeOpportunityAssessment {
        inventory_status,
        fusion_pairs,
        pedestal_candidates,
        mutation_guards,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionSupport {
    pub source_priority: Vec<String>,
    pub ten_win_role: String,
    pub mode: String,
    pub recommendation: String,
    pub pivot_rule: String,
    pub can_force_ten_win_route: bool,
    pub board_capacity: BoardCapacityStatus,
    pub storage_capacity: StorageCapacityStatus,
    pub upgrade_opportunity: UpgradeOpportunityAssessment,
    pub hard_constraints: Vec<String>,
    pub blocked_mutations: Vec<String>,
    pub gold_floor: u32,
    pub spend_budget: Option<u32>,
    pub shop_reroll_cap: u8,
    pub max_safe_opponent_level: Option<u8>,
    pub current_loss_streak: Option<u8>,
    pub current_wins: Option<u8>,
    pub wins_to_goal: Option<u8>,
    pub current_prestige: Option<u8>,
    pub max_prestige: Option<u8>,
    pub next_pvp_loss_cost: Option<u8>,
    pub prestige_after_next_pvp_loss: Option<u8>,
    pub combat_review_policy: CombatReviewPolicy,
    pub encounter_policy: EncounterDecisionPolicy,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardCapacityStatus {
    pub physical_slots: u8,
    pub unlocked_slot_ids: Option<Vec<u8>>,
    pub unlocked_capacity: Option<u8>,
    pub occupied_slots: Option<u8>,
    pub open_unlocked_slots: Option<u8>,
    pub fit_placements: Option<Vec<FitPlacement>>,
    pub unlock_provenance: String,
    pub verified: bool,
    pub capacity_gate_satisfied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitPlacement {
    pub instance_id: String,
    pub size: String,
    pub width: u8,
    pub left_socket: u8,
    pub slot_ids: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCapacityStatus {
    pub physical_slots: u8,
    pub usable_slot_ids: Option<Vec<u8>>,
    pub occupied_slot_ids: Option<Vec<u8>>,
    pub open_slot_ids: Option<Vec<u8>>,
    pub candidate_fit_placements: Option<Vec<FitPlacement>>,
    pub source_state_tick_id: Option<u64>,
    pub source_observation_id: Option<String>,
    pub provenance: String,
    pub verified: bool,
    pub reason: String,
}

impl StorageCapacityStatus {
    fn unavailable(observation: Option<&ObservedStorageObservation>, reason: &str) -> Self {
        Self {
            physical_slots: PHYSICAL_STORAGE_SLOTS,
            usable_slot_ids: None,
            occupied_slot_ids: None,
            open_slot_ids: None,
            candidate_fit_placements: None,
            source_state_tick_id: observation.and_then(|value| value.source_state_tick_id),
            source_observation_id: observation
                .and_then(|value| value.source_observation_id.clone()),
            provenance: observation
                .map(|value| value.provenance.clone())
                .unwrap_or_else(|| "no_storage_observation".into()),
            verified: false,
            reason: reason.into(),
        }
    }
}

fn card_width(size: &str) -> Option<u8> {
    match size.to_ascii_lowercase().as_str() {
        "small" => Some(1),
        "medium" => Some(2),
        "large" => Some(3),
        _ => None,
    }
}

fn build_storage_capacity(
    selection: &[CardInstance],
    observation: Option<&ObservedStorageObservation>,
    run_id: Option<&str>,
    state_tick_id: u64,
) -> StorageCapacityStatus {
    let Some(observation) = observation else {
        return StorageCapacityStatus::unavailable(None, "no_storage_observation");
    };
    if observation.verification != ObservationVerification::Verified {
        return StorageCapacityStatus::unavailable(
            Some(observation),
            "unverified_storage_observation",
        );
    }
    if run_id.is_none() || observation.run_id.as_deref() != run_id {
        return StorageCapacityStatus::unavailable(
            Some(observation),
            "storage_observation_run_id_mismatch",
        );
    }
    if observation.source_state_tick_id != Some(state_tick_id) {
        return StorageCapacityStatus::unavailable(
            Some(observation),
            "stale_storage_observation_state_tick",
        );
    }
    if observation
        .source_observation_id
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return StorageCapacityStatus::unavailable(
            Some(observation),
            "missing_storage_source_observation_id",
        );
    }

    let mut usable_slot_ids = observation.usable_slot_ids.clone();
    usable_slot_ids.sort_unstable();
    let original_usable_count = usable_slot_ids.len();
    usable_slot_ids.dedup();
    if usable_slot_ids.is_empty() || usable_slot_ids.len() != original_usable_count {
        return StorageCapacityStatus::unavailable(
            Some(observation),
            "invalid_storage_usable_slot_geometry",
        );
    }
    if usable_slot_ids
        .iter()
        .any(|slot| *slot >= PHYSICAL_STORAGE_SLOTS)
    {
        return StorageCapacityStatus::unavailable(
            Some(observation),
            "storage_slot_outside_physical_range",
        );
    }
    let usable = usable_slot_ids.iter().copied().collect::<BTreeSet<_>>();
    let mut occupied = BTreeSet::new();
    for span in &observation.occupied_spans {
        let Some(end) = span.left_socket.checked_add(span.width) else {
            return StorageCapacityStatus::unavailable(
                Some(observation),
                "invalid_storage_occupied_span",
            );
        };
        let expected = (span.left_socket..end).collect::<Vec<_>>();
        if span.width == 0
            || span.slot_ids != expected
            || span.slot_ids.iter().any(|slot| !usable.contains(slot))
            || span.slot_ids.iter().any(|slot| occupied.contains(slot))
        {
            return StorageCapacityStatus::unavailable(
                Some(observation),
                "invalid_storage_occupied_span",
            );
        }
        occupied.extend(span.slot_ids.iter().copied());
    }
    let open_slot_ids = usable.difference(&occupied).copied().collect::<Vec<_>>();
    let candidate_fit_placements = selection
        .iter()
        .filter(|card| {
            card.identity_status == IdentityStatus::Resolved
                && card.selection_category.as_deref() == Some("item")
        })
        .flat_map(|card| {
            let Some(width) = card_width(&card.size) else {
                return Vec::new();
            };
            usable_slot_ids
                .iter()
                .filter_map(|left_socket| {
                    let end = left_socket.checked_add(width)?;
                    let slot_ids = (*left_socket..end).collect::<Vec<_>>();
                    slot_ids
                        .iter()
                        .all(|slot| usable.contains(slot) && !occupied.contains(slot))
                        .then(|| FitPlacement {
                            instance_id: card.instance_id.clone(),
                            size: card.size.clone(),
                            width,
                            left_socket: *left_socket,
                            slot_ids,
                        })
                })
                .collect::<Vec<_>>()
        })
        .collect();

    StorageCapacityStatus {
        physical_slots: PHYSICAL_STORAGE_SLOTS,
        usable_slot_ids: Some(usable_slot_ids),
        occupied_slot_ids: Some(occupied.into_iter().collect()),
        open_slot_ids: Some(open_slot_ids),
        candidate_fit_placements: Some(candidate_fit_placements),
        source_state_tick_id: observation.source_state_tick_id,
        source_observation_id: observation.source_observation_id.clone(),
        provenance: observation.provenance.clone(),
        verified: true,
        reason: "verified_storage_geometry_bound_to_current_run_and_tick".into(),
    }
}

fn unlocked_slot_ids(bitmask: u16) -> Vec<u8> {
    (0..PHYSICAL_BOARD_SLOTS)
        .filter(|slot| bitmask & (1_u16 << slot) != 0)
        .collect()
}

fn build_board_capacity(
    board: &[CardInstance],
    stash: &[CardInstance],
    coverage: &Coverage,
    unlock_resolution: &BoardUnlockResolution,
) -> BoardCapacityStatus {
    let unlock_evidence = unlock_resolution.evidence.as_ref();
    let unlocked_slot_ids = unlock_evidence.map(|evidence| unlocked_slot_ids(evidence.bitmask));
    let unlocked_capacity = unlocked_slot_ids.as_ref().map(|slots| slots.len() as u8);
    let unlocked_mask = unlock_evidence.map(|evidence| evidence.bitmask);

    let mut occupied = [false; PHYSICAL_BOARD_SLOTS as usize];
    let geometry_valid = board.iter().all(|card| {
        let Some(start) = card.socket else {
            return false;
        };
        let Some(width) = card_width(&card.size) else {
            return false;
        };
        let Some(end) = start.checked_add(width) else {
            return false;
        };
        if end > PHYSICAL_BOARD_SLOTS {
            return false;
        }
        for slot in start..end {
            if unlocked_mask.is_none_or(|mask| mask & (1_u16 << slot) == 0) {
                return false;
            }
            let cell = &mut occupied[usize::from(slot)];
            if *cell {
                return false;
            }
            *cell = true;
        }
        true
    });
    let verified =
        unlock_evidence.is_some() && coverage.status == CoverageStatus::Verified && geometry_valid;
    let occupied_slots = verified.then(|| occupied.iter().filter(|value| **value).count() as u8);
    let open_unlocked_slots = occupied_slots
        .zip(unlocked_capacity)
        .map(|(occupied, unlocked)| unlocked.saturating_sub(occupied));
    let fit_placements = verified.then(|| {
        let unlocked_mask = unlocked_mask.expect("verified capacity has an unlock mask");
        stash
            .iter()
            .flat_map(|card| {
                let Some(width) = card_width(&card.size) else {
                    return Vec::new();
                };
                (0..=PHYSICAL_BOARD_SLOTS - width)
                    .filter_map(|left_socket| {
                        let slot_ids = (left_socket..left_socket + width).collect::<Vec<_>>();
                        slot_ids
                            .iter()
                            .all(|slot| {
                                unlocked_mask & (1_u16 << slot) != 0
                                    && !occupied[usize::from(*slot)]
                            })
                            .then(|| FitPlacement {
                                instance_id: card.instance_id.clone(),
                                size: card.size.clone(),
                                width,
                                left_socket,
                                slot_ids,
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    let capacity_gate_satisfied = open_unlocked_slots == Some(0);

    BoardCapacityStatus {
        physical_slots: PHYSICAL_BOARD_SLOTS,
        unlocked_slot_ids,
        unlocked_capacity,
        occupied_slots,
        open_unlocked_slots,
        fit_placements,
        unlock_provenance: unlock_resolution.reason.into(),
        verified,
        capacity_gate_satisfied,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatReviewPolicy {
    pub pvp_result_required: bool,
    pub pvp_progress_fields: Vec<String>,
    pub pve_result_required: bool,
    pub pve_review_focus: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncounterDecisionPolicy {
    pub inspection_rule: String,
    pub eligibility_rule: String,
    pub rank_priority: Vec<String>,
    pub unsafe_fallback: String,
    pub repeat_rule: String,
}

fn build_decision_support(
    relationships: &[RelationshipEdge],
    selection: &[CardInstance],
    progress: Option<&ObservedProgress>,
    policy: &DecisionPolicyConfig,
    board_capacity: BoardCapacityStatus,
    storage_capacity: StorageCapacityStatus,
    upgrade_opportunity: UpgradeOpportunityAssessment,
) -> DecisionSupport {
    let active = relationships
        .iter()
        .filter(|edge| edge.state == RelationshipState::Active)
        .count();
    let broken = relationships
        .iter()
        .filter(|edge| edge.state == RelationshipState::Broken)
        .count();
    let active_stash_passives = relationships
        .iter()
        .filter(|edge| {
            edge.id.starts_with("stash_passive.") && edge.state == RelationshipState::Active
        })
        .count();
    let loss_streak = progress.and_then(|value| value.loss_streak);
    let wins = progress.and_then(|value| value.wins);
    let prestige = progress.and_then(|value| value.prestige);
    let max_prestige = progress.and_then(|value| value.max_prestige);
    let day = progress.and_then(|value| value.day);
    let next_pvp_loss_cost = day;
    let prestige_after_next_pvp_loss = prestige
        .zip(day)
        .map(|(value, cost)| value.saturating_sub(cost));
    let critical_prestige = prestige
        .zip(day)
        .is_some_and(|(value, cost)| value <= cost.saturating_mul(2));
    let mode = if critical_prestige {
        "immediate_pvp_survival"
    } else if broken > 0 {
        "repair_or_pivot"
    } else if wins.is_some_and(|value| value >= 7) {
        "convert_current_core"
    } else if wins.is_some_and(|value| value <= 3) {
        "build_scalable_core"
    } else if active > 0 {
        "stabilize_core"
    } else {
        "evidence_gated_exploration"
    };
    let recommendation = match mode {
        "immediate_pvp_survival" => {
            "prefer_guaranteed_next_pvp_survival_and_current_core_gain_over_random_or_long_horizon_value"
        }
        "repair_or_pivot" => "repair_broken_edges_or_pivot_to_a_stronger_immediate_graph",
        "convert_current_core" => {
            "convert_the_verified_core_into_immediate_wins_without_speculative_pivots"
        }
        "build_scalable_core" => "build_a_scalable_core_while_preserving_immediate_survival_margin",
        "stabilize_core" => "compound_the_current_active_graph_without_forcing_a_named_archetype",
        _ => "collect_information_before_committing_to_an_archetype",
    };
    let mut hard_constraints = Vec::new();
    let mut blocked_mutations = vec![
        "force_ten_win_archetype_by_name_only".into(),
        "spend_below_gold_floor_without_immediate_verified_gain".into(),
        "choose_opponent_above_safe_level_without_measured_combat_margin".into(),
    ];
    if relationships.iter().any(|edge| {
        edge.id == "crow_nest.exactly_one_weapon" && edge.state == RelationshipState::Active
    }) {
        hard_constraints
            .push("preserve_exactly_one_weapon_unless_replacement_is_strictly_better".into());
        blocked_mutations.push("add_second_weapon_without_replacing_the_first".into());
    }
    let honing_targets = relationships
        .iter()
        .filter(|edge| {
            edge.id.starts_with("honing_steel.") && edge.state == RelationshipState::Active
        })
        .flat_map(|edge| edge.resolved_target_ids.iter())
        .collect::<Vec<_>>();
    if honing_targets.len() >= 2 && honing_targets.windows(2).all(|pair| pair[0] == pair[1]) {
        hard_constraints.push("preserve_double_honing_on_the_single_weapon".into());
    }
    if active_stash_passives > 0 {
        hard_constraints
            .push("preserve_active_stash_and_owned_effects_until_their_value_is_realized".into());
        blocked_mutations.push(
            "sell_item_with_active_stash_or_owned_effect_without_strict_verified_gain".into(),
        );
    }
    if selection
        .iter()
        .any(|card| card.identity_status == IdentityStatus::Unresolved)
    {
        hard_constraints.push("resolve_every_candidate_before_irreversible_selection".into());
        blocked_mutations.push("irreversible_choice_with_unresolved_candidate".into());
    }
    if !board_capacity.verified {
        hard_constraints.push("verify_board_capacity_before_player_combat".into());
        blocked_mutations.push("enter_player_combat_without_verified_board_capacity".into());
    } else if !board_capacity.capacity_gate_satisfied {
        hard_constraints.push("fill_unlocked_board_capacity_before_player_combat".into());
        blocked_mutations.push("enter_player_combat_with_open_unlocked_slots".into());
    }
    let spend_budget = progress
        .and_then(|value| value.gold)
        .map(|gold| gold.saturating_sub(policy.gold_floor));
    let max_safe_opponent_level = progress
        .and_then(|value| value.level)
        .map(|level| level.saturating_add(policy.safe_opponent_level_gap));
    let mut reasons = vec![
        format!("active_relationships={active}"),
        format!("broken_relationships={broken}"),
        format!("active_stash_passives={active_stash_passives}"),
    ];
    if let Some(value) = loss_streak {
        reasons.push(format!("loss_streak={value}"));
    }
    if let Some(value) = wins {
        reasons.push(format!("pvp_wins={value}"));
    }
    if let Some(value) = prestige {
        reasons.push(format!("prestige={value}"));
    }
    if let Some(value) = next_pvp_loss_cost {
        reasons.push(format!("next_pvp_loss_cost={value}"));
    }
    if let Some(value) = spend_budget {
        reasons.push(format!("verified_spend_budget={value}"));
    }
    if let Some(value) = board_capacity.occupied_slots {
        reasons.push(format!("board_occupied_slots={value}"));
    }
    if let Some(value) = board_capacity.open_unlocked_slots {
        reasons.push(format!("board_open_unlocked_slots={value}"));
    }
    DecisionSupport {
        source_priority: vec![
            "current_run_verified_state".into(),
            "relationship_graph".into(),
            "recent_combat_records".into(),
            "ten_win_corpus".into(),
        ],
        ten_win_role: "population_prior_not_route_authority".into(),
        mode: mode.into(),
        recommendation: recommendation.into(),
        pivot_rule: "pivot_only_when_the_immediate_verified_graph_improves_or_current_constraints_are_broken".into(),
        can_force_ten_win_route: false,
        board_capacity,
        storage_capacity,
        upgrade_opportunity,
        hard_constraints,
        blocked_mutations,
        gold_floor: policy.gold_floor,
        spend_budget,
        shop_reroll_cap: policy.shop_reroll_cap,
        max_safe_opponent_level,
        current_loss_streak: loss_streak,
        current_wins: wins,
        wins_to_goal: wins.map(|value| 10_u8.saturating_sub(value)),
        current_prestige: prestige,
        max_prestige,
        next_pvp_loss_cost,
        prestige_after_next_pvp_loss,
        combat_review_policy: CombatReviewPolicy {
            pvp_result_required: true,
            pvp_progress_fields: vec![
                "wins_before_after".into(),
                "prestige_before_after".into(),
                "day".into(),
            ],
            pve_result_required: false,
            pve_review_focus: vec![
                "measured_output_deficits".into(),
                "loot_identity".into(),
                "reward_chain_completion".into(),
            ],
        },
        encounter_policy: EncounterDecisionPolicy {
            inspection_rule: "resolve_each_candidate_once_before_selection".into(),
            eligibility_rule:
                "opponent_level_at_or_below_max_safe_unless_recent_combat_proves_margin".into(),
            rank_priority: if mode == "immediate_pvp_survival" {
                vec![
                    "guaranteed_next_pvp_survival_gain".into(),
                    "immediate_active_relationship_gain".into(),
                    "recent_pvp_deficit_repair".into(),
                    "reward_value".into(),
                ]
            } else {
                vec![
                    "immediate_active_relationship_gain".into(),
                    "recent_pvp_deficit_repair".into(),
                    "survival_margin".into(),
                    "reward_value".into(),
                ]
            },
            unsafe_fallback: "choose_lowest_measured_risk_when_no_candidate_is_eligible".into(),
            repeat_rule: "do_not_reinspect_unchanged_candidate_state".into(),
        },
        reasons,
    }
}

fn build_available_actions(
    app_state: &str,
    selection: &[CardInstance],
    unresolved_regions: &[UnresolvedRegion],
    board_capacity: &BoardCapacityStatus,
    allow_discard_current_choice: bool,
) -> Vec<String> {
    let mut actions = vec!["observe".into()];
    if !unresolved_regions.is_empty() {
        actions.push("inspect_unresolved_region".into());
    }
    if !selection.is_empty() {
        actions.push("inspect_candidate".into());
        if unresolved_regions.is_empty() {
            actions.push("select_candidate".into());
        }
    }
    if matches!(app_state, "LevelUpState" | "LootState" | "PedestalState") {
        actions.push("verify_pending_reward".into());
    }
    if app_state == "EncounterState" && allow_discard_current_choice {
        actions.push("discard_current_choice".into());
    }
    if app_state == "ChoiceState" {
        if board_capacity.verified
            && !board_capacity.capacity_gate_satisfied
            && board_capacity
                .fit_placements
                .as_ref()
                .is_some_and(|placements| !placements.is_empty())
        {
            actions.push("fill_board_from_stash_before_player_combat".into());
        }
        actions.push("inspect_encounter".into());
        if unresolved_regions.is_empty() {
            actions.push("select_encounter".into());
        }
    }
    actions
}

fn choice_kind_for_app_state(app_state: &str) -> &'static str {
    match app_state {
        "ChoiceState" => "encounter",
        "LootState" => "loot_reward",
        "LevelUpState" => "level_up_reward",
        "PedestalState" => "item_operation",
        "EncounterState" => "event_option",
        _ => "none",
    }
}

fn selection_section_for_app_state(app_state: &str) -> &'static str {
    match choice_kind_for_app_state(app_state) {
        "encounter" => "encounter_option",
        "loot_reward" => "loot_reward_option",
        "level_up_reward" => "level_up_reward_option",
        "item_operation" => "item_operation_option",
        "event_option" => "event_option",
        _ => "selection_option",
    }
}

fn selection_category(section: &str, instance_id: &str) -> Option<&'static str> {
    if section == "encounter_option" {
        return Some("encounter_entry");
    }
    if !section.ends_with("_option") {
        return None;
    }
    if instance_id.starts_with("skl_") {
        Some("skill")
    } else if instance_id.starts_with("itm_") {
        Some("item")
    } else if instance_id.starts_with("enc_") {
        Some("reward_event")
    } else if instance_id.starts_with("ste_") {
        Some("event_control")
    } else {
        Some("unknown")
    }
}

fn right_click_behavior(section: &str, instance_id: &str) -> Option<&'static str> {
    let category = selection_category(section, instance_id)?;
    match category {
        "encounter_entry" | "reward_event" => Some("preview"),
        "skill" => Some("not_a_selection_contract"),
        _ => Some("unknown_do_not_use_for_inspection"),
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    #[serde(rename = "tickId")]
    pub tick_id: u64,
    pub provenance: Provenance,
    pub run: RunState,
    pub board_items: Vec<CardInstance>,
    pub chest_items: Vec<CardInstance>,
    pub selection_options: Vec<CardInstance>,
    pub player_skills: Vec<String>,
    pub player_skill_instances: Vec<SkillInstance>,
    pub relationship_graph: Vec<RelationshipEdge>,
    pub build_evidence: Option<BuildEvidence>,
    pub state_completeness: StateCompleteness,
    pub decision_support: DecisionSupport,
    pub unresolved_regions: Vec<UnresolvedRegion>,
    pub candidate_actionable_regions: Vec<CandidateActionableRegion>,
    pub choice_fence: Option<ChoiceFence>,
    pub available_actions: Vec<String>,
    pub placement_receipts: Vec<PlacementReceipt>,
    pub purchase_receipts: Vec<PurchaseReceipt>,
    pub inventory_mutation_receipts: Vec<InventoryMutationReceipt>,
    pub decision_outcomes: Vec<DecisionReceipt>,
    pub reward_outcomes: Vec<RewardOutcome>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub player_log: SourceProvenance,
    pub game_data: GameDataProvenance,
    pub card_identity_cache: Option<CardIdentityCacheProvenance>,
    pub candidate_identity: CandidateIdentityProvenance,
    pub visual_fallback: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProvenance {
    pub path: PathBuf,
    pub mode: String,
    pub cursor: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDataProvenance {
    pub path: PathBuf,
    pub mode: String,
    pub indexed_templates: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardIdentityCacheProvenance {
    pub path: PathBuf,
    pub mode: String,
    pub loaded_records: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub app_state: String,
    pub run_id: Option<String>,
    pub state_tick_id: u64,
    pub progress: Option<ObservedProgress>,
    pub progress_status: String,
    pub progress_reason: String,
    pub pending_choice: bool,
    pub choice_kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedRegion {
    pub instance_id: String,
    pub reason: String,
    pub visual_scope: String,
    pub region_hint: String,
    pub crop: NormalizedRegion,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateActionableRegion {
    pub instance_id: String,
    pub identity_status: IdentityStatus,
    pub primary_gesture: String,
    pub region_hint: String,
    pub region_source: String,
    pub requires_fresh_observation_fence: bool,
    pub crop: NormalizedRegion,
}

pub struct IncrementalLog {
    path: PathBuf,
    offset: u64,
    remainder: String,
    checkpoint_start: u64,
    checkpoint: Vec<u8>,
}

impl IncrementalLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            remainder: String::new(),
            checkpoint_start: 0,
            checkpoint: Vec::new(),
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn poll(&mut self, model: &mut RunModel) -> Result<bool, CompanionError> {
        let length = self.path.metadata()?.len();
        let replaced = self.offset > length || !self.checkpoint_matches()?;
        if replaced {
            *model = RunModel::default();
            self.offset = 0;
            self.remainder.clear();
            self.checkpoint.clear();
        }
        let before_tick = model.tick;
        let mut file = File::open(&self.path)?;
        let read_start = self.offset;
        file.seek(SeekFrom::Start(read_start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.offset = self.offset.saturating_add(bytes.len() as u64);
        let combined_start = read_start.saturating_sub(self.remainder.len() as u64);
        let combined = format!("{}{}", self.remainder, String::from_utf8_lossy(&bytes));
        if combined.ends_with('\n') || combined.ends_with('\r') {
            model.ingest_text_at(combined_start, &combined);
            self.remainder.clear();
        } else if let Some(split) = combined.rfind('\n') {
            model.ingest_text_at(combined_start, &combined[..=split]);
            self.remainder = combined[split + 1..].to_owned();
        } else {
            self.remainder = combined;
        }
        self.refresh_checkpoint()?;
        Ok(model.tick != before_tick)
    }

    fn checkpoint_matches(&self) -> Result<bool, CompanionError> {
        if self.checkpoint.is_empty() {
            return Ok(true);
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.checkpoint_start))?;
        let mut current = vec![0; self.checkpoint.len()];
        file.read_exact(&mut current)?;
        Ok(current == self.checkpoint)
    }

    fn refresh_checkpoint(&mut self) -> Result<(), CompanionError> {
        let length = self.offset.min(64);
        self.checkpoint_start = self.offset.saturating_sub(length);
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.checkpoint_start))?;
        self.checkpoint = vec![0; length as usize];
        file.read_exact(&mut self.checkpoint)?;
        Ok(())
    }
}

pub struct CompanionRuntime {
    config: CompanionConfig,
    config_path: Option<PathBuf>,
    config_bytes: Option<Vec<u8>>,
    index: TemplateIndex,
    card_identity_cache: Option<CardIdentityCache>,
    ten_win_corpus: Option<TenWinCorpus>,
    log: IncrementalLog,
    model: RunModel,
    candidate_identity_resolver: CandidateIdentityResolver,
    candidate_identity_snapshot: CandidateIdentitySnapshot,
    last_model_tick: u64,
    published_tick: u64,
}

impl CompanionRuntime {
    pub fn new(config: CompanionConfig) -> Result<Self, CompanionError> {
        Self::build(config, None, None)
    }

    pub fn from_config_path(path: &Path) -> Result<Self, CompanionError> {
        let bytes = fs::read(path)?;
        let config = serde_json::from_slice::<CompanionConfig>(&bytes)?;
        Self::build(config, Some(path.to_path_buf()), Some(bytes))
    }

    fn build(
        config: CompanionConfig,
        config_path: Option<PathBuf>,
        config_bytes: Option<Vec<u8>>,
    ) -> Result<Self, CompanionError> {
        let index = TemplateIndex::load(&config.database_path)?;
        let card_identity_cache = config
            .card_identity_cache_path
            .as_ref()
            .map(|path| CardIdentityCache::load(path, &index))
            .transpose()?;
        let ten_win_corpus = config
            .ten_win_corpus_path
            .as_ref()
            .map(|path| {
                let value = serde_json::from_slice::<Value>(&std::fs::read(path)?)?;
                TenWinCorpus::parse_value(value).map_err(CompanionError::InvalidTenWinCorpus)
            })
            .transpose()?;
        let log = IncrementalLog::new(config.log_path.clone());
        let candidate_identity_resolver =
            CandidateIdentityResolver::new(config.candidate_identity_provider.as_ref());
        Ok(Self {
            config,
            config_path,
            config_bytes,
            index,
            card_identity_cache,
            ten_win_corpus,
            log,
            model: RunModel::default(),
            candidate_identity_resolver,
            candidate_identity_snapshot: CandidateIdentitySnapshot::disabled(),
            last_model_tick: u64::MAX,
            published_tick: 0,
        })
    }

    pub fn poll(&mut self) -> Result<Context, CompanionError> {
        let config_changed = self.refresh_config()?;
        if config_changed {
            self.candidate_identity_resolver =
                CandidateIdentityResolver::new(self.config.candidate_identity_provider.as_ref());
        }
        self.log.poll(&mut self.model)?;
        let identity_fence = self.model.candidate_identity_fence();
        let candidate_identity_snapshot = self
            .candidate_identity_resolver
            .resolve(&identity_fence, &self.index);
        let identity_changed = candidate_identity_snapshot != self.candidate_identity_snapshot;
        if config_changed || self.last_model_tick != self.model.tick || identity_changed {
            self.published_tick = self.published_tick.saturating_add(1);
            self.last_model_tick = self.model.tick;
        }
        self.candidate_identity_snapshot = candidate_identity_snapshot;
        let mut context = self.model.context_with_candidate_identity(
            &self.index,
            &self.config,
            self.log.offset(),
            self.ten_win_corpus.as_ref(),
            self.card_identity_cache.as_ref(),
            &self.candidate_identity_snapshot,
        );
        context.tick_id = self.published_tick;
        Ok(context)
    }

    fn refresh_config(&mut self) -> Result<bool, CompanionError> {
        let Some(path) = self.config_path.as_ref() else {
            return Ok(false);
        };
        let bytes = fs::read(path)?;
        if self.config_bytes.as_deref() == Some(bytes.as_slice()) {
            return Ok(false);
        }
        let refreshed = serde_json::from_slice::<CompanionConfig>(&bytes)?;
        if refreshed.log_path != self.config.log_path
            || refreshed.database_path != self.config.database_path
            || refreshed.listen != self.config.listen
            || refreshed.ten_win_corpus_path != self.config.ten_win_corpus_path
            || refreshed.card_identity_cache_path != self.config.card_identity_cache_path
        {
            return Err(CompanionError::InvalidConfigReload(
                "logPath, databasePath, listen, tenWinCorpusPath, and cardIdentityCachePath require a companion restart".into(),
            ));
        }
        self.config = refreshed;
        self.config_bytes = Some(bytes);
        Ok(true)
    }

    pub fn listen(&self) -> &str {
        &self.config.listen
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn template(
        id: &str,
        name: &str,
        size: &str,
        tags: &[&str],
        hidden: &[&str],
    ) -> TemplateSummary {
        TemplateSummary {
            id: id.into(),
            version: "1.0.0".into(),
            name: name.into(),
            starting_tier: "Bronze".into(),
            size: size.into(),
            tags: tags.iter().map(|value| (*value).to_owned()).collect(),
            hidden_tags: hidden.iter().map(|value| (*value).to_owned()).collect(),
            tooltips: Vec::new(),
            tier_attributes: BTreeMap::from([("Bronze".into(), json!({}))]),
        }
    }

    fn empty_upgrade_opportunity() -> UpgradeOpportunityAssessment {
        let verified = Coverage {
            status: CoverageStatus::Verified,
            reason: "test".into(),
        };
        build_upgrade_opportunity("ChoiceState", &[], &[], &[], &verified, &verified)
    }

    #[test]
    fn incremental_log_builds_resolved_board_and_localized_unknown_candidates() {
        let mut model = RunModel::default();
        let log = concat!(
            "[01:00:00.000] [AppState] State changed from [ChoiceState] to [EncounterState]\n",
            "[01:00:01.000] [BoardManager] Card Purchased: InstanceId: patch - TemplateIdtpl-patch - Target:PlayerSocket_0 - SectionPlayer\n",
            "[01:00:02.000] [BoardManager] Card Purchased: InstanceId: cutlass - TemplateIdtpl-cutlass - Target:PlayerSocket_8 - SectionPlayer\n",
            "[01:00:03.000] [GameSimHandler] Cards Spawned: [patch [Player] [Hand] [Socket_0] [Small] | [cutlass [Player] [Hand] [Socket_8] [Medium] | \n",
            "[01:00:04.000] [GameSimHandler] Cards Dealt: [offer-a [Medium] | [offer-b [Small] | [offer-c [Large] | \n",
        );
        model.ingest_text(log);
        let index = TemplateIndex::from_templates([
            template("tpl-patch", "Patch", "Small", &["Friend"], &["Heal"]),
            template("tpl-cutlass", "Cutlass", "Medium", &["Weapon"], &["Damage"]),
        ]);
        let config = CompanionConfig::test_default();

        let context = model.context(&index, &config, 123, None, None);

        assert_eq!(context.run.app_state, "EncounterState");
        assert_eq!(context.board_items.len(), 2);
        assert_eq!(context.board_items[0].name.as_deref(), Some("Patch"));
        assert_eq!(context.selection_options.len(), 3);
        assert!(
            context
                .selection_options
                .iter()
                .all(|card| card.identity_status == IdentityStatus::Unresolved)
        );
        assert_eq!(context.unresolved_regions.len(), 3);
        assert!(
            context
                .unresolved_regions
                .iter()
                .all(|region| region.visual_scope == "localized_card_only")
        );
    }

    #[test]
    fn relationship_graph_keeps_both_honing_edges_on_one_weapon() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[01:00:01.000] [BoardManager] Card Purchased: InstanceId: nest - TemplateIdtpl-nest - Target:PlayerSocket_2 - SectionPlayer\n",
            "[01:00:02.000] [BoardManager] Card Purchased: InstanceId: steel - TemplateIdtpl-steel - Target:PlayerSocket_5 - SectionPlayer\n",
            "[01:00:03.000] [BoardManager] Card Purchased: InstanceId: cutlass - TemplateIdtpl-cutlass - Target:PlayerSocket_8 - SectionPlayer\n",
            "[01:00:04.000] [GameSimHandler] Cards Spawned: [nest [Player] [Hand] [Socket_2] [Large] | [steel [Player] [Hand] [Socket_5] [Small] | [cutlass [Player] [Hand] [Socket_8] [Medium] | \n",
        ));
        let index = TemplateIndex::from_templates([
            template("tpl-nest", "Crow's Nest", "Large", &["Property"], &["Crit"]),
            template(
                "tpl-steel",
                "Honing Steel",
                "Small",
                &["Tool"],
                &["DamageReference"],
            ),
            template("tpl-cutlass", "Cutlass", "Medium", &["Weapon"], &["Damage"]),
        ]);

        let context = model.context(&index, &CompanionConfig::test_default(), 123, None, None);
        let graph = context.relationship_graph;

        assert!(
            graph
                .iter()
                .any(|edge| edge.id == "crow_nest.exactly_one_weapon"
                    && edge.state == RelationshipState::Active)
        );
        let honing = graph
            .iter()
            .filter(|edge| edge.id.starts_with("honing_steel."))
            .collect::<Vec<_>>();
        assert_eq!(honing.len(), 2);
        assert!(
            honing
                .iter()
                .all(|edge| edge.state == RelationshipState::Active)
        );
        assert!(
            honing
                .iter()
                .all(|edge| edge.resolved_target_ids == ["cutlass"])
        );
    }

    #[test]
    fn stash_effects_require_explicit_scope_and_become_active_relationships() {
        let passive = classify_effect_activation(
            "stash",
            &["At the start of each day, get a Small Aquatic item".into()],
        );
        assert_eq!(passive.activation_scopes, ["owned", "stash"]);
        assert_eq!(passive.passive_triggers, ["start_of_day"]);
        assert_eq!(passive.inventory_state, "stash_passive_active");

        let inert = classify_effect_activation("stash", &["Your Weapons have +10 Damage".into()]);
        assert!(inert.activation_scopes.is_empty());
        assert!(inert.passive_triggers.is_empty());
        assert_eq!(inert.inventory_state, "inert_inventory_until_moved");
    }

    #[test]
    fn diving_master_requires_a_leftmost_item_with_an_active_cooldown() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[01:00:01.000] [BoardManager] Card Purchased: InstanceId: nest - TemplateIdtpl-nest - Target:PlayerSocket_0 - SectionPlayer\n",
            "[01:00:02.000] [BoardManager] Card Purchased: InstanceId: flagship - TemplateIdtpl-flagship - Target:PlayerSocket_3 - SectionPlayer\n",
            "[01:00:03.000] [GameSimHandler] Cards Spawned: [nest [Player] [Hand] [Socket_0] [Large] | [flagship [Player] [Hand] [Socket_3] [Large] | \n",
        ));
        let mut flagship = template(
            "tpl-flagship",
            "Flagship",
            "Large",
            &["Aquatic", "Weapon"],
            &["Damage"],
        );
        flagship
            .tier_attributes
            .insert("Bronze".into(), json!({"CooldownMax": 5000}));
        let index = TemplateIndex::from_templates([
            template(
                "tpl-nest",
                "Crow's Nest",
                "Large",
                &["Aquatic", "Property"],
                &["Crit"],
            ),
            flagship,
        ]);
        let mut config = CompanionConfig::test_default();
        config.skill_names.push("Diving Master".into());

        let before = model.context(&index, &config, 123, None, None);
        assert!(before.relationship_graph.iter().any(|edge| {
            edge.id == "diving_master.leftmost_cooldown"
                && edge.state == RelationshipState::Dormant
                && edge.resolved_target_ids == ["nest"]
        }));

        model.ingest_text(concat!(
            "[01:00:04.000] [CardOperationUtility] Successfully moved card flagship to Socket_0\n",
            "[01:00:05.000] [CardOperationUtility] Successfully moved card nest to Socket_3\n",
        ));
        let after = model.context(&index, &config, 124, None, None);
        assert!(after.relationship_graph.iter().any(|edge| {
            edge.id == "diving_master.leftmost_cooldown"
                && edge.state == RelationshipState::Active
                && edge.resolved_target_ids == ["flagship"]
        }));
    }

    #[test]
    fn start_run_state_clears_positions_from_previous_runs() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [GameSimEventSocketsUnlocked] UnlockedSocketsBitmask: 1023\n",
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: old - TemplateIdtpl-old - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:00.500] [CardOperationUtility] Successfully moved card old to Socket_1\n",
            "[01:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[01:00:01.000] [BoardManager] Card Purchased: InstanceId: current - TemplateIdtpl-current - Target:PlayerSocket_8 - SectionPlayer\n",
        ));

        assert!(!model.player_items.contains_key("old"));
        assert!(model.player_items.contains_key("current"));
        assert!(model.template_ids.contains_key("old"));
        assert!(model.board_unlock_evidence.is_none());
        assert!(model.placement_receipts.is_empty());
    }

    #[test]
    fn progress_from_a_previous_run_cannot_drive_current_decisions() {
        let mut model = RunModel::default();
        model.ingest_text(
            "[01:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
        );
        let previous_run_id = model.run_id.clone().expect("first run id");
        let mut config = CompanionConfig::test_default();
        config.current_progress = Some(ObservedProgress {
            run_id: Some(previous_run_id),
            day: Some(8),
            hour: Some(1),
            level: Some(8),
            game_build: None,
            source_state_tick_id: None,
            verification: ObservationVerification::Unverified,
            health: Some(2240),
            max_health: Some(2240),
            gold: Some(53),
            wins: Some(2),
            losses: Some(4),
            loss_streak: Some(1),
            prestige: Some(3),
            max_prestige: Some(20),
            source_observation_id: Some("old-observation".into()),
            provenance: "verified_previous_run".into(),
        });
        assert!(
            model
                .context(&TemplateIndex::default(), &config, 10, None, None)
                .run
                .progress
                .is_some(),
            "progress bound to the current run should be usable"
        );

        model.ingest_text(concat!(
            "[01:00:01.000] [AppState] State changed from [StartRunAppState] to [ChoiceState]\n",
            "[02:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
        ));
        let context = model.context(&TemplateIndex::default(), &config, 20, None, None);

        assert_ne!(
            context.run.run_id,
            config.current_progress.as_ref().unwrap().run_id
        );
        assert!(
            context.run.progress.is_none(),
            "stale progress must fail closed instead of steering the next run"
        );
        assert_eq!(
            context.state_completeness.economy.status,
            CoverageStatus::Unknown
        );
        assert_eq!(context.decision_support.spend_budget, None);
    }

    #[test]
    fn multi_card_player_spawn_replaces_the_board_but_preserves_stash() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: old - TemplateIdtpl-old - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: stash - TemplateIdtpl-stash - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [current-a [Player] [Hand] [Socket_0] [Small] | [current-b [Player] [Hand] [Socket_1] [Small] | \n",
        ));

        assert!(!model.player_items.contains_key("old"));
        assert!(model.player_items.contains_key("stash"));
        assert!(model.player_items.contains_key("current-a"));
        assert!(model.player_items.contains_key("current-b"));
    }

    #[test]
    fn inventory_removal_receipt_evicts_the_exact_instance() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: sold - TemplateIdtpl-sold - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[00:00:01.000] [CardOperationUtility] Successfully removed item sold from player's inventory\n",
        ));

        assert!(!model.player_items.contains_key("sold"));
    }

    #[test]
    fn repeat_purchase_advances_the_owned_instance_tier() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: steel - TemplateIdtpl-steel - Target:PlayerSocket_3 - SectionPlayer\n",
            "[00:00:00.500] [GameSimHandler] Cards Dealt: [itm_steel_duplicate [Small] | [itm_other [Small] | \n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: steel - TemplateIdtpl-steel - Target:PlayerSocket_3 - SectionPlayer\n",
        ));
        let mut steel = template(
            "tpl-steel",
            "Honing Steel",
            "Small",
            &["Tool"],
            &["DamageReference"],
        );
        steel
            .tier_attributes
            .insert("Silver".into(), json!({"Custom_0": 10}));
        let index = TemplateIndex::from_templates([steel]);
        let mut config = CompanionConfig::test_default();
        config.instance_overrides.insert(
            "steel".into(),
            InstanceOverride {
                tier: Some("Bronze".into()),
                provenance: "old_visual_snapshot".into(),
                ..InstanceOverride::default()
            },
        );

        let context = model.context(&index, &config, 123, None, None);
        let steel = &context.board_items[0];

        assert_eq!(steel.tier.as_deref(), Some("Silver"));
        assert_eq!(
            steel.tier_provenance.as_deref(),
            Some("player_log_repeat_purchase_upgrade")
        );
        assert_eq!(steel.attributes, json!({"Custom_0": 10}));
    }

    #[test]
    fn selected_skills_and_current_progress_drive_immediate_pvp_survival_policy() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.000] [AppState] Selected skill skill-vitality to socket SkillSocket_0\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: nest - TemplateIdtpl-nest - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [BoardManager] Card Purchased: InstanceId: steel - TemplateIdtpl-steel - Target:PlayerSocket_3 - SectionPlayer\n",
            "[00:00:03.000] [BoardManager] Card Purchased: InstanceId: flagship - TemplateIdtpl-flagship - Target:PlayerSocket_7 - SectionPlayer\n",
            "[00:00:04.000] [GameSimHandler] Cards Spawned: [nest [Player] [Hand] [Socket_0] [Large] | [steel [Player] [Hand] [Socket_3] [Small] | [flagship [Player] [Hand] [Socket_7] [Large] | \n",
        ));
        let index = TemplateIndex::from_templates([
            template("tpl-nest", "Crow's Nest", "Large", &["Property"], &[]),
            template("tpl-steel", "Honing Steel", "Small", &["Tool"], &[]),
            template("tpl-flagship", "Flagship", "Large", &["Weapon"], &[]),
        ]);
        let mut config = CompanionConfig::test_default();
        config
            .skill_instance_overrides
            .insert("skill-vitality".into(), "Vitality Surge".into());
        config.current_progress = Some(ObservedProgress {
            run_id: model.run_id.clone(),
            day: Some(7),
            hour: Some(2),
            level: Some(7),
            game_build: None,
            source_state_tick_id: None,
            verification: ObservationVerification::Unverified,
            health: Some(1650),
            max_health: Some(1650),
            gold: Some(44),
            wins: Some(2),
            losses: Some(3),
            loss_streak: Some(0),
            prestige: Some(10),
            max_prestige: Some(20),
            source_observation_id: Some("345".into()),
            provenance: "test_observation".into(),
        });

        let context = model.context(&index, &config, 123, None, None);

        assert!(
            context
                .player_skills
                .iter()
                .any(|skill| skill == "Vitality Surge")
        );
        assert_eq!(
            context.state_completeness.board.status,
            CoverageStatus::Verified
        );
        assert_eq!(
            context.state_completeness.economy.status,
            CoverageStatus::Partial
        );
        assert_eq!(context.decision_support.mode, "immediate_pvp_survival");
        assert_eq!(context.decision_support.spend_budget, Some(14));
        assert_eq!(context.decision_support.max_safe_opponent_level, Some(9));
        assert!(!context.decision_support.can_force_ten_win_route);
        assert_eq!(
            context.decision_support.encounter_policy.rank_priority[0],
            "guaranteed_next_pvp_survival_gain"
        );
        assert_eq!(context.decision_support.current_wins, Some(2));
        assert_eq!(context.decision_support.wins_to_goal, Some(8));
        assert_eq!(context.decision_support.current_prestige, Some(10));
        assert_eq!(context.decision_support.next_pvp_loss_cost, Some(7));
        assert_eq!(
            context.decision_support.prestige_after_next_pvp_loss,
            Some(3)
        );
        assert!(
            !context
                .decision_support
                .combat_review_policy
                .pve_result_required
        );
        assert_eq!(
            context.decision_support.encounter_policy.unsafe_fallback,
            "choose_lowest_measured_risk_when_no_candidate_is_eligible"
        );
        assert!(
            context
                .decision_support
                .blocked_mutations
                .iter()
                .any(|mutation| mutation == "add_second_weapon_without_replacing_the_first")
        );
    }

    #[test]
    fn unresolved_candidates_block_irreversible_selection() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [unknown [Medium] | \n",
        ));

        let context = model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );

        assert_eq!(
            context.state_completeness.selection.status,
            CoverageStatus::Partial
        );
        assert!(
            context
                .available_actions
                .iter()
                .any(|action| action == "inspect_candidate")
        );
        assert!(
            !context
                .available_actions
                .iter()
                .any(|action| action == "select_candidate")
        );
        assert!(
            context
                .decision_support
                .blocked_mutations
                .iter()
                .any(|mutation| mutation == "irreversible_choice_with_unresolved_candidate")
        );
        assert_eq!(context.run.choice_kind, "encounter");
        assert_eq!(context.selection_options[0].section, "encounter_option");
        assert_eq!(
            context.selection_options[0].selection_category.as_deref(),
            Some("encounter_entry")
        );
        assert_eq!(
            context.selection_options[0].right_click_behavior.as_deref(),
            Some("preview")
        );
    }

    #[test]
    fn nested_skill_choice_exposes_left_click_as_the_only_selection_contract() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [LevelUpState] to [EncounterState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [skl_choice [Medium] | \n",
        ));

        let context = model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );

        assert_eq!(context.run.choice_kind, "event_option");
        assert_eq!(context.selection_options[0].section, "event_option");
        assert_eq!(
            context.selection_options[0].selection_category.as_deref(),
            Some("skill")
        );
        let selection = serde_json::to_value(&context.selection_options[0])
            .expect("skill selection serializes");
        assert_eq!(selection["primaryGesture"], "left_click");
        assert_eq!(selection["rightClickBehavior"], "not_a_selection_contract");
    }

    #[test]
    fn ownerless_finished_message_cannot_own_a_choice_source() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [AppState] State changed from [StartRunAppState] to [EncounterState]\n",
            "[00:00:00.200] [GameSimHandler] Cards Dealt: [itm_ownerless [Small] | \n",
            "[00:00:00.300] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [ownerless]\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("ownerless choice context serializes");

        assert_eq!(
            context["selectionOptions"][0]["instanceId"],
            "itm_ownerless"
        );
        assert_eq!(context["choiceFence"], Value::Null);
        assert!(
            !context["availableActions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|action| action == "discard_current_choice")
        );
    }

    #[test]
    fn event_option_exit_publishes_a_fenced_discard_receipt_for_an_unresolved_candidate() {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let exit_line = "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, after_exit) = fixture
            .split_once(exit_line)
            .expect("fixture contains the explicit discard request");
        let mut model = RunModel::default();
        model.ingest_text(before_exit);

        let before = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("pre-discard context serializes");
        let run_id = before["run"]["runId"]
            .as_str()
            .expect("fixture starts a concrete run")
            .to_owned();
        let source_tick = before["run"]["stateTickId"]
            .as_u64()
            .expect("choice has a semantic state tick");

        assert_eq!(before["run"]["choiceKind"], "event_option");
        assert_eq!(before["selectionOptions"][0]["instanceId"], "itm_gKj7HU5");
        assert_eq!(
            before["selectionOptions"][0]["identityStatus"],
            "unresolved"
        );
        assert_eq!(before["choiceFence"]["runId"], run_id);
        assert_eq!(before["choiceFence"]["stateTickId"], source_tick);
        assert_eq!(before["choiceFence"]["selectionMessageId"], "4FQ");
        assert_eq!(before["choiceFence"]["choiceKind"], "event_option");
        assert_eq!(
            before["choiceFence"]["candidateInstanceIds"],
            json!(["itm_gKj7HU5"])
        );
        assert!(
            !before["availableActions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|action| action == "select_candidate")
        );
        assert!(
            before["availableActions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|action| action == "discard_current_choice")
        );

        model.ingest_text(&format!("{exit_line}{after_exit}"));
        let after = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            456,
            None,
            None,
        ))
        .expect("post-discard context serializes");
        let receipt = &after["decisionOutcomes"][0];

        assert_eq!(receipt["choiceFence"]["runId"], run_id);
        assert_eq!(receipt["choiceFence"]["stateTickId"], source_tick);
        assert_eq!(receipt["choiceFence"]["selectionMessageId"], "4FQ");
        assert_eq!(receipt["sourceSelectionMessageId"], "4FQ");
        assert_eq!(receipt["commitMessageId"], "next");
        assert_eq!(receipt["choiceFence"]["choiceKind"], "event_option");
        assert_eq!(
            receipt["choiceFence"]["candidateInstanceIds"],
            json!(["itm_gKj7HU5"])
        );
        assert_eq!(receipt["requestedAction"], "discard");
        assert_eq!(receipt["status"], "discarded");
        assert_eq!(receipt["finalized"], true);
        assert_eq!(receipt["evidence"]["exitCommandSent"], true);
        assert_eq!(receipt["evidence"]["exitCommandResponse"], true);
        assert_eq!(receipt["evidence"]["choiceTransitionSeen"], true);
        assert_eq!(receipt["evidence"]["exactCandidatesDisposed"], true);
        assert_eq!(receipt["evidence"]["candidatePurchaseSeen"], false);
        assert_eq!(after["rewardOutcomes"], json!([]));
    }

    #[test]
    fn ownerless_finished_message_cannot_commit_an_observed_discard() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [source]\n",
            "[00:00:00.200] [AppState] State changed from [StartRunAppState] to [EncounterState]\n",
            "[00:00:00.201] [GameSimHandler] Cards Dealt: [itm_fenced [Small] | \n",
            "[00:00:00.202] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [source]\n",
            "[00:00:01.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
            "[00:00:01.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
            "[00:00:01.101] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[00:00:01.102] [GameSimHandler] Cards Dealt: [enc_next [Medium] | \n",
            "[00:00:01.103] [GameSimHandler] Cards Disposed: itm_fenced |\n",
            "[00:00:01.104] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [noise]\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("ownerless discard context serializes");
        let receipt = &context["decisionOutcomes"][0];

        assert_eq!(receipt["sourceSelectionMessageId"], "source");
        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["finalized"], false);
        assert_eq!(receipt["commitMessageId"], Value::Null);
        assert_eq!(receipt["evidence"]["exactCandidatesDisposed"], false);
        assert_eq!(receipt["evidence"]["batchConflicted"], true);
    }

    #[test]
    fn processing_after_a_relevant_transition_cannot_retroactively_own_the_batch() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [source]\n",
            "[00:00:00.200] [AppState] State changed from [StartRunAppState] to [EncounterState]\n",
            "[00:00:00.201] [GameSimHandler] Cards Dealt: [itm_fenced [Small] | \n",
            "[00:00:00.202] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [source]\n",
            "[00:00:01.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
            "[00:00:01.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
            "[00:00:01.101] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[00:00:01.102] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [late]\n",
            "[00:00:01.103] [GameSimHandler] Cards Dealt: [enc_next [Medium] | \n",
            "[00:00:01.104] [GameSimHandler] Cards Disposed: itm_fenced |\n",
            "[00:00:01.105] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [late]\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("late-owner context serializes");
        let receipt = &context["decisionOutcomes"][0];

        assert_eq!(receipt["sourceSelectionMessageId"], "source");
        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["finalized"], false);
        assert_eq!(receipt["commitMessageId"], Value::Null);
        assert_eq!(context["choiceFence"], Value::Null);
    }

    #[test]
    fn a_duplicate_same_fence_exit_conflicts_the_cached_pending_receipt() {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let transition_line =
            "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]";
        let (through_response, final_tail) = fixture
            .split_once(transition_line)
            .expect("fixture contains the post-discard transition");
        let mut model = RunModel::default();
        model.ingest_text(through_response);

        let pending = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("pending context serializes");
        assert_eq!(
            pending["decisionOutcomes"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(pending["decisionOutcomes"][0]["status"], "pending");
        assert_eq!(
            pending["decisionOutcomes"][0]["sameFencePolicy"],
            "deny_repeat_use_cached_receipt"
        );
        assert!(
            !pending["availableActions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|action| action == "discard_current_choice")
        );

        model.ingest_text(
            "[05:40:00.200] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
        );
        let cached = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            234,
            None,
            None,
        ))
        .expect("same-fence context serializes");
        assert_eq!(cached["decisionOutcomes"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            cached["decisionOutcomes"][0]["choiceFence"],
            pending["decisionOutcomes"][0]["choiceFence"]
        );
        assert_eq!(
            cached["decisionOutcomes"][0]["evidence"]["exitCommandResponse"],
            true
        );
        assert_eq!(
            cached["decisionOutcomes"][0]["evidence"]["batchConflicted"],
            true
        );
        assert_eq!(
            cached["decisionOutcomes"][0]["reason"],
            "discard_commit_batch_conflict"
        );

        model.ingest_text(&format!("{transition_line}{final_tail}"));
        let finalized = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            345,
            None,
            None,
        ))
        .expect("final context serializes");
        assert_eq!(
            finalized["decisionOutcomes"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(finalized["decisionOutcomes"][0]["status"], "pending");
        assert_eq!(finalized["decisionOutcomes"][0]["finalized"], false);
        assert_eq!(
            finalized["decisionOutcomes"][0]["evidence"]["batchConflicted"],
            true
        );
    }

    #[test]
    fn event_option_disposal_is_not_finalized_when_the_same_message_later_purchases_the_candidate()
    {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let exit_line = "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, _) = fixture
            .split_once(exit_line)
            .expect("fixture contains the explicit discard request");
        let mut model = RunModel::default();
        model.ingest_text(before_exit);
        model.ingest_text(concat!(
            "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
            "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
            "[05:40:00.160] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
            "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[05:40:00.161] [GameSimHandler] Cards Dealt: [enc_next_a [Medium] | [enc_next_b [Medium] | \n",
            "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
        ));

        let before_message_finish = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("pre-boundary decision context serializes");
        assert_eq!(
            before_message_finish["decisionOutcomes"][0]["status"],
            "pending"
        );
        assert_eq!(
            before_message_finish["decisionOutcomes"][0]["finalized"],
            false
        );
        assert_eq!(
            before_message_finish["decisionOutcomes"][0]["evidence"]["exactCandidatesDisposed"],
            true
        );

        model.ingest_text(concat!(
            "[05:40:00.164] [BoardManager] Card Purchased: InstanceId: itm_gKj7HU5 - TemplateId8535493a-67ae-4248-a34d-176549948686 - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[05:40:00.165] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("conflicted decision context serializes");
        let receipt = &context["decisionOutcomes"][0];

        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["finalized"], false);
        assert_eq!(receipt["evidence"]["exactCandidatesDisposed"], true);
        assert_eq!(receipt["evidence"]["candidatePurchaseSeen"], true);
    }

    #[test]
    fn interleaved_finished_message_cannot_own_or_finalize_an_event_option_discard() {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let exit_line = "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, _) = fixture
            .split_once(exit_line)
            .expect("fixture contains the explicit discard request");
        let mut model = RunModel::default();
        model.ingest_text(before_exit);
        model.ingest_text(concat!(
            "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
            "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
            "[05:40:00.160] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
            "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[05:40:00.161] [GameSimHandler] Cards Dealt: [enc_next_a [Medium] | [enc_next_b [Medium] | \n",
            "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
            "[05:40:00.163] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [noise]\n",
            "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("interleaved decision context serializes");
        let source_receipt = context["decisionOutcomes"]
            .as_array()
            .expect("decision outcomes")
            .iter()
            .find(|receipt| receipt["choiceFence"]["selectionMessageId"] == "4FQ")
            .expect("source fence receipt is preserved");

        assert_eq!(source_receipt["status"], "pending");
        assert_eq!(source_receipt["finalized"], false);
        assert_eq!(source_receipt["sourceSelectionMessageId"], "4FQ");
        assert_eq!(source_receipt["commitMessageId"], Value::Null);
        assert_eq!(context["choiceFence"], Value::Null);
    }

    #[test]
    fn a_second_deal_in_one_message_batch_fails_closed_for_commit_and_selection_ownership() {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let exit_line = "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, _) = fixture
            .split_once(exit_line)
            .expect("fixture contains the explicit discard request");
        let mut model = RunModel::default();
        model.ingest_text(before_exit);
        model.ingest_text(concat!(
            "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
            "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
            "[05:40:00.160] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
            "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[05:40:00.161] [GameSimHandler] Cards Dealt: [enc_next_a [Medium] |\n",
            "[05:40:00.162] [GameSimHandler] Cards Dealt: [enc_next_b [Medium] |\n",
            "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
            "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("ambiguous batch context serializes");
        let source_receipt = context["decisionOutcomes"]
            .as_array()
            .expect("decision outcomes")
            .iter()
            .find(|receipt| receipt["sourceSelectionMessageId"] == "4FQ")
            .expect("source fence remains pending");

        assert_eq!(source_receipt["status"], "pending");
        assert_eq!(source_receipt["commitMessageId"], Value::Null);
        assert_eq!(context["choiceFence"], Value::Null);
    }

    #[test]
    fn out_of_order_or_duplicate_discard_steps_conflict_immediately_and_cannot_recover() {
        struct Case {
            name: &'static str,
            offending_sequence: &'static str,
            attempted_completion: &'static str,
        }

        let cases = [
            Case {
                name: "duplicate exit command",
                offending_sequence: "[05:40:00.001] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
                attempted_completion: concat!(
                    "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
                    "[05:40:00.160] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.162] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                    "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
            Case {
                name: "duplicate exit response",
                offending_sequence: concat!(
                    "[05:40:00.159] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 159 ms\n",
                    "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
                ),
                attempted_completion: concat!(
                    "[05:40:00.160] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.162] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                    "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
            Case {
                name: "deal before response",
                offending_sequence: concat!(
                    "[05:40:00.100] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.101] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                ),
                attempted_completion: concat!(
                    "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
                    "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
            Case {
                name: "deal before transition",
                offending_sequence: concat!(
                    "[05:40:00.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
                    "[05:40:00.101] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.102] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                ),
                attempted_completion: concat!(
                    "[05:40:00.161] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.163] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
            Case {
                name: "disposal before deal",
                offending_sequence: concat!(
                    "[05:40:00.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
                    "[05:40:00.101] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.102] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.103] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                ),
                attempted_completion: concat!(
                    "[05:40:00.104] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
            Case {
                name: "duplicate transition",
                offending_sequence: concat!(
                    "[05:40:00.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
                    "[05:40:00.101] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.102] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.103] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                ),
                attempted_completion: concat!(
                    "[05:40:00.104] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                    "[05:40:00.105] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
            Case {
                name: "duplicate disposal",
                offending_sequence: concat!(
                    "[05:40:00.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
                    "[05:40:00.101] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.102] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.103] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                    "[05:40:00.104] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.105] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                ),
                attempted_completion: "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
            },
            Case {
                name: "extra deal",
                offending_sequence: concat!(
                    "[05:40:00.100] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 100 ms\n",
                    "[05:40:00.101] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [next]\n",
                    "[05:40:00.102] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
                    "[05:40:00.103] [GameSimHandler] Cards Dealt: [enc_next [Medium] |\n",
                    "[05:40:00.104] [GameSimHandler] Cards Dealt: [enc_extra [Medium] |\n",
                ),
                attempted_completion: concat!(
                    "[05:40:00.105] [GameSimHandler] Cards Disposed: itm_gKj7HU5 |\n",
                    "[05:40:00.164] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [next]\n",
                ),
            },
        ];

        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let exit_line = "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, _) = fixture
            .split_once(exit_line)
            .expect("fixture contains the explicit discard request");

        for case in cases {
            let mut model = RunModel::default();
            model.ingest_text(before_exit);
            model.ingest_text(&format!("{exit_line}\n{}", case.offending_sequence));

            let conflicted = serde_json::to_value(model.context(
                &TemplateIndex::default(),
                &CompanionConfig::test_default(),
                123,
                None,
                None,
            ))
            .unwrap_or_else(|error| panic!("{} conflict context: {error}", case.name));
            let receipt = &conflicted["decisionOutcomes"][0];
            assert_eq!(receipt["sourceSelectionMessageId"], "4FQ", "{}", case.name);
            assert_eq!(receipt["status"], "pending", "{}", case.name);
            assert_eq!(receipt["finalized"], false, "{}", case.name);
            assert_eq!(receipt["commitMessageId"], Value::Null, "{}", case.name);
            assert_eq!(
                receipt["reason"], "discard_commit_batch_conflict",
                "{}",
                case.name
            );
            assert_eq!(
                receipt["evidence"]["batchConflicted"], true,
                "{}",
                case.name
            );

            model.ingest_text(case.attempted_completion);
            let after_completion = serde_json::to_value(model.context(
                &TemplateIndex::default(),
                &CompanionConfig::test_default(),
                456,
                None,
                None,
            ))
            .unwrap_or_else(|error| panic!("{} completion context: {error}", case.name));
            let receipt = &after_completion["decisionOutcomes"][0];
            assert_eq!(receipt["status"], "pending", "{}", case.name);
            assert_eq!(receipt["finalized"], false, "{}", case.name);
            assert_eq!(receipt["commitMessageId"], Value::Null, "{}", case.name);
            assert_eq!(
                receipt["evidence"]["batchConflicted"], true,
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn a_new_fence_exit_is_denied_without_overwriting_the_pending_source_fence() {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let exit_line = "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, _) = fixture
            .split_once(exit_line)
            .expect("fixture contains the explicit discard request");
        let mut model = RunModel::default();
        model.ingest_text(before_exit);
        model.ingest_text(concat!(
            "[05:40:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
            "[05:40:00.160] [NetworkManager] [HttpGameClient] /commands | ExitCurrentStateCommand response | 160 ms\n",
            "[05:40:00.161] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [incomplete-a]\n",
            "[05:41:00.000] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [choice-b]\n",
            "[05:41:00.001] [GameSimHandler] Cards Dealt: [itm_choice_b [Small] |\n",
            "[05:41:00.002] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [choice-b]\n",
            "[05:42:00.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("conflicting fence context serializes");
        let receipts = context["decisionOutcomes"]
            .as_array()
            .expect("decision outcomes");
        let source = receipts
            .iter()
            .find(|receipt| receipt["sourceSelectionMessageId"] == "4FQ")
            .expect("source fence remains pending");
        let denied = receipts
            .iter()
            .find(|receipt| receipt["sourceSelectionMessageId"] == "choice-b")
            .expect("conflicting fence has a denial receipt");

        assert_eq!(
            source["choiceFence"]["candidateInstanceIds"],
            json!(["itm_gKj7HU5"])
        );
        assert_eq!(source["status"], "pending");
        assert_eq!(source["finalized"], false);
        assert_eq!(
            denied["choiceFence"]["candidateInstanceIds"],
            json!(["itm_choice_b"])
        );
        assert_eq!(denied["status"], "denied");
        assert_eq!(denied["finalized"], true);
        assert_eq!(denied["reason"], "active_discard_fence_conflict");
        assert_eq!(denied["commitMessageId"], Value::Null);
        assert_eq!(context["choiceFence"]["selectionMessageId"], "choice-b");
        assert!(
            !context["availableActions"]
                .as_array()
                .expect("actions")
                .iter()
                .any(|action| action == "discard_current_choice")
        );
    }

    #[test]
    fn magma_fixture_marks_its_synthetic_future_chain_as_non_live_test_evidence() {
        let fixture = include_str!("../../fixtures/magma-core-event-option-discarded.log");
        let lines = fixture.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 20);
        assert!(
            lines[..13]
                .iter()
                .all(|line| !line.contains("SYNTHETIC TEST CONTINUATION"))
        );
        assert!(lines[13].contains("SYNTHETIC TEST CONTINUATION: lines 14-20"));
        assert!(lines[15].contains("Processing [NetMessageGameSim]  |  Id: [next]"));
    }

    #[test]
    fn merchant_purchase_stays_a_purchase_receipt_and_never_becomes_a_reward_outcome() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[16:46:21.136] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[16:55:14.714] [AppState] State changed from [ChoiceState] to [EncounterState]\n",
            "[16:55:14.715] [GameSimHandler] Cards Dealt: [itm_OiD0MGx [Medium] | [itm_Q2vxg9R [Medium] | [itm_MNhbDmc [Medium] | \n",
            "[16:55:14.719] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [2KF]\n",
            "[16:56:45.288] [CardOperationUtility] Successfully moved card to: [itm_MNhbDmc [Player] [Stash] [Socket_0] [Medium]\n",
            "[16:56:45.293] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[16:56:45.355] [NetworkManager] [HttpGameClient] /commands | SelectItemCommand response | 60 ms\n",
            "[16:56:45.355] [BoardManager] Card Purchased: InstanceId: itm_MNhbDmc - TemplateId3f867017-c026-412d-99f7-67ca26092e7a - Target:PlayerStorageSocket_0 - SectionStorage\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("merchant context serializes");

        assert_eq!(
            context["purchaseReceipts"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(context["purchaseReceipts"][0]["instanceId"], "itm_MNhbDmc");
        assert_eq!(context["rewardOutcomes"], json!([]));
        assert_eq!(context["decisionOutcomes"], json!([]));
    }

    #[test]
    fn unresolved_single_loot_candidate_exposes_only_a_fenced_center_hint() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [ReplayState] to [LootState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [skl_loot [Medium] | \n",
        ));

        let context = model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        let regions = serde_json::to_value(&context)
            .expect("context serializes")["candidateActionableRegions"]
            .clone();

        assert_eq!(regions.as_array().map(Vec::len), Some(1));
        assert_eq!(regions[0]["instanceId"], "skl_loot");
        assert_eq!(regions[0]["identityStatus"], "unresolved");
        assert_eq!(regions[0]["primaryGesture"], "left_click");
        assert_eq!(regions[0]["regionSource"], "unverified_layout_hint");
        assert_eq!(regions[0]["requiresFreshObservationFence"], true);
        assert_eq!(regions[0]["regionHint"], "loot_single_candidate_center");
        let center_x = regions[0]["crop"]["x"]
            .as_f64()
            .expect("center hint has an x coordinate");
        assert!((center_x - 0.41).abs() < 0.001);
    }

    #[test]
    fn resolved_loot_candidate_keeps_the_same_fresh_observation_fence() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [ReplayState] to [LootState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [skl_loot [Medium] | \n",
        ));
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "instanceOverrides": {
                "skl_loot": {"templateId": "tpl-skill"}
            }
        }))
        .expect("resolved identity config");
        let index = TemplateIndex::from_templates([template(
            "tpl-skill",
            "Resolved skill",
            "Medium",
            &[],
            &[],
        )]);

        let context = serde_json::to_value(model.context(&index, &config, 123, None, None))
            .expect("context serializes");
        let region = &context["candidateActionableRegions"][0];

        assert_eq!(region["identityStatus"], "resolved");
        assert_eq!(region["primaryGesture"], "left_click");
        assert_eq!(region["regionSource"], "unverified_layout_hint");
        assert_eq!(region["requiresFreshObservationFence"], true);
    }

    #[test]
    fn loot_skill_is_claimed_only_after_the_full_exact_commit_survives_disposal() {
        let mut model = RunModel::default();
        model.ingest_text(include_str!("../../fixtures/loot-skill-reward-claimed.log"));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("context serializes");
        let outcome = &context["rewardOutcomes"][0];

        assert_eq!(outcome["candidateInstanceIds"], json!(["skl_reward"]));
        assert_eq!(outcome["selectedInstanceId"], "skl_reward");
        assert_eq!(outcome["status"], "claimed");
        assert_eq!(outcome["finalized"], true);
        assert_eq!(outcome["evidence"]["selectedSkillSeen"], true);
        assert_eq!(outcome["evidence"]["selectSkillCommandSent"], true);
        assert_eq!(outcome["evidence"]["selectSkillCommandResponse"], true);
        assert_eq!(outcome["evidence"]["lootTransitionSeen"], true);
        assert_eq!(outcome["evidence"]["selectedInstanceDisposed"], false);
        assert_eq!(context["selectionOptions"][0]["instanceId"], "enc_next_a");
    }

    #[test]
    fn exiting_loot_and_disposing_its_exact_candidate_is_a_destructive_discard() {
        let mut model = RunModel::default();
        model.ingest_text(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
        );
        let fixture = include_str!("../../fixtures/loot-skill-reward-discarded.log");
        let exit_line = "[00:00:03.000] [NetworkManager] [HttpGameClient] Sending ExitCurrentStateCommand to /commands";
        let (before_exit, after_exit) = fixture
            .split_once(exit_line)
            .expect("Loot fixture contains its exit request");
        model.ingest_text(before_exit);
        model.ingest_text(
            "[00:00:02.500] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [loot-message]\n",
        );
        model.ingest_text(&format!("{exit_line}{after_exit}"));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("context serializes");
        let outcome = &context["rewardOutcomes"][0];

        assert_eq!(outcome["status"], "discarded");
        assert_eq!(outcome["finalized"], true);
        assert_eq!(outcome["reason"], "exit_disposed_pending_reward");
        assert_eq!(outcome["evidence"]["exitCommandSent"], true);
        assert_eq!(outcome["evidence"]["exitCommandResponse"], true);
        assert_eq!(outcome["evidence"]["lootTransitionSeen"], true);
        assert_eq!(context["decisionOutcomes"], json!([]));
    }

    #[test]
    fn loot_item_claim_requires_the_exact_card_purchased_destination() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [AppState] State changed from [ReplayState] to [LootState]\n",
            "[00:00:02.000] [GameSimHandler] Cards Dealt: [itm_reward [Small] | \n",
            "[00:00:03.000] [CardOperationUtility] Successfully moved card to: [itm_reward [Player] [Stash] [Socket_7] [Small]\n",
            "[00:00:03.010] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[00:00:03.200] [NetworkManager] [HttpGameClient] /commands | SelectItemCommand response | 190 ms\n",
            "[00:00:03.210] [BoardManager] Card Purchased: InstanceId: itm_reward - TemplateIdtpl-reward - Target:PlayerStorageSocket_7 - SectionStorage\n",
            "[00:00:03.300] [AppState] State changed from [LootState] to [ChoiceState]\n",
            "[00:00:03.310] [GameSimHandler] Cards Dealt: [enc_next [Medium] | \n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("context serializes");
        let outcome = &context["rewardOutcomes"][0];

        assert_eq!(outcome["selectedInstanceId"], "itm_reward");
        assert_eq!(outcome["status"], "claimed");
        assert_eq!(outcome["finalized"], true);
        assert_eq!(
            outcome["reason"],
            "exact_item_purchase_destination_observed"
        );
        assert_eq!(outcome["itemDestination"]["section"], "stash");
        assert_eq!(outcome["itemDestination"]["socket"], 7);
    }

    #[test]
    fn item_drag_and_command_delivery_without_card_purchased_remain_pending() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [AppState] State changed from [ReplayState] to [LootState]\n",
            "[00:00:02.000] [GameSimHandler] Cards Dealt: [itm_reward [Small] | \n",
            "[00:00:03.000] [CardOperationUtility] Successfully moved card to: [itm_reward [Player] [Stash] [Socket_7] [Small]\n",
            "[00:00:03.010] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[00:00:03.200] [NetworkManager] [HttpGameClient] /commands | SelectItemCommand response | 190 ms\n",
            "[00:00:03.300] [AppState] State changed from [LootState] to [ChoiceState]\n",
            "[00:00:03.310] [GameSimHandler] Cards Dealt: [enc_next [Medium] | \n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("context serializes");
        let outcome = &context["rewardOutcomes"][0];

        assert_eq!(outcome["candidateInstanceIds"], json!(["itm_reward"]));
        assert_eq!(outcome["status"], "pending");
        assert_eq!(outcome["itemDestination"], Value::Null);
        assert_eq!(
            outcome["evidence"]["exactCardPurchasedDestinationSeen"],
            false
        );
    }

    #[test]
    fn next_deal_cannot_erase_an_unfinalized_skill_reward_commit() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [AppState] State changed from [ReplayState] to [LootState]\n",
            "[00:00:02.000] [GameSimHandler] Cards Dealt: [skl_reward [Medium] | \n",
            "[00:00:03.000] [AppState] Selected skill skl_reward to socket SkillSocket_0\n",
            "[00:00:03.010] [NetworkManager] [HttpGameClient] Sending SelectSkillCommand to /commands\n",
            "[00:00:03.300] [AppState] State changed from [LootState] to [ChoiceState]\n",
            "[00:00:03.310] [GameSimHandler] Cards Dealt: [enc_next [Medium] | \n",
            "[00:00:03.320] [GameSimHandler] Cards Disposed: com_previous |\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("context serializes");
        let outcome = &context["rewardOutcomes"][0];

        assert_eq!(outcome["candidateInstanceIds"], json!(["skl_reward"]));
        assert_eq!(outcome["status"], "pending");
        assert_eq!(outcome["finalized"], false);
        assert_eq!(outcome["evidence"]["selectSkillCommandResponse"], false);
        assert_eq!(context["selectionOptions"][0]["instanceId"], "enc_next");
    }

    #[test]
    fn disposed_selected_skill_is_never_published_as_claimed() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [AppState] State changed from [ReplayState] to [LootState]\n",
            "[00:00:02.000] [GameSimHandler] Cards Dealt: [skl_reward [Medium] | \n",
            "[00:00:03.000] [AppState] Selected skill skl_reward to socket SkillSocket_0\n",
            "[00:00:03.010] [NetworkManager] [HttpGameClient] Sending SelectSkillCommand to /commands\n",
            "[00:00:03.200] [NetworkManager] [HttpGameClient] /commands | SelectSkillCommand response | 190 ms\n",
            "[00:00:03.300] [AppState] State changed from [LootState] to [ChoiceState]\n",
            "[00:00:03.310] [GameSimHandler] Cards Disposed: skl_reward |\n",
        ));

        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        ))
        .expect("context serializes");
        let outcome = &context["rewardOutcomes"][0];

        assert_eq!(outcome["status"], "unresolved");
        assert_eq!(outcome["finalized"], true);
        assert_eq!(outcome["reason"], "selected_skill_was_disposed");
        assert_eq!(outcome["evidence"]["selectedInstanceDisposed"], true);
    }

    #[test]
    fn package_profile_fixture_and_reward_targets_share_the_published_contract() {
        let profile = serde_json::from_str::<Value>(include_str!("../../profile.json"))
            .expect("profile JSON");
        let package = serde_json::from_str::<Value>(include_str!("../../profile-package.json"))
            .expect("package JSON");
        let fixture = serde_json::from_str::<Value>(include_str!(
            "../../fixtures/day-7-measured-recovery.json"
        ))
        .expect("fixture JSON");
        let context = serde_json::to_value(RunModel::default().context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            0,
            None,
            None,
        ))
        .expect("context serializes");

        assert_eq!(profile["profile_version"], "1.7.0");
        assert_eq!(context["schemaVersion"], "3.6.0");
        assert_eq!(profile["profile_version"], package["version"]);
        assert_eq!(
            profile["state_sources"][0]["expected_schema_version"],
            context["schemaVersion"]
        );
        assert_eq!(fixture["schemaVersion"], context["schemaVersion"]);
        assert_eq!(fixture["choiceFence"], Value::Null);
        assert_eq!(fixture["decisionOutcomes"], json!([]));
        assert_eq!(context["choiceFence"], Value::Null);
        assert_eq!(context["decisionOutcomes"], json!([]));
        assert!(context["decisionSupport"]["upgradeOpportunity"].is_object());
        assert!(fixture["decisionSupport"]["upgradeOpportunity"].is_object());

        let inventory_targets = profile["surfaces"]
            .as_array()
            .expect("profile surfaces")
            .iter()
            .find(|surface| surface["id"] == "inventory")
            .and_then(|surface| surface["targets"].as_array())
            .expect("inventory targets");
        assert!(
            inventory_targets
                .iter()
                .any(|target| target["id"] == "upgrade_opportunity")
        );
        assert!(
            inventory_targets
                .iter()
                .any(|target| target["id"] == "inventory_mutation_receipt")
        );
        assert_eq!(context["inventoryMutationReceipts"], json!([]));
        assert_eq!(fixture["inventoryMutationReceipts"], json!([]));

        let reward_targets = profile["surfaces"]
            .as_array()
            .expect("profile surfaces")
            .iter()
            .find(|surface| surface["id"] == "reward")
            .and_then(|surface| surface["targets"].as_array())
            .expect("reward targets");
        assert!(reward_targets.iter().any(|target| {
            target["id"] == "reward_skill"
                && target["supported_actions"]
                    .as_array()
                    .is_some_and(|actions| actions.iter().any(|action| action == "claim"))
        }));
        assert!(reward_targets.iter().any(|target| {
            target["id"] == "event_exit"
                && target["supported_actions"]
                    == json!(["discard_pending_reward", "discard_current_choice"])
        }));
        assert_eq!(
            profile["settings"]["destructive_confirmation_required"],
            true
        );
    }

    #[test]
    fn pvp_run_phase_uses_wins_and_prestige_instead_of_loss_streak() {
        let base = ObservedProgress {
            run_id: None,
            day: Some(4),
            hour: Some(3),
            level: Some(4),
            game_build: None,
            source_state_tick_id: None,
            verification: ObservationVerification::Unverified,
            health: Some(1000),
            max_health: Some(1000),
            gold: Some(30),
            wins: Some(2),
            losses: Some(0),
            loss_streak: Some(9),
            prestige: Some(20),
            max_prestige: Some(20),
            source_observation_id: Some("phase-test".into()),
            provenance: "test".into(),
        };
        let policy = DecisionPolicyConfig::default();

        let empty_board = BoardCapacityStatus {
            physical_slots: 10,
            unlocked_slot_ids: Some((0..10).collect()),
            unlocked_capacity: Some(10),
            occupied_slots: Some(10),
            open_unlocked_slots: Some(0),
            fit_placements: Some(Vec::new()),
            unlock_provenance: "test".into(),
            verified: true,
            capacity_gate_satisfied: true,
        };
        let empty_storage = StorageCapacityStatus::unavailable(None, "test");
        let early = build_decision_support(
            &[],
            &[],
            Some(&base),
            &policy,
            empty_board.clone(),
            empty_storage.clone(),
            empty_upgrade_opportunity(),
        );
        assert_eq!(early.mode, "build_scalable_core");

        let mut late = base.clone();
        late.wins = Some(8);
        let late = build_decision_support(
            &[],
            &[],
            Some(&late),
            &policy,
            empty_board.clone(),
            empty_storage.clone(),
            empty_upgrade_opportunity(),
        );
        assert_eq!(late.mode, "convert_current_core");

        let mut critical = base;
        critical.day = Some(7);
        critical.prestige = Some(10);
        let critical = build_decision_support(
            &[],
            &[],
            Some(&critical),
            &policy,
            empty_board,
            empty_storage,
            empty_upgrade_opportunity(),
        );
        assert_eq!(critical.mode, "immediate_pvp_survival");
    }

    #[test]
    fn card_identity_cache_joins_exact_local_template_to_bazaardb_id() {
        let index = TemplateIndex::from_templates([template(
            "7317d6a2-adea-442c-9e97-7f7bbf64ae99",
            "Unibou",
            "Medium",
            &["Friend"],
            &["Shield"],
        )]);
        let cache = CardIdentityCache {
            schema_version: 1,
            provider: "bazaardb".into(),
            records: BTreeMap::from([(
                "7317d6a2-adea-442c-9e97-7f7bbf64ae99".into(),
                CachedExternalCardIdentity {
                    external_card_id: "l1n7dqkk5gpl0n6h52880y0jq5".into(),
                    canonical_name: "Unibou".into(),
                    card_type: "item".into(),
                    url: "https://bazaardb.gg/card/l1n7dqkk5gpl0n6h52880y0jq5/Unibou".into(),
                    source_patch: "16.2 (Jul 17)".into(),
                    verified_at: "2026-08-09".into(),
                    match_basis: vec!["canonical_name".into(), "size".into(), "attributes".into()],
                },
            )]),
        };
        cache.validate(&index).expect("valid exact mapping");
        let mut model = RunModel::default();
        model.ingest_text("[00:00:00.000] [BoardManager] Card Purchased: InstanceId: item - TemplateId7317d6a2-adea-442c-9e97-7f7bbf64ae99 - Target:PlayerStorageSocket_8 - SectionStorage\n");

        let context = model.context(
            &index,
            &CompanionConfig::test_default(),
            123,
            None,
            Some(&cache),
        );

        assert_eq!(context.chest_items[0].external_references.len(), 1);
        assert_eq!(
            context.chest_items[0].external_references[0].external_card_id,
            "l1n7dqkk5gpl0n6h52880y0jq5"
        );
    }

    #[test]
    fn ambiguous_socket_only_move_preserves_existing_stash_section() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: item - TemplateIdtpl-item - Target:PlayerStorageSocket_8 - SectionStorage\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card item to Socket_6\n",
        ));
        let index =
            TemplateIndex::from_templates([template("tpl-item", "Stored item", "Small", &[], &[])]);

        let context = model.context(&index, &CompanionConfig::test_default(), 123, None, None);

        assert!(context.board_items.is_empty());
        assert_eq!(context.chest_items[0].socket, Some(6));
    }

    #[test]
    fn verified_open_board_blocks_player_combat_and_surfaces_the_stash_fill_action() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[00:00:00.500] [GameSimEventSocketsUnlocked] UnlockedSocketsBitmask: 1023\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [BoardManager] Card Purchased: InstanceId: b - TemplateIdtpl-b - Target:PlayerSocket_2 - SectionPlayer\n",
            "[00:00:03.000] [BoardManager] Card Purchased: InstanceId: c - TemplateIdtpl-c - Target:PlayerSocket_4 - SectionPlayer\n",
            "[00:00:04.000] [BoardManager] Card Purchased: InstanceId: d - TemplateIdtpl-d - Target:PlayerSocket_6 - SectionPlayer\n",
            "[00:00:05.000] [BoardManager] Card Purchased: InstanceId: e - TemplateIdtpl-e - Target:PlayerSocket_8 - SectionPlayer\n",
            "[00:00:06.000] [BoardManager] Card Purchased: InstanceId: spare - TemplateIdtpl-spare - Target:PlayerStorageSocket_9 - SectionStorage\n",
            "[00:00:07.000] [GameSimHandler] Cards Spawned: [a [Player] [Hand] [Socket_0] [Medium] | [b [Player] [Hand] [Socket_2] [Medium] | [c [Player] [Hand] [Socket_4] [Medium] | [d [Player] [Hand] [Socket_6] [Medium] | [e [Player] [Hand] [Socket_8] [Small] | \n",
        ));
        let index = TemplateIndex::from_templates([
            template("tpl-a", "A", "Medium", &[], &[]),
            template("tpl-b", "B", "Medium", &[], &[]),
            template("tpl-c", "C", "Medium", &[], &[]),
            template("tpl-d", "D", "Medium", &[], &[]),
            template("tpl-e", "E", "Small", &[], &[]),
            template("tpl-spare", "Spare", "Small", &[], &[]),
        ]);

        let context = model.context(&index, &CompanionConfig::test_default(), 123, None, None);

        assert!(context.decision_support.board_capacity.verified);
        assert_eq!(context.decision_support.board_capacity.physical_slots, 10);
        assert_eq!(
            context.decision_support.board_capacity.unlocked_capacity,
            Some(10)
        );
        assert_eq!(
            context.decision_support.board_capacity.occupied_slots,
            Some(9)
        );
        assert_eq!(
            context.decision_support.board_capacity.open_unlocked_slots,
            Some(1)
        );
        assert!(
            !context
                .decision_support
                .board_capacity
                .capacity_gate_satisfied
        );
        assert!(
            context
                .decision_support
                .hard_constraints
                .iter()
                .any(|value| value == "fill_unlocked_board_capacity_before_player_combat")
        );
        assert!(
            context
                .decision_support
                .blocked_mutations
                .iter()
                .any(|value| value == "enter_player_combat_with_open_unlocked_slots")
        );
        assert!(
            context
                .available_actions
                .iter()
                .any(|value| value == "fill_board_from_stash_before_player_combat")
        );
    }

    #[test]
    fn authoritative_unlock_bitmask_drives_capacity_and_contiguous_fit_placements() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[00:00:01.000] [GameSimEventSocketsUnlocked] UnlockedSocketsBitmask: 252\n",
            "[00:00:02.000] [BoardManager] Card Purchased: InstanceId: a - TemplateIdtpl-a - Target:PlayerSocket_2 - SectionPlayer\n",
            "[00:00:03.000] [BoardManager] Card Purchased: InstanceId: b - TemplateIdtpl-b - Target:PlayerSocket_3 - SectionPlayer\n",
            "[00:00:04.000] [BoardManager] Card Purchased: InstanceId: spare - TemplateIdtpl-spare - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[00:00:05.000] [GameSimHandler] Cards Spawned: [a [Player] [Hand] [Socket_2] [Small] | [b [Player] [Hand] [Socket_3] [Small] | \n",
        ));
        let index = TemplateIndex::from_templates([
            template("tpl-a", "A", "Small", &[], &[]),
            template("tpl-b", "B", "Small", &[], &[]),
            template("tpl-spare", "Spare", "Medium", &[], &[]),
        ]);

        let context = model.context(&index, &CompanionConfig::test_default(), 123, None, None);
        let capacity = serde_json::to_value(&context.decision_support.board_capacity)
            .expect("capacity serializes");

        assert_eq!(capacity["physicalSlots"], 10);
        assert_eq!(capacity["unlockedSlotIds"], json!([2, 3, 4, 5, 6, 7]));
        assert_eq!(capacity["unlockedCapacity"], 6);
        assert_eq!(capacity["occupiedSlots"], 2);
        assert_eq!(capacity["openUnlockedSlots"], 4);
        assert_eq!(
            capacity["fitPlacements"],
            json!([
                {
                    "instanceId": "spare",
                    "size": "Medium",
                    "width": 2,
                    "leftSocket": 4,
                    "slotIds": [4, 5]
                },
                {
                    "instanceId": "spare",
                    "size": "Medium",
                    "width": 2,
                    "leftSocket": 5,
                    "slotIds": [5, 6]
                },
                {
                    "instanceId": "spare",
                    "size": "Medium",
                    "width": 2,
                    "leftSocket": 6,
                    "slotIds": [6, 7]
                }
            ])
        );
    }

    #[test]
    fn verified_level_fallback_requires_the_current_run_tick_and_supported_build() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [AppState] State changed from [StartRunAppState] to [ChoiceState]\n",
            "[00:00:02.000] [BoardManager] Card Purchased: InstanceId: a - TemplateIdtpl-a - Target:PlayerSocket_2 - SectionPlayer\n",
            "[00:00:03.000] [BoardManager] Card Purchased: InstanceId: b - TemplateIdtpl-b - Target:PlayerSocket_3 - SectionPlayer\n",
            "[00:00:04.000] [BoardManager] Card Purchased: InstanceId: spare - TemplateIdtpl-spare - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[00:00:05.000] [GameSimHandler] Cards Spawned: [a [Player] [Hand] [Socket_2] [Small] | [b [Player] [Hand] [Socket_3] [Small] | \n",
        ));
        let index = TemplateIndex::from_templates([
            template("tpl-a", "A", "Small", &[], &[]),
            template("tpl-b", "B", "Small", &[], &[]),
            template("tpl-spare", "Spare", "Medium", &[], &[]),
        ]);
        let run_id = model.run_id.clone().expect("current run id");
        let observed_tick_id = model.tick;
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentProgress": {
                "runId": run_id,
                "level": 2,
                "gameBuild": "1.0.11894",
                "sourceStateTickId": observed_tick_id,
                "sourceObservationId": "observation-level-2",
                "verification": "verified",
                "provenance": "verified_live_observation"
            }
        }))
        .expect("level fallback config");

        let current = model.context(&index, &config, 123, None, None);
        let current_capacity = serde_json::to_value(&current.decision_support.board_capacity)
            .expect("capacity serializes");
        assert_eq!(current.run.state_tick_id, observed_tick_id);
        assert!(current.decision_support.board_capacity.verified);
        assert_eq!(
            current_capacity["unlockedSlotIds"],
            json!([2, 3, 4, 5, 6, 7])
        );
        assert_eq!(
            current_capacity["unlockProvenance"],
            "verified_level_fallback_build_1.0.11894"
        );

        model.ingest_text("[00:00:06.000] [GameSimHandler] Cards Dealt: [offer-a [Small] | \n");
        let stale = model.context(&index, &config, 124, None, None);
        let stale_capacity = serde_json::to_value(&stale.decision_support.board_capacity)
            .expect("capacity serializes");
        assert!(!stale.decision_support.board_capacity.verified);
        assert_eq!(stale_capacity["unlockedSlotIds"], Value::Null);
        assert_eq!(stale_capacity["fitPlacements"], Value::Null);
        assert!(
            !stale
                .available_actions
                .iter()
                .any(|action| action == "fill_board_from_stash_before_player_combat")
        );
    }

    #[test]
    fn final_socket_only_receipt_keeps_desired_socket_and_clamp_unknown() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: item - TemplateIdtpl-item - Target:PlayerStorageSocket_8 - SectionStorage\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card item to Socket_6\n",
        ));

        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-item",
                "Stored item",
                "Small",
                &[],
                &[],
            )]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        let value = serde_json::to_value(context).expect("context serializes");

        assert_eq!(
            value["placementReceipts"],
            json!([{
                "instanceId": "item",
                "desiredSocket": null,
                "clamp": "unknown",
                "finalSocket": 6,
                "provenance": "card_operation_utility_final_socket_only"
            }])
        );
    }

    #[test]
    fn authoritative_unlock_mask_wins_over_a_conflicting_verified_level_fallback() {
        let progress = serde_json::from_value::<ObservedProgress>(json!({
            "runId": "run",
            "level": 4,
            "gameBuild": "1.0.11894",
            "sourceStateTickId": 7,
            "sourceObservationId": "observation",
            "verification": "verified",
            "provenance": "verified_live_observation"
        }))
        .expect("progress");
        let authoritative = BoardUnlockEvidence {
            bitmask: 0x0fc,
            provenance: "game_sim_event_sockets_unlocked_bitmask",
        };

        let resolved = resolve_board_unlock(Some(&authoritative), Some(&progress), 7);

        assert_eq!(resolved.evidence, Some(authoritative));
        assert_eq!(resolved.reason, "game_sim_event_sockets_unlocked_bitmask");
    }

    #[test]
    fn authoritative_unlock_parser_accepts_event_decimal_and_snapshot_hex() {
        assert_eq!(
            parse_authoritative_board_unlock(
                "[GameSimEventSocketsUnlocked] UnlockedSocketsBitmask: 510"
            ),
            Some(BoardUnlockEvidence {
                bitmask: 0x1fe,
                provenance: "game_sim_event_sockets_unlocked_bitmask",
            })
        );
        assert_eq!(
            parse_authoritative_board_unlock("[PlayerSnapshotDTO] UnlockedSlots=0x1FE"),
            Some(BoardUnlockEvidence {
                bitmask: 0x1fe,
                provenance: "player_snapshot_unlocked_slots_bitmask",
            })
        );
    }

    #[test]
    fn unsupported_build_or_unverified_level_fallback_fails_closed() {
        let progress = |game_build: &str, verification: &str| {
            serde_json::from_value::<ObservedProgress>(json!({
                "runId": "run",
                "level": 2,
                "gameBuild": game_build,
                "sourceStateTickId": 7,
                "sourceObservationId": "observation",
                "verification": verification,
                "provenance": "visual_observation"
            }))
            .expect("progress")
        };

        let wrong_build = progress("1.0.11895", "verified");
        let wrong_build = resolve_board_unlock(None, Some(&wrong_build), 7);
        assert!(wrong_build.evidence.is_none());
        assert_eq!(
            wrong_build.reason,
            "unsupported_game_build_for_level_fallback"
        );

        let unverified = progress("1.0.11894", "unverified");
        let unverified = resolve_board_unlock(None, Some(&unverified), 7);
        assert!(unverified.evidence.is_none());
        assert_eq!(unverified.reason, "unverified_level_observation");
    }

    #[test]
    fn aggregate_open_slots_without_a_contiguous_width_fit_does_not_surface_fill() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [EncounterState] to [ChoiceState]\n",
            "[00:00:00.500] [GameSimEventSocketsUnlocked] UnlockedSocketsBitmask: 1023\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [BoardManager] Card Purchased: InstanceId: b - TemplateIdtpl-b - Target:PlayerSocket_2 - SectionPlayer\n",
            "[00:00:03.000] [BoardManager] Card Purchased: InstanceId: c - TemplateIdtpl-c - Target:PlayerSocket_4 - SectionPlayer\n",
            "[00:00:04.000] [BoardManager] Card Purchased: InstanceId: d - TemplateIdtpl-d - Target:PlayerSocket_5 - SectionPlayer\n",
            "[00:00:05.000] [BoardManager] Card Purchased: InstanceId: e - TemplateIdtpl-e - Target:PlayerSocket_6 - SectionPlayer\n",
            "[00:00:06.000] [BoardManager] Card Purchased: InstanceId: f - TemplateIdtpl-f - Target:PlayerSocket_7 - SectionPlayer\n",
            "[00:00:07.000] [BoardManager] Card Purchased: InstanceId: g - TemplateIdtpl-g - Target:PlayerSocket_8 - SectionPlayer\n",
            "[00:00:08.000] [BoardManager] Card Purchased: InstanceId: h - TemplateIdtpl-h - Target:PlayerSocket_9 - SectionPlayer\n",
            "[00:00:09.000] [BoardManager] Card Purchased: InstanceId: spare - TemplateIdtpl-spare - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[00:00:10.000] [GameSimHandler] Cards Spawned: [a [Player] [Hand] [Socket_0] [Small] | [b [Player] [Hand] [Socket_2] [Small] | [c [Player] [Hand] [Socket_4] [Small] | [d [Player] [Hand] [Socket_5] [Small] | [e [Player] [Hand] [Socket_6] [Small] | [f [Player] [Hand] [Socket_7] [Small] | [g [Player] [Hand] [Socket_8] [Small] | [h [Player] [Hand] [Socket_9] [Small] | \n",
        ));
        let index = TemplateIndex::from_templates([
            template("tpl-a", "A", "Small", &[], &[]),
            template("tpl-b", "B", "Small", &[], &[]),
            template("tpl-c", "C", "Small", &[], &[]),
            template("tpl-d", "D", "Small", &[], &[]),
            template("tpl-e", "E", "Small", &[], &[]),
            template("tpl-f", "F", "Small", &[], &[]),
            template("tpl-g", "G", "Small", &[], &[]),
            template("tpl-h", "H", "Small", &[], &[]),
            template("tpl-spare", "Spare", "Medium", &[], &[]),
        ]);

        let context = model.context(&index, &CompanionConfig::test_default(), 123, None, None);

        assert_eq!(
            context.decision_support.board_capacity.open_unlocked_slots,
            Some(2)
        );
        assert_eq!(
            context.decision_support.board_capacity.fit_placements,
            Some(Vec::new())
        );
        assert!(
            !context
                .available_actions
                .iter()
                .any(|action| action == "fill_board_from_stash_before_player_combat")
        );
    }

    #[test]
    fn verified_storage_observation_exposes_exact_candidate_fit() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [itm_offer_storage [Medium] | \n",
        ));
        let run_id = model.run_id.clone().expect("current run id");
        let source_state_tick_id = model.tick;
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "instanceOverrides": {
                "itm_offer_storage": {
                    "templateId": "tpl-offer",
                    "provenance": "verified_test_mapping"
                }
            },
            "currentStorageObservation": {
                "runId": run_id,
                "sourceStateTickId": source_state_tick_id,
                "sourceObservationId": "observation-storage-open",
                "verification": "verified",
                "usableSlotIds": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                "occupiedSpans": [
                    {"leftSocket": 0, "width": 3, "slotIds": [0, 1, 2]},
                    {"leftSocket": 5, "width": 2, "slotIds": [5, 6]},
                    {"leftSocket": 9, "width": 1, "slotIds": [9]}
                ],
                "provenance": "verified_live_storage_observation"
            }
        }))
        .expect("storage observation config");
        let index = TemplateIndex::from_templates([template(
            "tpl-offer",
            "Storage candidate",
            "Medium",
            &[],
            &[],
        )]);

        let context = model.context(&index, &config, 123, None, None);
        let storage = serde_json::to_value(&context.decision_support)
            .expect("decision support serializes")["storageCapacity"]
            .clone();

        assert_eq!(storage["physicalSlots"], 10);
        assert_eq!(storage["verified"], true);
        assert_eq!(
            storage["usableSlotIds"],
            json!([0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
        );
        assert_eq!(storage["occupiedSlotIds"], json!([0, 1, 2, 5, 6, 9]));
        assert_eq!(storage["openSlotIds"], json!([3, 4, 7, 8]));
        assert_eq!(
            storage["candidateFitPlacements"],
            json!([
                {
                    "instanceId": "itm_offer_storage",
                    "size": "Medium",
                    "width": 2,
                    "leftSocket": 3,
                    "slotIds": [3, 4]
                },
                {
                    "instanceId": "itm_offer_storage",
                    "size": "Medium",
                    "width": 2,
                    "leftSocket": 7,
                    "slotIds": [7, 8]
                }
            ])
        );
        assert_eq!(storage["sourceStateTickId"], source_state_tick_id);
        assert_eq!(storage["sourceObservationId"], "observation-storage-open");
        assert_eq!(storage["provenance"], "verified_live_storage_observation");
    }

    #[test]
    fn exact_purchase_log_sequence_emits_a_new_fenced_positive_receipt() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:59:34.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:59:34.500] [GameSimHandler] Cards Dealt: [itm_SC39ZsU [Small] | \n",
        ));
        let index = TemplateIndex::from_templates([template(
            "b23c5988-7091-4b77-a7fb-134c0d0a3285",
            "Jay Jay shop item",
            "Small",
            &[],
            &[],
        )]);
        let before = model.context(&index, &CompanionConfig::test_default(), 100, None, None);
        let before_tick = before.run.state_tick_id;
        assert!(
            serde_json::to_value(&before).expect("context serializes")["purchaseReceipts"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );

        model.ingest_text(include_str!("../../fixtures/jay-jay-storage-purchase.log"));
        let after = model.context(&index, &CompanionConfig::test_default(), 200, None, None);
        let after_tick = after.run.state_tick_id;
        let value = serde_json::to_value(&after).expect("context serializes");

        assert!(after_tick > before_tick);
        assert_eq!(
            value["purchaseReceipts"],
            json!([{
                "stateTickId": after_tick,
                "instanceId": "itm_SC39ZsU",
                "templateId": "b23c5988-7091-4b77-a7fb-134c0d0a3285",
                "targetSection": "stash",
                "finalSocket": 7,
                "desiredSocket": null,
                "clamp": "unknown",
                "selectItemCommandSeenInWindow": true,
                "correlationWindowMs": 2000,
                "provenance": "player_log_exact_purchase_commit"
            }])
        );
        assert!(after.selection_options.is_empty());
        assert_eq!(after.chest_items[0].instance_id, "itm_SC39ZsU");
        assert_eq!(after.chest_items[0].socket, Some(7));
    }

    #[test]
    fn purchase_without_an_established_run_is_not_published() {
        let mut model = RunModel::default();
        model.ingest_text(include_str!("../../fixtures/jay-jay-storage-purchase.log"));
        let context = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );

        assert_eq!(context.run.run_id, None);
        assert!(context.purchase_receipts.is_empty());
    }

    #[test]
    fn storage_preflight_fails_closed_for_wrong_run_stale_or_unverified_evidence() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [itm_offer_storage [Small] | \n",
        ));
        let run_id = model.run_id.clone().expect("current run id");
        let state_tick_id = model.tick;
        let index = TemplateIndex::from_templates([template(
            "tpl-offer",
            "Storage candidate",
            "Small",
            &[],
            &[],
        )]);
        let cases = [
            (
                "wrong_run",
                "different-run".to_owned(),
                state_tick_id,
                "verified",
                "storage_observation_run_id_mismatch",
            ),
            (
                "stale",
                run_id.clone(),
                state_tick_id.saturating_sub(1),
                "verified",
                "stale_storage_observation_state_tick",
            ),
            (
                "unverified",
                run_id,
                state_tick_id,
                "unverified",
                "unverified_storage_observation",
            ),
        ];

        for (case, observed_run_id, observed_tick_id, verification, expected_reason) in cases {
            let config = serde_json::from_value::<CompanionConfig>(json!({
                "logPath": "",
                "databasePath": "",
                "instanceOverrides": {
                    "itm_offer_storage": {"templateId": "tpl-offer"}
                },
                "currentStorageObservation": {
                    "runId": observed_run_id,
                    "sourceStateTickId": observed_tick_id,
                    "sourceObservationId": format!("observation-{case}"),
                    "verification": verification,
                    "usableSlotIds": [0, 1],
                    "occupiedSpans": [],
                    "provenance": "test"
                }
            }))
            .expect("storage observation config");
            let context = model.context(&index, &config, 123, None, None);
            let storage = serde_json::to_value(context.decision_support.storage_capacity)
                .expect("storage capacity serializes");

            assert_eq!(storage["verified"], false, "{case}");
            assert_eq!(storage["candidateFitPlacements"], Value::Null, "{case}");
            assert_eq!(storage["reason"], expected_reason, "{case}");
        }
    }

    #[test]
    fn storage_observation_outside_physical_slots_fails_closed() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [itm_offer_storage [Small] | \n",
        ));
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "instanceOverrides": {"itm_offer_storage": {"templateId": "tpl-offer"}},
            "currentStorageObservation": {
                "runId": model.run_id.clone(),
                "sourceStateTickId": model.tick,
                "sourceObservationId": "observation-out-of-range",
                "verification": "verified",
                "usableSlotIds": [254],
                "occupiedSpans": [],
                "provenance": "invalid_test_observation"
            }
        }))
        .expect("storage observation config");
        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-offer",
                "Storage candidate",
                "Small",
                &[],
                &[],
            )]),
            &config,
            123,
            None,
            None,
        );
        let storage = context.decision_support.storage_capacity;

        assert!(!storage.verified);
        assert_eq!(storage.candidate_fit_placements, None);
        assert_eq!(storage.reason, "storage_slot_outside_physical_range");
    }

    #[test]
    fn fragmented_storage_does_not_invent_a_contiguous_candidate_fit() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [itm_offer_storage [Medium] | \n",
        ));
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "instanceOverrides": {"itm_offer_storage": {"templateId": "tpl-offer"}},
            "currentStorageObservation": {
                "runId": model.run_id.clone(),
                "sourceStateTickId": model.tick,
                "sourceObservationId": "observation-fragmented",
                "verification": "verified",
                "usableSlotIds": [0, 1, 2, 3],
                "occupiedSpans": [
                    {"leftSocket": 1, "width": 1, "slotIds": [1]},
                    {"leftSocket": 3, "width": 1, "slotIds": [3]}
                ],
                "provenance": "verified_test_observation"
            }
        }))
        .expect("storage observation config");
        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-offer",
                "Storage candidate",
                "Medium",
                &[],
                &[],
            )]),
            &config,
            123,
            None,
            None,
        );

        assert!(context.decision_support.storage_capacity.verified);
        assert_eq!(
            context.decision_support.storage_capacity.open_slot_ids,
            Some(vec![0, 2])
        );
        assert_eq!(
            context
                .decision_support
                .storage_capacity
                .candidate_fit_placements,
            Some(Vec::new())
        );
    }

    #[test]
    fn unknown_candidate_size_does_not_invent_a_storage_fit() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [itm_offer_storage [Unknown] | \n",
        ));
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "instanceOverrides": {"itm_offer_storage": {"templateId": "tpl-offer"}},
            "currentStorageObservation": {
                "runId": model.run_id.clone(),
                "sourceStateTickId": model.tick,
                "sourceObservationId": "observation-unknown-size",
                "verification": "verified",
                "usableSlotIds": [0, 1, 2],
                "occupiedSpans": [],
                "provenance": "verified_test_observation"
            }
        }))
        .expect("storage observation config");
        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-offer",
                "Storage candidate",
                "",
                &[],
                &[],
            )]),
            &config,
            123,
            None,
            None,
        );

        assert!(context.decision_support.storage_capacity.verified);
        assert_eq!(
            context
                .decision_support
                .storage_capacity
                .candidate_fit_placements,
            Some(Vec::new())
        );
    }

    #[test]
    fn resolved_skill_candidate_does_not_receive_an_item_storage_fit() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [GameSimHandler] Cards Dealt: [skl_storage [Small] | \n",
        ));
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "instanceOverrides": {"skl_storage": {"templateId": "tpl-skill"}},
            "currentStorageObservation": {
                "runId": model.run_id.clone(),
                "sourceStateTickId": model.tick,
                "sourceObservationId": "observation-skill-candidate",
                "verification": "verified",
                "usableSlotIds": [0, 1, 2],
                "occupiedSpans": [],
                "provenance": "verified_test_observation"
            }
        }))
        .expect("storage observation config");
        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-skill",
                "Skill candidate",
                "Small",
                &[],
                &[],
            )]),
            &config,
            123,
            None,
            None,
        );

        assert_eq!(
            context
                .decision_support
                .storage_capacity
                .candidate_fit_placements,
            Some(Vec::new())
        );
    }

    #[test]
    fn move_only_is_not_a_purchase_receipt() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:59:34.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:59:35.405] [CardOperationUtility] Successfully moved card to: [itm_SC39ZsU [Player] [Stash] [Socket_7] [Small]\n",
        ));
        let context = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );

        assert_eq!(context.purchase_receipts, Vec::new());
        assert_eq!(context.placement_receipts.len(), 1);
    }

    #[test]
    fn purchase_response_without_send_evidence_is_not_a_receipt() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:59:34.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:59:35.405] [CardOperationUtility] Successfully moved card to: [itm_SC39ZsU [Player] [Stash] [Socket_7] [Small]\n",
            "[00:59:35.645] [NetworkManager] [HttpGameClient] /commands | SelectItemCommand response | 232 ms\n",
            "[00:59:35.645] [BoardManager] Card Purchased: InstanceId: itm_SC39ZsU - TemplateIdb23c5988-7091-4b77-a7fb-134c0d0a3285 - Target:PlayerStorageSocket_7 - SectionStorage\n",
        ));

        let context = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        assert!(context.purchase_receipts.is_empty());
    }

    #[test]
    fn purchase_evidence_must_fit_inside_one_total_correlation_window() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[23:59:59.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.000] [CardOperationUtility] Successfully moved card to: [itm_SC39ZsU [Player] [Stash] [Socket_7] [Small]\n",
            "[00:00:01.900] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[00:00:03.800] [BoardManager] Card Purchased: InstanceId: itm_SC39ZsU - TemplateIdb23c5988-7091-4b77-a7fb-134c0d0a3285 - Target:PlayerStorageSocket_7 - SectionStorage\n",
        ));

        let context = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        assert!(context.purchase_receipts.is_empty());
    }

    #[test]
    fn purchase_receipt_rejects_a_final_socket_outside_the_physical_surface() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:59:34.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:59:35.405] [CardOperationUtility] Successfully moved card to: [itm_SC39ZsU [Player] [Stash] [Socket_254] [Small]\n",
            "[00:59:35.411] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[00:59:35.645] [BoardManager] Card Purchased: InstanceId: itm_SC39ZsU - TemplateIdb23c5988-7091-4b77-a7fb-134c0d0a3285 - Target:PlayerStorageSocket_254 - SectionStorage\n",
        ));
        let context = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );

        assert!(context.purchase_receipts.is_empty());
        assert!(context.placement_receipts.is_empty());
        assert!(context.chest_items.is_empty());
    }

    #[test]
    fn new_run_clears_old_purchase_receipts() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:59:34.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:59:34.100] [AppState] State changed from [StartRunAppState] to [EncounterState]\n",
            "[00:59:35.405] [CardOperationUtility] Successfully moved card to: [itm_SC39ZsU [Player] [Stash] [Socket_7] [Small]\n",
            "[00:59:35.411] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[00:59:35.645] [NetworkManager] [HttpGameClient] /commands | SelectItemCommand response | 232 ms\n",
            "[00:59:35.645] [BoardManager] Card Purchased: InstanceId: itm_SC39ZsU - TemplateIdb23c5988-7091-4b77-a7fb-134c0d0a3285 - Target:PlayerStorageSocket_7 - SectionStorage\n",
        ));
        let before = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        assert_eq!(before.purchase_receipts.len(), 1);

        model.ingest_text(
            "[01:00:00.000] [AppState] State changed from [EncounterState] to [StartRunAppState]\n",
        );

        let after = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            124,
            None,
            None,
        );
        assert!(after.purchase_receipts.is_empty());
    }

    #[test]
    fn partial_purchase_evidence_cannot_cross_a_new_run_fence() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[23:59:59.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[23:59:59.100] [AppState] State changed from [StartRunAppState] to [EncounterState]\n",
            "[23:59:59.800] [CardOperationUtility] Successfully moved card to: [itm_SC39ZsU [Player] [Stash] [Socket_7] [Small]\n",
            "[23:59:59.900] [NetworkManager] [HttpGameClient] Sending SelectItemCommand to /commands\n",
            "[00:00:00.000] [AppState] State changed from [EncounterState] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_SC39ZsU - TemplateIdb23c5988-7091-4b77-a7fb-134c0d0a3285 - Target:PlayerStorageSocket_7 - SectionStorage\n",
        ));
        let context = model.context(
            &TemplateIndex::from_templates([]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );

        assert!(context.purchase_receipts.is_empty());
        assert!(context.placement_receipts.is_empty());
    }

    #[test]
    fn equal_template_and_tier_are_a_direct_fusion_candidate() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [EncounterState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_pearl_owned - TemplateIdtpl-pearl - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_pearl_owned [Player] [Hand] [Socket_0] [Small] | \n",
            "[00:00:03.000] [GameSimHandler] Cards Dealt: [itm_pearl_offer [Small] | \n",
        ));
        let mut pearl = template("tpl-pearl", "Pearl", "Small", &[], &[]);
        pearl.starting_tier = "Gold".into();
        pearl.tier_attributes.insert("Diamond".into(), json!({}));
        let mut config = CompanionConfig::test_default();
        config.instance_overrides.insert(
            "itm_pearl_offer".into(),
            InstanceOverride {
                template_id: Some("tpl-pearl".into()),
                tier: Some("Gold".into()),
                enchantment: None,
                provenance: "verified_test_observation".into(),
            },
        );

        let context = model.context(
            &TemplateIndex::from_templates([pearl]),
            &config,
            123,
            None,
            None,
        );
        let opportunity = &context.decision_support.upgrade_opportunity;
        let fusion = opportunity
            .fusion_pairs
            .iter()
            .find(|pair| {
                [
                    pair.left_instance_id.as_str(),
                    pair.right_instance_id.as_str(),
                ]
                .into_iter()
                .collect::<BTreeSet<_>>()
                    == BTreeSet::from(["itm_pearl_owned", "itm_pearl_offer"])
            })
            .expect("owned Gold Pearl and offered Gold Pearl pair");

        assert_eq!(fusion.status, "direct_fusion_candidate");
        assert_eq!(fusion.can_fuse, Some(true));
        assert_eq!(fusion.resulting_tier.as_deref(), Some("Diamond"));
        assert_eq!(fusion.blocked_reason.as_deref(), None);
        let offer_guard = opportunity
            .mutation_guards
            .iter()
            .find(|guard| guard.instance_id == "itm_pearl_offer")
            .expect("offered Pearl mutation guard");
        assert_eq!(offer_guard.upgrade_opportunity, "direct_fusion_candidate");
        assert_eq!(
            offer_guard.blocked_reason.as_deref(),
            Some("direct_fusion_candidate_must_be_reviewed_before_sell_or_skip")
        );

        let mut unresolved_peer = context.selection_options[0].clone();
        unresolved_peer.instance_id = "itm_unresolved_peer".into();
        unresolved_peer.template_id = None;
        unresolved_peer.identity_status = IdentityStatus::Unresolved;
        unresolved_peer.identity_provenance = None;
        unresolved_peer.tier = None;
        unresolved_peer.tier_provenance = None;
        let verified = Coverage {
            status: CoverageStatus::Verified,
            reason: "verified_test_inventory".into(),
        };
        let guarded = build_upgrade_opportunity(
            "EncounterState",
            &context.board_items,
            &[],
            &[context.selection_options[0].clone(), unresolved_peer],
            &verified,
            &verified,
        );

        assert_eq!(
            guarded
                .fusion_pairs
                .iter()
                .filter(|pair| pair.status == "direct_fusion_candidate")
                .count(),
            1
        );
        assert!(
            guarded
                .mutation_guards
                .iter()
                .all(|guard| guard.upgrade_opportunity == "unknown")
        );
    }

    #[test]
    fn same_template_at_different_tiers_is_explicitly_not_fusible_but_partial_stash_stays_unknown()
    {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [ChoiceState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_illuso_shielded - TemplateIdtpl-illuso - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [BoardManager] Card Purchased: InstanceId: itm_illuso_silver - TemplateIdtpl-illuso - Target:PlayerStorageSocket_0 - SectionStorage\n",
            "[00:00:03.000] [GameSimHandler] Cards Spawned: [itm_illuso_shielded [Player] [Hand] [Socket_0] [Small] | [itm_illuso_silver [Player] [Stash] [Socket_0] [Small] | \n",
        ));
        let mut config = CompanionConfig::test_default();
        config.instance_overrides.insert(
            "itm_illuso_shielded".into(),
            InstanceOverride {
                template_id: Some("tpl-illuso".into()),
                tier: Some("Bronze".into()),
                enchantment: Some("Shielded".into()),
                provenance: "verified_test_observation".into(),
            },
        );
        config.instance_overrides.insert(
            "itm_illuso_silver".into(),
            InstanceOverride {
                template_id: Some("tpl-illuso".into()),
                tier: Some("Silver".into()),
                enchantment: None,
                provenance: "verified_test_observation".into(),
            },
        );

        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-illuso",
                "IllusoRay",
                "Small",
                &[],
                &[],
            )]),
            &config,
            123,
            None,
            None,
        );
        assert_eq!(
            context.state_completeness.stash.status,
            CoverageStatus::Partial
        );
        let opportunity = &context.decision_support.upgrade_opportunity;
        let pair = opportunity
            .fusion_pairs
            .iter()
            .find(|pair| pair.template_id.as_deref() == Some("tpl-illuso"))
            .expect("known Illuso pair");

        assert_eq!(pair.status, "not_fusible");
        assert_eq!(pair.can_fuse, Some(false));
        assert_eq!(pair.blocked_reason.as_deref(), Some("tier_mismatch"));
        assert_eq!(pair.left_tier.as_deref(), Some("Bronze"));
        assert_eq!(pair.right_tier.as_deref(), Some("Silver"));
        assert!(opportunity.mutation_guards.iter().all(|guard| {
            guard.upgrade_opportunity == "unknown"
                && guard.blocked_reason.as_deref()
                    == Some("partial_stash_cannot_prove_no_fusion_candidate")
        }));
    }

    #[test]
    fn upgrade_pedestal_keeps_the_flagship_instance_and_enchantment_evidence() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [PedestalState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_flagship - TemplateIdtpl-flagship - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_flagship [Player] [Hand] [Socket_0] [Large] | \n",
            "[00:00:03.000] [GameSimHandler] Cards Dealt: [itm_upgrade_operation [Small] | \n",
        ));
        let mut upgrade_operation = template(
            "tpl-upgrade-operation",
            "Upgrade an Item",
            "Small",
            &[],
            &[],
        );
        upgrade_operation.tooltips = vec!["Upgrade an item.".into()];
        let mut config = CompanionConfig::test_default();
        config.instance_overrides.insert(
            "itm_flagship".into(),
            InstanceOverride {
                template_id: Some("tpl-flagship".into()),
                tier: Some("Silver".into()),
                enchantment: Some("Sterling".into()),
                provenance: "verified_test_observation".into(),
            },
        );
        config.instance_overrides.insert(
            "itm_upgrade_operation".into(),
            InstanceOverride {
                template_id: Some("tpl-upgrade-operation".into()),
                tier: None,
                enchantment: None,
                provenance: "verified_test_observation".into(),
            },
        );

        let context = model.context(
            &TemplateIndex::from_templates([
                template("tpl-flagship", "Flagship", "Large", &[], &[]),
                upgrade_operation,
            ]),
            &config,
            123,
            None,
            None,
        );
        let opportunity = &context.decision_support.upgrade_opportunity;
        let candidate = opportunity
            .pedestal_candidates
            .iter()
            .find(|candidate| candidate.instance_id == "itm_flagship")
            .expect("Silver Sterling Flagship pedestal candidate");

        assert_eq!(candidate.operation_instance_id, "itm_upgrade_operation");
        assert_eq!(candidate.from_tier, "Silver");
        assert_eq!(candidate.to_tier, "Gold");
        assert!(candidate.preserves_instance_id);
        assert_eq!(candidate.enchantment.as_deref(), Some("Sterling"));
        assert_eq!(
            candidate.enchantment_provenance.as_deref(),
            Some("verified_test_observation")
        );
        let guard = opportunity
            .mutation_guards
            .iter()
            .find(|guard| guard.instance_id == "itm_flagship")
            .expect("Flagship mutation guard");
        assert_eq!(guard.upgrade_opportunity, "pedestal_upgrade_candidate");
        assert_eq!(
            guard.blocked_reason.as_deref(),
            Some("pending_pedestal_upgrade_must_be_resolved_before_sell_or_skip")
        );
    }

    #[test]
    fn unresolved_item_stays_unknown_even_when_inventory_coverage_is_verified() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [EncounterState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_known_pearl - TemplateIdtpl-known-pearl - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_known_pearl [Player] [Hand] [Socket_0] [Small] | \n",
            "[00:00:03.000] [GameSimHandler] Cards Dealt: [itm_unresolved_offer [Small] | \n",
        ));
        let mut pearl = template("tpl-known-pearl", "Pearl", "Small", &[], &[]);
        pearl.starting_tier = "Gold".into();
        let context = model.context(
            &TemplateIndex::from_templates([pearl]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        let verified = Coverage {
            status: CoverageStatus::Verified,
            reason: "verified_test_inventory".into(),
        };

        let opportunity = build_upgrade_opportunity(
            "EncounterState",
            &context.board_items,
            &context.chest_items,
            &context.selection_options,
            &verified,
            &verified,
        );
        let offer_guard = opportunity
            .mutation_guards
            .iter()
            .find(|guard| guard.instance_id == "itm_unresolved_offer")
            .expect("unresolved offer guard");
        let pearl_guard = opportunity
            .mutation_guards
            .iter()
            .find(|guard| guard.instance_id == "itm_known_pearl")
            .expect("known Pearl guard");

        assert_eq!(offer_guard.upgrade_opportunity, "unknown");
        assert_eq!(
            offer_guard.blocked_reason.as_deref(),
            Some("identity_template_or_tier_unresolved")
        );
        assert_eq!(pearl_guard.upgrade_opportunity, "unknown");
        assert_eq!(
            pearl_guard.blocked_reason.as_deref(),
            Some("peer_identity_or_tier_unresolved")
        );
    }

    #[test]
    fn equal_unsupported_tiers_are_unknown_not_maximum_tier() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [EncounterState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_mythic_owned - TemplateIdtpl-mythic - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_mythic_owned [Player] [Hand] [Socket_0] [Small] | \n",
            "[00:00:03.000] [GameSimHandler] Cards Dealt: [itm_mythic_offer [Small] | \n",
        ));
        let mut unsupported = template("tpl-mythic", "Unknown tier item", "Small", &[], &[]);
        unsupported.starting_tier = "Mythic".into();
        let mut config = CompanionConfig::test_default();
        config.instance_overrides.insert(
            "itm_mythic_offer".into(),
            InstanceOverride {
                template_id: Some("tpl-mythic".into()),
                tier: Some("Mythic".into()),
                enchantment: None,
                provenance: "verified_test_observation".into(),
            },
        );

        let context = model.context(
            &TemplateIndex::from_templates([unsupported]),
            &config,
            123,
            None,
            None,
        );
        let pair = context
            .decision_support
            .upgrade_opportunity
            .fusion_pairs
            .iter()
            .find(|pair| pair.template_id.as_deref() == Some("tpl-mythic"))
            .expect("unsupported-tier pair evidence");

        assert_eq!(pair.status, "unknown");
        assert_eq!(pair.can_fuse, None);
        assert_eq!(pair.resulting_tier, None);
        assert_eq!(
            pair.blocked_reason.as_deref(),
            Some("unsupported_or_unknown_tier")
        );
        assert_ne!(pair.blocked_reason.as_deref(), Some("maximum_tier"));
        assert!(
            context
                .decision_support
                .upgrade_opportunity
                .mutation_guards
                .iter()
                .all(|guard| guard.upgrade_opportunity == "unknown")
        );

        let mut diamond_owned = context.board_items[0].clone();
        diamond_owned.tier = Some("Diamond".into());
        let mut diamond_offer = context.selection_options[0].clone();
        diamond_offer.tier = Some("Diamond".into());
        let diamond = fusion_pair_evidence(&diamond_owned, &diamond_offer)
            .expect("exact Diamond pair evidence");
        assert_eq!(diamond.status, "not_fusible");
        assert_eq!(diamond.can_fuse, Some(false));
        assert_eq!(diamond.blocked_reason.as_deref(), Some("maximum_tier"));
    }

    #[test]
    fn conflicting_duplicate_instance_evidence_cannot_self_fuse() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [EncounterState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_duplicate - TemplateIdtpl-duplicate - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_duplicate [Player] [Hand] [Socket_0] [Small] | \n",
        ));
        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-duplicate",
                "Duplicate evidence",
                "Small",
                &[],
                &[],
            )]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        let owned = context.board_items[0].clone();
        let mut peer = owned.clone();
        peer.instance_id = "itm_duplicate_peer".into();
        let mut conflicting_selection = owned.clone();
        conflicting_selection.section = "event_option".into();
        conflicting_selection.selection_category = Some("item".into());
        conflicting_selection.tier = Some("Silver".into());
        conflicting_selection.tier_provenance = Some("conflicting_test_observation".into());
        let verified = Coverage {
            status: CoverageStatus::Verified,
            reason: "verified_test_inventory".into(),
        };

        let opportunity = build_upgrade_opportunity(
            "EncounterState",
            &[owned, peer],
            &[],
            &[conflicting_selection],
            &verified,
            &verified,
        );

        assert!(opportunity.fusion_pairs.is_empty());
        assert_eq!(opportunity.mutation_guards.len(), 2);
        assert!(opportunity.mutation_guards.iter().all(|guard| {
            guard.upgrade_opportunity == "unknown"
                && guard.blocked_reason.as_deref()
                    == Some("conflicting_duplicate_instance_evidence")
        }));
    }

    #[test]
    fn three_distinct_equal_tier_instances_keep_three_canonical_pairs() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [EncounterState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_pearl_a - TemplateIdtpl-pearl-three - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_pearl_a [Player] [Hand] [Socket_0] [Small] | \n",
        ));
        let mut pearl = template("tpl-pearl-three", "Pearl", "Small", &[], &[]);
        pearl.starting_tier = "Gold".into();
        let context = model.context(
            &TemplateIndex::from_templates([pearl]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        let first = context.board_items[0].clone();
        let mut second = first.clone();
        second.instance_id = "itm_pearl_b".into();
        second.section = "stash".into();
        let mut third = first.clone();
        third.instance_id = "itm_pearl_c".into();
        third.section = "event_option".into();
        third.selection_category = Some("item".into());
        let verified = Coverage {
            status: CoverageStatus::Verified,
            reason: "verified_test_inventory".into(),
        };

        let opportunity = build_upgrade_opportunity(
            "EncounterState",
            &[first],
            &[second],
            &[third],
            &verified,
            &verified,
        );
        let canonical_pairs = opportunity
            .fusion_pairs
            .iter()
            .map(|pair| {
                let mut ids = [
                    pair.left_instance_id.clone(),
                    pair.right_instance_id.clone(),
                ];
                ids.sort();
                ids
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(opportunity.fusion_pairs.len(), 3);
        assert_eq!(canonical_pairs.len(), 3);
        assert!(
            opportunity
                .fusion_pairs
                .iter()
                .all(|pair| pair.left_instance_id < pair.right_instance_id)
        );
    }

    #[test]
    fn identical_duplicate_instance_evidence_is_deduplicated_without_a_self_pair() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [EncounterState]\n",
            "[00:00:01.000] [BoardManager] Card Purchased: InstanceId: itm_same - TemplateIdtpl-same - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:02.000] [GameSimHandler] Cards Spawned: [itm_same [Player] [Hand] [Socket_0] [Small] | \n",
        ));
        let context = model.context(
            &TemplateIndex::from_templates([template(
                "tpl-same",
                "Same evidence",
                "Small",
                &[],
                &[],
            )]),
            &CompanionConfig::test_default(),
            123,
            None,
            None,
        );
        let owned = context.board_items[0].clone();
        let mut selection = owned.clone();
        selection.section = "event_option".into();
        selection.selection_category = Some("item".into());
        let verified = Coverage {
            status: CoverageStatus::Verified,
            reason: "verified_test_inventory".into(),
        };

        let opportunity = build_upgrade_opportunity(
            "EncounterState",
            &[owned],
            &[],
            &[selection],
            &verified,
            &verified,
        );

        assert!(opportunity.fusion_pairs.is_empty());
        assert_eq!(opportunity.mutation_guards.len(), 1);
        assert_eq!(opportunity.mutation_guards[0].upgrade_opportunity, "none");
        assert_eq!(opportunity.mutation_guards[0].blocked_reason, None);
    }

    #[test]
    fn authoritative_sell_and_atomic_swap_publish_fenced_inventory_mutation_receipts() {
        let source = include_str!("../../fixtures/inventory-mutations-authoritative.log");
        let mut model = RunModel::default();
        model.ingest_text(source);
        let mut sold_template = template(
            "014d9c98-e823-443c-98a3-6367ab81c956",
            "Test on-sell item",
            "Small",
            &[],
            &[],
        );
        sold_template.tooltips =
            vec!["When you sell this, your leftmost Ammo item gains +2 Max Ammo.".into()];
        let context = serde_json::to_value(model.context(
            &TemplateIndex::from_templates([sold_template.clone()]),
            &CompanionConfig::test_default(),
            source.len() as u64,
            None,
            None,
        ))
        .expect("context serializes");
        let receipts = context["inventoryMutationReceipts"]
            .as_array()
            .expect("inventory mutation receipts");
        let swap = receipts
            .iter()
            .find(|receipt| receipt["operation"] == "swap_reorder")
            .expect("atomic swap receipt");
        let sell = receipts
            .iter()
            .find(|receipt| receipt["operation"] == "sell")
            .expect("sell receipt");

        assert_eq!(
            swap["exactInstanceIds"],
            json!(["itm_wFPcN6k", "itm_FFIhpYN"])
        );
        assert_eq!(
            swap["locations"],
            json!([
                {
                    "instanceId": "itm_wFPcN6k",
                    "before": {"section": "board", "socket": 2},
                    "after": {"section": "board", "socket": 1}
                },
                {
                    "instanceId": "itm_FFIhpYN",
                    "before": {"section": "board", "socket": 1},
                    "after": {"section": "board", "socket": 2}
                }
            ])
        );
        assert_eq!(swap["evidence"]["commandSent"], true);
        assert_eq!(swap["evidence"]["commandResponse"], true);
        assert_eq!(swap["evidence"]["requestId"], "4");
        assert_eq!(swap["evidence"]["messageId"], "VC8");
        assert_eq!(swap["evidence"]["messageOwnerCompleted"], true);
        assert_eq!(swap["finalized"], false);
        assert_eq!(swap["status"], "awaiting_verified_observation");
        assert_eq!(swap["reason"], "log_commit_requires_verified_observation");
        assert_eq!(swap["requiresVerifiedObservation"], true);

        assert_eq!(sell["exactInstanceIds"], json!(["itm_b0UO1QF"]));
        assert_eq!(
            sell["locations"],
            json!([{
                "instanceId": "itm_b0UO1QF",
                "before": {"section": "board", "socket": 3},
                "after": {"section": null, "socket": null}
            }])
        );
        assert_eq!(sell["sellExpectation"]["valueGold"], 1);
        assert_eq!(
            sell["sellExpectation"]["effect"]["descriptions"],
            json!(["When you sell this, your leftmost Ammo item gains +2 Max Ammo."])
        );
        assert_eq!(
            sell["sellExpectation"]["effect"]["status"],
            "resolved_on_sell_tooltip"
        );
        assert_eq!(sell["evidence"]["commandSent"], true);
        assert_eq!(sell["evidence"]["commandResponse"], true);
        assert_eq!(sell["evidence"]["commitSeen"], true);
        assert_eq!(sell["evidence"]["requestId"], "21");
        assert_eq!(sell["evidence"]["messageId"], "o5R");
        assert_eq!(sell["evidence"]["messageOwnerCompleted"], true);
        assert_eq!(sell["finalized"], false);
        assert_eq!(sell["status"], "awaiting_verified_observation");
        assert_eq!(sell["reason"], "log_commit_requires_verified_observation");
        assert_eq!(sell["requiresVerifiedObservation"], true);

        for receipt in [swap, sell] {
            assert!(receipt["runId"].as_str().is_some_and(|id| !id.is_empty()));
            assert!(receipt["stateTickId"].as_u64().is_some_and(|tick| tick > 0));
            assert!(
                receipt["logCursor"]
                    .as_u64()
                    .is_some_and(|cursor| cursor > 0)
            );
            assert!(receipt["mutationFence"]["runId"].is_string());
            assert!(receipt["mutationFence"]["stateTickId"].is_u64());
            assert!(receipt["mutationFence"]["logCursor"].is_u64());
            assert_eq!(receipt["sameFencePolicy"], "deny_repeat_use_cached_receipt");
        }

        let swap_config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentInventoryMutationObservations": [{
                "runId": swap["runId"],
                "sourceStateTickId": swap["stateTickId"],
                "receiptLogCursor": swap["logCursor"],
                "sourceObservationId": "fresh-frame-swap",
                "verification": "verified",
                "operation": "swap_reorder",
                "exactInstanceIds": swap["exactInstanceIds"],
                "locations": [
                    {"instanceId": "itm_wFPcN6k", "section": "board", "socket": 1},
                    {"instanceId": "itm_FFIhpYN", "section": "board", "socket": 2}
                ],
                "observedGoldDelta": null,
                "effectStatus": "not_applicable",
                "provenance": "verified_fresh_post_action_frame"
            }]
        }))
        .expect("swap observation config");
        let swap_verified = serde_json::to_value(model.context(
            &TemplateIndex::from_templates([sold_template.clone()]),
            &swap_config,
            source.len() as u64,
            None,
            None,
        ))
        .expect("verified swap context serializes");
        let swap_verified = swap_verified["inventoryMutationReceipts"]
            .as_array()
            .and_then(|receipts| {
                receipts
                    .iter()
                    .find(|receipt| receipt["operation"] == "swap_reorder")
            })
            .expect("verified swap receipt");
        assert_eq!(swap_verified["finalized"], true);
        assert_eq!(swap_verified["status"], "succeeded");
        assert_eq!(
            swap_verified["reason"],
            "exact_atomic_reorder_and_verified_observation"
        );
        assert_eq!(swap_verified["verifiedObservationId"], "fresh-frame-swap");

        let sell_config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentInventoryMutationObservations": [{
                "runId": sell["runId"],
                "sourceStateTickId": sell["stateTickId"],
                "receiptLogCursor": sell["logCursor"],
                "sourceObservationId": "fresh-frame-sell",
                "verification": "verified",
                "operation": "sell",
                "exactInstanceIds": sell["exactInstanceIds"],
                "locations": [
                    {"instanceId": "itm_b0UO1QF", "section": null, "socket": null}
                ],
                "observedGoldDelta": 1,
                "effectStatus": "verified",
                "provenance": "verified_fresh_post_action_frame"
            }]
        }))
        .expect("sell observation config");
        let sell_verified = serde_json::to_value(model.context(
            &TemplateIndex::from_templates([sold_template]),
            &sell_config,
            source.len() as u64,
            None,
            None,
        ))
        .expect("verified sell context serializes");
        let sell_verified = sell_verified["inventoryMutationReceipts"]
            .as_array()
            .and_then(|receipts| {
                receipts
                    .iter()
                    .find(|receipt| receipt["operation"] == "sell")
            })
            .expect("verified sell receipt");
        assert_eq!(sell_verified["finalized"], true);
        assert_eq!(sell_verified["status"], "succeeded");
        assert_eq!(
            sell_verified["reason"],
            "exact_sell_and_verified_value_effect_observation"
        );
        assert_eq!(sell_verified["verifiedObservationId"], "fresh-frame-sell");
    }

    #[test]
    fn completed_mutation_keeps_its_original_log_cursor_and_cached_fence() {
        let source = concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:00.101] [BoardManager] Card Purchased: InstanceId: itm_b - TemplateIdtpl-b - Target:PlayerSocket_1 - SectionPlayer\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_a to Socket_1\n",
            "[00:00:01.001] [CardOperationUtility] Successfully moved card itm_b to Socket_0\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:01.015] [NetworkManager] [HttpGameClient] Captured request id: 1\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 10 ms\n",
            "[00:00:01.021] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [owner]\n",
            "[00:00:01.022] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner]\n",
        );
        let expected_cursor = source.len() as u64;
        let mut model = RunModel::default();
        model.ingest_text(source);
        let first = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            expected_cursor,
            None,
            None,
        ))
        .expect("first context serializes");
        model.ingest_text(
            "[00:00:02.000] [AppState] State changed from [ChoiceState] to [EncounterState]\n",
        );
        let second = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            expected_cursor + 80,
            None,
            None,
        ))
        .expect("second context serializes");

        assert_eq!(
            first["inventoryMutationReceipts"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            second["inventoryMutationReceipts"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(
            first["inventoryMutationReceipts"][0],
            second["inventoryMutationReceipts"][0]
        );
        assert_eq!(
            second["inventoryMutationReceipts"][0]["logCursor"],
            expected_cursor
        );
    }

    #[test]
    fn partial_or_ambiguous_mutation_evidence_stays_pending_and_fails_closed() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_unknown to Socket_4\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 10 ms\n",
        ));
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentStorageObservation": {
                "runId": model.run_id.clone(),
                "sourceStateTickId": model.tick,
                "sourceObservationId": "stale-or-companion-only-snapshot",
                "verification": "verified",
                "usableSlotIds": [0, 1, 2, 3, 4],
                "occupiedSpans": [{"leftSocket": 4, "width": 1, "slotIds": [4]}],
                "provenance": "must_not_supply_mutation_before_state"
            }
        }))
        .expect("config");
        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &config,
            500,
            None,
            None,
        ))
        .expect("context serializes");
        let receipt = &context["inventoryMutationReceipts"][0];

        assert_eq!(receipt["exactInstanceIds"], json!(["itm_unknown"]));
        assert_eq!(receipt["locations"][0]["before"]["section"], Value::Null);
        assert_eq!(receipt["locations"][0]["before"]["socket"], Value::Null);
        assert_eq!(receipt["finalized"], false);
        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["reason"], "missing_authoritative_before_location");
        assert_eq!(receipt["requiresVerifiedObservation"], true);
    }

    #[test]
    fn a_second_command_before_completion_permanently_conflicts_the_mutation_batch() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_sell - TemplateIdtpl-sell - Target:PlayerStorageSocket_2 - SectionStorage\n",
            "[00:00:01.000] [CardOperationUtility] Successfully removed item itm_sell from player's inventory\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending SellCardCommand to /commands\n",
            "[00:00:01.011] [NetworkManager] [HttpGameClient] Sending SellCardCommand to /commands\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | SellCardCommand response | 10 ms\n",
            "[00:00:01.021] [BoardManager] Sold Card itm_sell for 2 gold.\n",
        ));
        let context = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            700,
            None,
            None,
        ))
        .expect("context serializes");
        let receipt = &context["inventoryMutationReceipts"][0];

        assert_eq!(receipt["evidence"]["ambiguous"], true);
        assert_eq!(receipt["finalized"], false);
        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["reason"], "ambiguous_command_evidence");
    }

    #[test]
    fn one_sided_move_cannot_be_finalized_as_an_atomic_swap() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_a to Socket_1\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:01.015] [NetworkManager] [HttpGameClient] Captured request id: 9\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 10 ms\n",
            "[00:00:01.021] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [one-sided]\n",
            "[00:00:01.022] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [one-sided]\n",
        ));
        let before = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            800,
            None,
            None,
        ))
        .expect("context serializes");
        let receipt = &before["inventoryMutationReceipts"][0];
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentInventoryMutationObservations": [{
                "runId": receipt["runId"],
                "sourceStateTickId": receipt["stateTickId"],
                "receiptLogCursor": receipt["logCursor"],
                "sourceObservationId": "frame-one-sided",
                "verification": "verified",
                "operation": "swap_reorder",
                "exactInstanceIds": ["itm_a"],
                "locations": [{"instanceId": "itm_a", "section": "board", "socket": 1}],
                "observedGoldDelta": null,
                "effectStatus": "not_applicable",
                "provenance": "verified_but_not_a_swap"
            }]
        }))
        .expect("observation config");
        let after = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &config,
            800,
            None,
            None,
        ))
        .expect("context serializes");
        let receipt = &after["inventoryMutationReceipts"][0];

        assert_eq!(receipt["finalized"], false);
        assert_eq!(receipt["status"], "pending");
        assert_eq!(receipt["reason"], "not_exact_two_instance_bijective_swap");
    }

    #[test]
    fn command_response_requires_a_captured_request_and_exact_message_owner_finish() {
        let mut missing_request = RunModel::default();
        missing_request.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:00.101] [BoardManager] Card Purchased: InstanceId: itm_b - TemplateIdtpl-b - Target:PlayerSocket_1 - SectionPlayer\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_a to Socket_1\n",
            "[00:00:01.001] [CardOperationUtility] Successfully moved card itm_b to Socket_0\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 10 ms\n",
            "[00:00:01.021] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [owner]\n",
            "[00:00:01.022] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner]\n",
        ));
        let missing_request = serde_json::to_value(missing_request.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            900,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(
            missing_request["inventoryMutationReceipts"][0]["finalized"],
            false
        );
        assert_eq!(
            missing_request["inventoryMutationReceipts"][0]["reason"],
            "command_response_without_captured_request_id"
        );

        let mut mismatched_owner = RunModel::default();
        mismatched_owner.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:00.101] [BoardManager] Card Purchased: InstanceId: itm_b - TemplateIdtpl-b - Target:PlayerSocket_1 - SectionPlayer\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_a to Socket_1\n",
            "[00:00:01.001] [CardOperationUtility] Successfully moved card itm_b to Socket_0\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:01.015] [NetworkManager] [HttpGameClient] Captured request id: 10\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 10 ms\n",
            "[00:00:01.021] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [owner-a]\n",
            "[00:00:01.022] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner-b]\n",
        ));
        let mismatched_owner = serde_json::to_value(mismatched_owner.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            900,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(
            mismatched_owner["inventoryMutationReceipts"][0]["finalized"],
            false
        );
        assert_eq!(
            mismatched_owner["inventoryMutationReceipts"][0]["reason"],
            "message_owner_mismatch"
        );
    }

    #[test]
    fn duplicate_or_late_message_owner_events_permanently_invalidate_the_receipt() {
        let valid_prefix = concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:00.101] [BoardManager] Card Purchased: InstanceId: itm_b - TemplateIdtpl-b - Target:PlayerSocket_1 - SectionPlayer\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_a to Socket_1\n",
            "[00:00:01.001] [CardOperationUtility] Successfully moved card itm_b to Socket_0\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:01.015] [NetworkManager] [HttpGameClient] Captured request id: 10\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 10 ms\n",
            "[00:00:01.021] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [owner]\n",
        );
        let mut duplicate_processing = RunModel::default();
        duplicate_processing.ingest_text(valid_prefix);
        duplicate_processing.ingest_text(concat!(
            "[00:00:01.022] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [owner]\n",
            "[00:00:01.023] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner]\n",
        ));
        let duplicate = serde_json::to_value(duplicate_processing.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(
            duplicate["inventoryMutationReceipts"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            duplicate["inventoryMutationReceipts"][0]["logCommitted"],
            false
        );
        assert_eq!(
            duplicate["inventoryMutationReceipts"][0]["reason"],
            "duplicate_message_owner_processing"
        );

        let mut late_finish = RunModel::default();
        late_finish.ingest_text(valid_prefix);
        late_finish.ingest_text(concat!(
            "[00:00:01.022] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner]\n",
            "[00:00:01.023] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner]\n",
        ));
        let late = serde_json::to_value(late_finish.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(
            late["inventoryMutationReceipts"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(late["inventoryMutationReceipts"][0]["logCommitted"], false);
        assert_eq!(
            late["inventoryMutationReceipts"][0]["reason"],
            "message_owner_reused_after_archive"
        );
    }

    #[test]
    fn sell_commit_must_be_inside_the_bound_message_owner_and_request_cursor_window() {
        let mut early_sell = RunModel::default();
        early_sell.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_sell - TemplateIdtpl-sell - Target:PlayerStorageSocket_2 - SectionStorage\n",
            "[00:00:01.000] [CardOperationUtility] Successfully removed item itm_sell from player's inventory\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending SellCardCommand to /commands\n",
            "[00:00:01.015] [NetworkManager] [HttpGameClient] Captured request id: 1\n",
            "[00:00:01.020] [NetworkManager] [HttpGameClient] /commands | SellCardCommand response | 10 ms\n",
            "[00:00:01.021] [BoardManager] Sold Card itm_sell for 2 gold.\n",
            "[00:00:01.022] [GameMessageHandler] Processing [NetMessageGameSim]  |  Id: [owner]\n",
            "[00:00:01.023] [GameMessageHandler] Finished processing [NetMessageGameSim]  |  Id: [owner]\n",
        ));
        let early = serde_json::to_value(early_sell.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(early["inventoryMutationReceipts"][0]["logCommitted"], false);
        assert_eq!(
            early["inventoryMutationReceipts"][0]["reason"],
            "sell_commit_outside_message_owner"
        );

        let mut late_request = RunModel::default();
        late_request.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.100] [BoardManager] Card Purchased: InstanceId: itm_a - TemplateIdtpl-a - Target:PlayerSocket_0 - SectionPlayer\n",
            "[00:00:00.101] [BoardManager] Card Purchased: InstanceId: itm_b - TemplateIdtpl-b - Target:PlayerSocket_1 - SectionPlayer\n",
            "[00:00:01.000] [CardOperationUtility] Successfully moved card itm_a to Socket_1\n",
            "[00:00:01.001] [CardOperationUtility] Successfully moved card itm_b to Socket_0\n",
            "[00:00:01.010] [NetworkManager] [HttpGameClient] Sending MoveItemCommand to /commands\n",
            "[00:00:04.015] [NetworkManager] [HttpGameClient] Captured request id: 2\n",
            "[00:00:04.020] [NetworkManager] [HttpGameClient] /commands | MoveItemCommand response | 5 ms\n",
        ));
        let late = serde_json::to_value(late_request.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(late["inventoryMutationReceipts"][0]["logCommitted"], false);
        assert_eq!(
            late["inventoryMutationReceipts"][0]["reason"],
            "request_id_outside_command_correlation_window"
        );
    }

    #[test]
    fn stale_or_repeated_verified_observation_never_creates_or_finalizes_another_receipt() {
        let mut model = RunModel::default();
        model.ingest_text(include_str!(
            "../../fixtures/inventory-mutations-authoritative.log"
        ));
        let base = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        let swap = base["inventoryMutationReceipts"]
            .as_array()
            .and_then(|receipts| {
                receipts
                    .iter()
                    .find(|receipt| receipt["operation"] == "swap_reorder")
            })
            .expect("swap receipt");
        let stale_config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentInventoryMutationObservations": [{
                "runId": swap["runId"],
                "sourceStateTickId": swap["stateTickId"],
                "receiptLogCursor": swap["logCursor"].as_u64().unwrap() - 1,
                "sourceObservationId": "stale-frame",
                "verification": "verified",
                "operation": "swap_reorder",
                "exactInstanceIds": swap["exactInstanceIds"],
                "locations": [
                    {"instanceId": "itm_wFPcN6k", "section": "board", "socket": 1},
                    {"instanceId": "itm_FFIhpYN", "section": "board", "socket": 2}
                ],
                "observedGoldDelta": null,
                "effectStatus": "not_applicable",
                "provenance": "stale_test_observation"
            }]
        }))
        .expect("stale config");
        let first = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &stale_config,
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        let second = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &stale_config,
            1_100,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(
            first["inventoryMutationReceipts"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            first["inventoryMutationReceipts"],
            second["inventoryMutationReceipts"]
        );
        let stale_swap = first["inventoryMutationReceipts"]
            .as_array()
            .and_then(|receipts| {
                receipts
                    .iter()
                    .find(|receipt| receipt["operation"] == "swap_reorder")
            })
            .expect("stale swap receipt");
        assert_eq!(stale_swap["finalized"], false);
        assert_eq!(stale_swap["verifiedObservationId"], Value::Null);
    }

    #[test]
    fn any_conflicting_verified_payload_on_the_same_mutation_fence_fails_closed() {
        let mut model = RunModel::default();
        model.ingest_text(include_str!(
            "../../fixtures/inventory-mutations-authoritative.log"
        ));
        let mut sold_template = template(
            "014d9c98-e823-443c-98a3-6367ab81c956",
            "Test on-sell item",
            "Small",
            &[],
            &[],
        );
        sold_template.tooltips = vec!["When you sell this, gain a test effect.".into()];
        let base = serde_json::to_value(model.context(
            &TemplateIndex::from_templates([sold_template.clone()]),
            &CompanionConfig::test_default(),
            1_000,
            None,
            None,
        ))
        .expect("context serializes");
        let receipts = base["inventoryMutationReceipts"]
            .as_array()
            .expect("mutation receipts");
        let swap = receipts
            .iter()
            .find(|receipt| receipt["operation"] == "swap_reorder")
            .expect("swap receipt");
        let sell = receipts
            .iter()
            .find(|receipt| receipt["operation"] == "sell")
            .expect("sell receipt");
        let config = serde_json::from_value::<CompanionConfig>(json!({
            "logPath": "",
            "databasePath": "",
            "currentInventoryMutationObservations": [
                {
                    "runId": swap["runId"],
                    "sourceStateTickId": swap["stateTickId"],
                    "receiptLogCursor": swap["logCursor"],
                    "sourceObservationId": "correct-swap",
                    "verification": "verified",
                    "operation": "swap_reorder",
                    "exactInstanceIds": swap["exactInstanceIds"],
                    "locations": [
                        {"instanceId": "itm_wFPcN6k", "section": "board", "socket": 1},
                        {"instanceId": "itm_FFIhpYN", "section": "board", "socket": 2}
                    ],
                    "observedGoldDelta": null,
                    "effectStatus": "not_applicable",
                    "provenance": "correct_swap"
                },
                {
                    "runId": swap["runId"],
                    "sourceStateTickId": swap["stateTickId"],
                    "receiptLogCursor": swap["logCursor"],
                    "sourceObservationId": "conflicting-swap-fence",
                    "verification": "verified",
                    "operation": "sell",
                    "exactInstanceIds": ["itm_conflict"],
                    "locations": [
                        {"instanceId": "itm_conflict", "section": null, "socket": null}
                    ],
                    "observedGoldDelta": 99,
                    "effectStatus": "verified",
                    "provenance": "conflicting_swap_payload"
                },
                {
                    "runId": sell["runId"],
                    "sourceStateTickId": sell["stateTickId"],
                    "receiptLogCursor": sell["logCursor"],
                    "sourceObservationId": "correct-sell",
                    "verification": "verified",
                    "operation": "sell",
                    "exactInstanceIds": sell["exactInstanceIds"],
                    "locations": [
                        {"instanceId": "itm_b0UO1QF", "section": null, "socket": null}
                    ],
                    "observedGoldDelta": 1,
                    "effectStatus": "verified",
                    "provenance": "correct_sell"
                },
                {
                    "runId": sell["runId"],
                    "sourceStateTickId": sell["stateTickId"],
                    "receiptLogCursor": sell["logCursor"],
                    "sourceObservationId": "conflicting-sell-fence",
                    "verification": "verified",
                    "operation": "swap_reorder",
                    "exactInstanceIds": ["itm_x", "itm_y"],
                    "locations": [
                        {"instanceId": "itm_x", "section": "board", "socket": 0},
                        {"instanceId": "itm_y", "section": "board", "socket": 1}
                    ],
                    "observedGoldDelta": null,
                    "effectStatus": "not_applicable",
                    "provenance": "conflicting_sell_payload"
                }
            ]
        }))
        .expect("conflicting observation config");
        let context = serde_json::to_value(model.context(
            &TemplateIndex::from_templates([sold_template]),
            &config,
            1_000,
            None,
            None,
        ))
        .expect("context serializes");

        for receipt in context["inventoryMutationReceipts"]
            .as_array()
            .expect("mutation receipts")
        {
            assert_eq!(receipt["finalized"], false);
            assert_eq!(receipt["status"], "pending");
            assert_eq!(receipt["reason"], "ambiguous_verified_observation_evidence");
            assert_eq!(receipt["verifiedObservationId"], Value::Null);
        }
    }

    #[test]
    fn inventory_mutation_receipts_are_run_scoped_and_bounded() {
        let mut model = RunModel::default();
        model.ingest_text(concat!(
            "[00:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[00:00:00.010] [AppState] State changed from [StartRunAppState] to [ChoiceState]\n",
        ));
        for index in 0..65 {
            let minute = index / 60;
            let second = index % 60;
            model.ingest_text(&format!(
                concat!(
                    "[00:{minute:02}:{second:02}.000] [BoardManager] Card Purchased: InstanceId: itm_{index:02} - TemplateIdtpl-{index:02} - Target:PlayerStorageSocket_0 - SectionStorage\n",
                    "[00:{minute:02}:{second:02}.001] [CardOperationUtility] Successfully removed item itm_{index:02} from player's inventory\n",
                    "[00:{minute:02}:{second:02}.002] [NetworkManager] [HttpGameClient] Sending SellCardCommand to /commands\n",
                    "[00:{minute:02}:{second:02}.003] [NetworkManager] [HttpGameClient] /commands | SellCardCommand response | 1 ms\n",
                    "[00:{minute:02}:{second:02}.004] [BoardManager] Sold Card itm_{index:02} for 1 gold.\n"
                ),
                minute = minute,
                second = second,
                index = index,
            ));
        }
        let before_new_run = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            10_000,
            None,
            None,
        ))
        .expect("context serializes");
        let receipts = before_new_run["inventoryMutationReceipts"]
            .as_array()
            .expect("receipts");
        assert_eq!(receipts.len(), 64);
        assert_eq!(receipts[0]["exactInstanceIds"], json!(["itm_01"]));
        assert_eq!(receipts[63]["exactInstanceIds"], json!(["itm_64"]));

        model.ingest_text(
            "[02:00:00.000] [AppState] State changed from [ChoiceState] to [StartRunAppState]\n",
        );
        let after_new_run = serde_json::to_value(model.context(
            &TemplateIndex::default(),
            &CompanionConfig::test_default(),
            10_100,
            None,
            None,
        ))
        .expect("context serializes");
        assert_eq!(after_new_run["inventoryMutationReceipts"], json!([]));
    }
}
