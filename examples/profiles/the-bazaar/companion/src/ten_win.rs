use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use serde::Serialize;
use serde_json::Value;

const EXPECTED_SCHEMA_VERSION: u64 = 2;
const MAX_MATCHES: usize = 5;

#[derive(Clone, Debug)]
pub struct TenWinCorpus {
    generated_at: Option<String>,
    reference_by_template_id: BTreeMap<String, usize>,
    heroes: BTreeMap<String, HeroCorpus>,
}

#[derive(Clone, Debug)]
struct HeroCorpus {
    builds: Vec<Build>,
    card_index: BTreeMap<usize, Vec<usize>>,
}

#[derive(Clone, Debug)]
struct Build {
    build_id: usize,
    template_ids: HashSet<String>,
    layout: Vec<LayoutItem>,
    stats: BuildStats,
}

#[derive(Clone, Debug)]
struct LayoutItem {
    template_id: String,
    slot: Option<i64>,
    tier: Option<i64>,
    enchantment: Option<String>,
    size: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStats {
    pub completed_run_count: i64,
    pub ten_win_run_count: i64,
    pub ten_win_rate_bps: Option<i64>,
    pub p75_ten_win_final_day: Option<i64>,
    pub elite_completed_run_count: i64,
    pub elite_ten_win_run_count: i64,
    pub elite_ten_win_rate_bps: Option<i64>,
    pub score: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildEvidence {
    pub schema_version: u64,
    pub generated_at: Option<String>,
    pub hero: String,
    pub evidence_role: String,
    pub selection_mode: String,
    pub recall_mode: String,
    pub selected_template_ids: Vec<String>,
    pub selected_count: usize,
    pub max_matched_selected_count: usize,
    pub single_final_build_contains_all_selected: bool,
    pub matches: Vec<BuildMatchEvidence>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildMatchEvidence {
    pub build_id: usize,
    pub matched_selected_count: usize,
    pub matched_board_count: usize,
    pub matched_stash_count: usize,
    pub matched_shop_count: usize,
    pub live_state_score: usize,
    pub stats: BuildStats,
    pub final_layout: Vec<FinalLayoutCard>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalLayoutCard {
    pub template_id: String,
    pub display_name: Option<String>,
    pub slot: Option<i64>,
    pub tier: Option<String>,
    pub enchantment: Option<String>,
    pub size: Option<String>,
}

#[derive(Clone, Copy)]
struct BuildColumns {
    card_refs: usize,
    layout: usize,
    stats: usize,
}

#[derive(Clone, Copy)]
struct LayoutColumns {
    card_ref: usize,
    slot: usize,
    tier: usize,
    enchant_ref: usize,
    size: usize,
}

#[derive(Clone, Copy)]
struct StatsColumns {
    completed_run_count: usize,
    ten_win_run_count: usize,
    ten_win_rate_bps: usize,
    p75_ten_win_final_day: usize,
    elite_completed_run_count: usize,
    elite_ten_win_run_count: usize,
    elite_ten_win_rate_bps: usize,
    score: usize,
}

impl TenWinCorpus {
    pub fn parse_value(root: Value) -> Result<Self, String> {
        if root.get("schema_version").and_then(Value::as_u64) != Some(EXPECTED_SCHEMA_VERSION) {
            return Err("ten-win corpus requires schema_version 2".into());
        }
        let cards = parse_string_table(root.get("cards"), "cards")?;
        let enchantments = parse_string_table(root.get("enchantments"), "enchantments")?;
        let schemas = root
            .get("schemas")
            .and_then(Value::as_object)
            .ok_or("ten-win corpus is missing schemas")?;
        let build_schema = parse_schema(schemas.get("build"), "build")?;
        let layout_schema = parse_schema(schemas.get("layout"), "layout")?;
        let stats_schema = parse_schema(schemas.get("stats"), "stats")?;
        let build_columns = BuildColumns {
            card_refs: column(&build_schema, "card_refs")?,
            layout: column(&build_schema, "layout")?,
            stats: column(&build_schema, "stats")?,
        };
        let layout_columns = LayoutColumns {
            card_ref: column(&layout_schema, "card_ref")?,
            slot: column(&layout_schema, "slot")?,
            tier: column(&layout_schema, "tier")?,
            enchant_ref: column(&layout_schema, "enchant_ref")?,
            size: column(&layout_schema, "size")?,
        };
        let stats_columns = StatsColumns {
            completed_run_count: column(&stats_schema, "completed_run_count")?,
            ten_win_run_count: column(&stats_schema, "ten_win_run_count")?,
            ten_win_rate_bps: column(&stats_schema, "ten_win_rate_bps")?,
            p75_ten_win_final_day: column(&stats_schema, "p75_ten_win_final_day")?,
            elite_completed_run_count: column(&stats_schema, "elite_completed_run_count")?,
            elite_ten_win_run_count: column(&stats_schema, "elite_ten_win_run_count")?,
            elite_ten_win_rate_bps: column(&stats_schema, "elite_ten_win_rate_bps")?,
            score: column(&stats_schema, "score")?,
        };

        let reference_by_template_id = cards
            .iter()
            .enumerate()
            .filter_map(|(reference, template_id)| {
                template_id
                    .as_ref()
                    .map(|template_id| (template_id.clone(), reference))
            })
            .collect();
        let hero_values = root
            .get("heroes")
            .and_then(Value::as_object)
            .ok_or("ten-win corpus is missing heroes")?;
        let mut heroes = BTreeMap::new();
        for (hero, value) in hero_values {
            let builds = value
                .get("builds")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("ten-win hero {hero} is missing builds"))?
                .iter()
                .enumerate()
                .filter_map(|(build_id, row)| {
                    parse_build(
                        build_id,
                        row,
                        &cards,
                        &enchantments,
                        build_columns,
                        layout_columns,
                        stats_columns,
                    )
                })
                .collect();
            let card_index = parse_card_index(value.get("card_index"));
            heroes.insert(hero.clone(), HeroCorpus { builds, card_index });
        }

        Ok(Self {
            generated_at: root
                .get("generated_at")
                .and_then(Value::as_str)
                .map(str::to_owned),
            reference_by_template_id,
            heroes,
        })
    }

    pub fn evaluate<F>(
        &self,
        hero: &str,
        board: &[String],
        stash: &[String],
        shop: &[String],
        mut name_for: F,
    ) -> Option<BuildEvidence>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let hero_data = self.heroes.get(hero)?;
        let selected_template_ids = distinct_ids(board, stash, shop);
        if selected_template_ids.is_empty() {
            return None;
        }

        let mut covered_sets = Vec::new();
        let mut any_uncovered = false;
        for template_id in &selected_template_ids {
            let build_ids = self
                .reference_by_template_id
                .get(template_id)
                .and_then(|reference| hero_data.card_index.get(reference));
            if let Some(build_ids) = build_ids.filter(|ids| !ids.is_empty()) {
                covered_sets.push(build_ids.iter().copied().collect::<HashSet<_>>());
            } else {
                any_uncovered = true;
            }
        }
        if covered_sets.is_empty() {
            return None;
        }

        let mut candidate_ids = if any_uncovered {
            union_all(&covered_sets)
        } else {
            intersect_all(&covered_sets)
        };
        let recall_mode = if any_uncovered || candidate_ids.is_empty() {
            candidate_ids = union_all(&covered_sets);
            "union_fallback"
        } else {
            "intersection"
        };
        let selected = selected_template_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let board_set = board.iter().cloned().collect::<HashSet<_>>();
        let stash_set = stash.iter().cloned().collect::<HashSet<_>>();
        let shop_set = shop.iter().cloned().collect::<HashSet<_>>();
        let mut matches = candidate_ids
            .into_iter()
            .filter_map(|build_id| hero_data.builds.get(build_id))
            .map(|build| {
                let matched_selected_count = build.template_ids.intersection(&selected).count();
                let matched_board_count = build.template_ids.intersection(&board_set).count();
                let matched_stash_count = build.template_ids.intersection(&stash_set).count();
                let matched_shop_count = build.template_ids.intersection(&shop_set).count();
                let live_state_score = build
                    .template_ids
                    .iter()
                    .map(|template_id| {
                        if board_set.contains(template_id) {
                            3
                        } else if stash_set.contains(template_id) {
                            2
                        } else if shop_set.contains(template_id) {
                            1
                        } else {
                            0
                        }
                    })
                    .sum();
                BuildMatchEvidence {
                    build_id: build.build_id,
                    matched_selected_count,
                    matched_board_count,
                    matched_stash_count,
                    matched_shop_count,
                    live_state_score,
                    stats: build.stats.clone(),
                    final_layout: build
                        .layout
                        .iter()
                        .map(|item| FinalLayoutCard {
                            template_id: item.template_id.clone(),
                            display_name: name_for(&item.template_id),
                            slot: item.slot,
                            tier: tier_name(item.tier),
                            enchantment: item.enchantment.clone(),
                            size: size_name(item.size),
                        })
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        matches.sort_by(compare_matches);
        matches.truncate(MAX_MATCHES);
        let max_matched_selected_count = matches
            .first()
            .map(|item| item.matched_selected_count)
            .unwrap_or_default();
        Some(BuildEvidence {
            schema_version: EXPECTED_SCHEMA_VERSION,
            generated_at: self.generated_at.clone(),
            hero: hero.to_owned(),
            evidence_role: "historical_prior_not_action_authority".into(),
            selection_mode: "resolved_live_items".into(),
            recall_mode: recall_mode.into(),
            selected_count: selected_template_ids.len(),
            selected_template_ids,
            max_matched_selected_count,
            single_final_build_contains_all_selected: max_matched_selected_count == selected.len(),
            matches,
        })
    }
}

fn parse_build(
    build_id: usize,
    value: &Value,
    cards: &[Option<String>],
    enchantments: &[Option<String>],
    build_columns: BuildColumns,
    layout_columns: LayoutColumns,
    stats_columns: StatsColumns,
) -> Option<Build> {
    let row = value.as_array()?;
    let template_ids = row
        .get(build_columns.card_refs)?
        .as_array()?
        .iter()
        .filter_map(Value::as_u64)
        .filter_map(|reference| cards.get(reference as usize))
        .filter_map(Clone::clone)
        .collect();
    let layout = row
        .get(build_columns.layout)?
        .as_array()?
        .iter()
        .filter_map(|value| parse_layout(value, cards, enchantments, layout_columns))
        .collect();
    let stats = parse_stats(row.get(build_columns.stats)?, stats_columns)?;
    Some(Build {
        build_id,
        template_ids,
        layout,
        stats,
    })
}

fn parse_layout(
    value: &Value,
    cards: &[Option<String>],
    enchantments: &[Option<String>],
    columns: LayoutColumns,
) -> Option<LayoutItem> {
    let row = value.as_array()?;
    let card_ref = integer_at(row, columns.card_ref)? as usize;
    let template_id = cards.get(card_ref)?.clone()?;
    let enchantment = integer_at(row, columns.enchant_ref)
        .filter(|reference| *reference > 0)
        .and_then(|reference| enchantments.get(reference as usize))
        .cloned()
        .flatten();
    Some(LayoutItem {
        template_id,
        slot: integer_at(row, columns.slot),
        tier: integer_at(row, columns.tier),
        enchantment,
        size: integer_at(row, columns.size),
    })
}

fn parse_stats(value: &Value, columns: StatsColumns) -> Option<BuildStats> {
    let row = value.as_array()?;
    Some(BuildStats {
        completed_run_count: integer_at(row, columns.completed_run_count).unwrap_or_default(),
        ten_win_run_count: integer_at(row, columns.ten_win_run_count).unwrap_or_default(),
        ten_win_rate_bps: integer_at(row, columns.ten_win_rate_bps),
        p75_ten_win_final_day: integer_at(row, columns.p75_ten_win_final_day),
        elite_completed_run_count: integer_at(row, columns.elite_completed_run_count)
            .unwrap_or_default(),
        elite_ten_win_run_count: integer_at(row, columns.elite_ten_win_run_count)
            .unwrap_or_default(),
        elite_ten_win_rate_bps: integer_at(row, columns.elite_ten_win_rate_bps),
        score: integer_at(row, columns.score).unwrap_or_default(),
    })
}

fn parse_card_index(value: Option<&Value>) -> BTreeMap<usize, Vec<usize>> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let pair = value.as_array()?;
            let reference = pair.first()?.as_u64()? as usize;
            let build_ids = pair
                .get(1)?
                .as_array()?
                .iter()
                .filter_map(Value::as_u64)
                .map(|id| id as usize)
                .collect::<Vec<_>>();
            Some((reference, build_ids))
        })
        .collect()
}

fn parse_string_table(value: Option<&Value>, name: &str) -> Result<Vec<Option<String>>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("ten-win corpus is missing {name}"))
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
}

