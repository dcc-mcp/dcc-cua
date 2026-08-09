use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

mod ten_win;

use ten_win::{BuildEvidence, TenWinCorpus};

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
    pub decision_policy: DecisionPolicyConfig,
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
            decision_policy: DecisionPolicyConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedProgress {
    pub day: Option<u8>,
    pub hour: Option<u8>,
    pub level: Option<u8>,
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

#[derive(Clone, Debug, Default)]
pub struct RunModel {
    app_state: String,
    tick: u64,
    template_ids: BTreeMap<String, String>,
    player_items: BTreeMap<String, CardLocation>,
    selection_options: Vec<Candidate>,
    instance_upgrade_counts: BTreeMap<String, u8>,
    selected_skill_instance_ids: Vec<String>,
    board_snapshot_seen: bool,
    stash_delta_seen: bool,
    pending_new_purchase_instances: BTreeSet<String>,
}

impl RunModel {
    pub fn ingest_text(&mut self, text: &str) {
        for line in text.lines() {
            self.ingest_line(line);
        }
    }

    fn ingest_line(&mut self, line: &str) {
        let mut changed = false;
        if line.contains("State changed from ")
            && let Some(state) = line
                .rsplit(" to [")
                .next()
                .and_then(|tail| tail.strip_suffix(']'))
            && self.app_state != state
        {
            if state == "StartRunAppState" {
                self.player_items.clear();
                self.selection_options.clear();
                self.instance_upgrade_counts.clear();
                self.selected_skill_instance_ids.clear();
                self.board_snapshot_seen = false;
                self.stash_delta_seen = false;
                self.pending_new_purchase_instances.clear();
            }
            self.app_state = state.to_owned();
            changed = true;
        }

        if let Some(rest) = line.split("Card Purchased: InstanceId: ").nth(1)
            && let Some((instance_id, rest)) = rest.split_once(" - TemplateId")
            && let Some((template_id, rest)) = rest.split_once(" - Target:")
        {
            let instance_id = instance_id.trim();
            let template_id = template_id.trim();
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
            if let Some(location) = target_location(target, None) {
                self.player_items.insert(instance_id.to_owned(), location);
            }
            self.selection_options
                .retain(|candidate| candidate.instance_id != instance_id);
            if is_player_target {
                self.pending_new_purchase_instances.clear();
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
            self.selected_skill_instance_ids
                .push(instance_id.to_owned());
            changed = true;
        }

        if let Some(rest) = line.split("Cards Dealt: ").nth(1) {
            self.selection_options = parse_card_segments(rest)
                .into_iter()
                .map(|(instance_id, size)| Candidate { instance_id, size })
                .collect();
            changed = true;
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
            for instance_id in rest.split('|').map(str::trim).filter(|id| !id.is_empty()) {
                self.player_items.remove(instance_id);
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
            changed |= self.player_items.remove(instance_id).is_some();
            self.selection_options
                .retain(|candidate| candidate.instance_id != instance_id);
        } else if let Some(instance_id) = line
            .split("Sold Card ")
            .nth(1)
            .and_then(|rest| rest.split(" for ").next())
            .map(str::trim)
            .filter(|instance_id| !instance_id.is_empty())
        {
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
                self.player_items.insert(
                    instance_id.into(),
                    CardLocation {
                        section: section.into(),
                        socket: parse_socket(rest),
                        size,
                    },
                );
                changed = true;
            }
        } else if let Some(rest) = line.split("Successfully moved card ").nth(1)
            && let Some((instance_id, socket)) = rest.split_once(" to Socket_")
            && let Ok(socket) = socket.trim().parse::<u8>()
        {
            if let Some(location) = self.player_items.get_mut(instance_id.trim()) {
                location.socket = Some(socket);
            }
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
        let mut board_items = Vec::new();
        let mut chest_items = Vec::new();
        for (instance_id, location) in &self.player_items {
            let card = self.resolve_card(instance_id, location, index, config, card_identity_cache);
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
        let state_completeness = build_state_completeness(
            self,
            &board_items,
            &chest_items,
            &selection_options,
            &player_skill_instances,
            config.current_progress.as_ref(),
        );
        let decision_support = build_decision_support(
            &relationship_graph,
            &selection_options,
            config.current_progress.as_ref(),
            &config.decision_policy,
        );
        let available_actions =
            build_available_actions(&self.app_state, &selection_options, &unresolved_regions);
        Context {
            schema_version: "2.5.0".into(),
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
                visual_fallback: "localized_unknown_regions_only".into(),
            },
            run: RunState {
                app_state: self.app_state.clone(),
                progress: config.current_progress.clone(),
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
            available_actions,
        }
    }

    fn resolve_card(
        &self,
        instance_id: &str,
        location: &CardLocation,
        index: &TemplateIndex,
        config: &CompanionConfig,
        card_identity_cache: Option<&CardIdentityCache>,
    ) -> CardInstance {
        let override_data = config.instance_overrides.get(instance_id);
        let template_id = override_data
            .and_then(|value| value.template_id.as_deref())
            .or_else(|| self.template_ids.get(instance_id).map(String::as_str));
        let template = template_id.and_then(|id| index.get(id));
        let provenance = if override_data
            .and_then(|value| value.template_id.as_ref())
            .is_some()
        {
            override_data.map(|value| value.provenance.clone())
        } else if template_id.is_some() {
            Some("player_log_template_id".into())
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

fn target_location(target: &str, size: Option<String>) -> Option<CardLocation> {
    if let Some(socket) = target.strip_prefix("PlayerSocket_") {
        return Some(CardLocation {
            section: "board".into(),
            socket: socket.parse().ok(),
            size: size.unwrap_or_default(),
        });
    }
    target
        .strip_prefix("PlayerStorageSocket_")
        .map(|socket| CardLocation {
            section: "stash".into(),
            socket: socket.parse().ok(),
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
    pub right_click_behavior: Option<String>,
    pub socket: Option<u8>,
    pub size: String,
    pub tier: Option<String>,
    pub tier_provenance: Option<String>,
    pub enchantment: Option<String>,
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
pub struct DecisionSupport {
    pub source_priority: Vec<String>,
    pub ten_win_role: String,
    pub mode: String,
    pub recommendation: String,
    pub pivot_rule: String,
    pub can_force_ten_win_route: bool,
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
    if app_state == "ChoiceState" {
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
        "skill" => Some("selects_candidate"),
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
    pub available_actions: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub player_log: SourceProvenance,
    pub game_data: GameDataProvenance,
    pub card_identity_cache: Option<CardIdentityCacheProvenance>,
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
    pub progress: Option<ObservedProgress>,
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
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.offset = self.offset.saturating_add(bytes.len() as u64);
        let combined = format!("{}{}", self.remainder, String::from_utf8_lossy(&bytes));
        if combined.ends_with('\n') || combined.ends_with('\r') {
            model.ingest_text(&combined);
            self.remainder.clear();
        } else if let Some(split) = combined.rfind('\n') {
            model.ingest_text(&combined[..=split]);
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
        Ok(Self {
            config,
            config_path,
            config_bytes,
            index,
            card_identity_cache,
            ten_win_corpus,
            log,
            model: RunModel::default(),
            last_model_tick: u64::MAX,
            published_tick: 0,
        })
    }

    pub fn poll(&mut self) -> Result<Context, CompanionError> {
        let config_changed = self.refresh_config()?;
        self.log.poll(&mut self.model)?;
        if config_changed || self.last_model_tick != self.model.tick {
            self.published_tick = self.published_tick.saturating_add(1);
            self.last_model_tick = self.model.tick;
        }
        let mut context = self.model.context(
            &self.index,
            &self.config,
            self.log.offset(),
            self.ten_win_corpus.as_ref(),
            self.card_identity_cache.as_ref(),
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
            "[00:00:00.000] [BoardManager] Card Purchased: InstanceId: old - TemplateIdtpl-old - Target:PlayerSocket_0 - SectionPlayer\n",
            "[01:00:00.000] [AppState] State changed from [null] to [StartRunAppState]\n",
            "[01:00:01.000] [BoardManager] Card Purchased: InstanceId: current - TemplateIdtpl-current - Target:PlayerSocket_8 - SectionPlayer\n",
        ));

        assert!(!model.player_items.contains_key("old"));
        assert!(model.player_items.contains_key("current"));
        assert!(model.template_ids.contains_key("old"));
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
            day: Some(7),
            hour: Some(2),
            level: Some(7),
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
    fn nested_skill_choice_marks_right_click_as_irreversible_selection() {
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
        assert_eq!(
            context.selection_options[0].right_click_behavior.as_deref(),
            Some("selects_candidate")
        );
    }

    #[test]
    fn pvp_run_phase_uses_wins_and_prestige_instead_of_loss_streak() {
        let base = ObservedProgress {
            day: Some(4),
            hour: Some(3),
            level: Some(4),
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

        let early = build_decision_support(&[], &[], Some(&base), &policy);
        assert_eq!(early.mode, "build_scalable_core");

        let mut late = base.clone();
        late.wins = Some(8);
        let late = build_decision_support(&[], &[], Some(&late), &policy);
        assert_eq!(late.mode, "convert_current_core");

        let mut critical = base;
        critical.day = Some(7);
        critical.prestige = Some(10);
        let critical = build_decision_support(&[], &[], Some(&critical), &policy);
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
}