fn parse_schema(value: Option<&Value>, name: &str) -> Result<Vec<String>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("ten-win corpus is missing {name} schema"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("ten-win corpus {name} schema contains a non-string"))
        })
        .collect()
}

fn column(schema: &[String], name: &str) -> Result<usize, String> {
    schema
        .iter()
        .position(|column| column == name)
        .ok_or_else(|| format!("ten-win corpus schema is missing {name}"))
}

fn integer_at(row: &[Value], index: usize) -> Option<i64> {
    row.get(index).and_then(Value::as_i64)
}

fn distinct_ids(board: &[String], stash: &[String], shop: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    board
        .iter()
        .chain(stash)
        .chain(shop)
        .filter(|id| !id.is_empty() && seen.insert((*id).clone()))
        .cloned()
        .collect()
}

fn intersect_all(sets: &[HashSet<usize>]) -> HashSet<usize> {
    let mut result = sets.first().cloned().unwrap_or_default();
    for set in sets.iter().skip(1) {
        result.retain(|item| set.contains(item));
        if result.is_empty() {
            break;
        }
    }
    result
}

fn union_all(sets: &[HashSet<usize>]) -> HashSet<usize> {
    sets.iter().flat_map(|set| set.iter().copied()).collect()
}

fn compare_matches(left: &BuildMatchEvidence, right: &BuildMatchEvidence) -> Ordering {
    right
        .matched_selected_count
        .cmp(&left.matched_selected_count)
        .then_with(|| right.live_state_score.cmp(&left.live_state_score))
        .then_with(|| right.stats.score.cmp(&left.stats.score))
        .then_with(|| left.build_id.cmp(&right.build_id))
}

fn tier_name(value: Option<i64>) -> Option<String> {
    match value? {
        1 => Some("Bronze".into()),
        2 => Some("Silver".into()),
        3 => Some("Gold".into()),
        4 => Some("Diamond".into()),
        5 => Some("Legendary".into()),
        _ => None,
    }
}

fn size_name(value: Option<i64>) -> Option<String> {
    match value? {
        1 => Some("Small".into()),
        2 => Some("Medium".into()),
        3 => Some("Large".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::TenWinCorpus;

    #[test]
    fn union_fallback_preserves_layout_and_ranks_live_overlap() {
        let corpus = TenWinCorpus::parse_value(
            json!({
                "schema_version": 2,
                "generated_at": "2026-07-30T04:11:05Z",
                "cards": ["a", "b", "c"],
                "enchantments": [null, "Icy"],
                "schemas": {
                    "build": ["card_refs", "layout", "stats", "selection"],
                    "layout": ["card_ref", "slot", "tier", "enchant_ref", "size"],
                    "stats": ["completed_run_count", "ten_win_run_count", "ten_win_rate_bps", "avg_ten_win_final_day_tenth", "p75_ten_win_final_day", "avg_ten_win_final_losses_tenth", "elite_completed_run_count", "elite_ten_win_run_count", "elite_ten_win_rate_bps", "elite_avg_ten_win_final_day_tenth", "score"]
                },
                "heroes": {
                    "Vanessa": {
                        "builds": [
                            [[0, 1], [[0, 4, 3, 1, 2], [1, 6, 4, 0, 1]], [20, 12, 6000, 0, 9, 0, 3, 2, 6667, 0, 900], [0, null]],
                            [[0, 2], [[0, 1, 2, 0, 1], [2, 2, 3, 0, 2]], [10, 8, 8000, 0, 8, 0, 2, 2, 10000, 0, 800], [0, null]]
                        ],
                        "card_index": [[0, [0, 1]], [1, [0]], [2, [1]]]
                    }
                }
            }),
        )
        .expect("valid corpus");

        let evidence = corpus
            .evaluate("Vanessa", &["b".into()], &["c".into()], &[], |_| None)
            .expect("matched builds");

        assert_eq!(evidence.recall_mode, "union_fallback");
        assert_eq!(evidence.matches[0].build_id, 0);
        assert_eq!(evidence.matches[0].matched_selected_count, 1);
        assert_eq!(evidence.matches[0].final_layout[0].slot, Some(4));
        assert_eq!(
            evidence.matches[0].final_layout[0].tier.as_deref(),
            Some("Gold")
        );
        assert_eq!(
            evidence.matches[0].final_layout[0].size.as_deref(),
            Some("Medium")
        );
        assert_eq!(
            evidence.matches[0].final_layout[0].enchantment.as_deref(),
            Some("Icy")
        );
    }
}
