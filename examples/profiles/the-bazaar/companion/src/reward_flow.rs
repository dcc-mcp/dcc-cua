use serde::Serialize;

const MAX_REWARD_OUTCOMES: usize = 64;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardEvidence {
    pub selected_skill_seen: bool,
    pub select_skill_command_sent: bool,
    pub select_skill_command_response: bool,
    pub loot_transition_seen: bool,
    pub selected_instance_disposed: Option<bool>,
    pub exit_command_sent: bool,
    pub exit_command_response: bool,
    pub exact_card_purchased_destination_seen: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardOutcome {
    pub reward_id: u64,
    pub source_state: String,
    pub candidate_instance_ids: Vec<String>,
    pub selected_instance_id: Option<String>,
    pub status: String,
    pub finalized: bool,
    pub reason: String,
    pub evidence: RewardEvidence,
    pub item_destination: Option<RewardItemDestination>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardItemDestination {
    pub section: String,
    pub socket: u8,
}

#[derive(Clone, Debug)]
struct PendingRewardFlow {
    reward_id: u64,
    candidate_instance_ids: Vec<String>,
    selected_instance_id: Option<String>,
    evidence: RewardEvidence,
    item_destination: Option<RewardItemDestination>,
}

impl PendingRewardFlow {
    fn outcome(&self) -> RewardOutcome {
        RewardOutcome {
            reward_id: self.reward_id,
            source_state: "LootState".into(),
            candidate_instance_ids: self.candidate_instance_ids.clone(),
            selected_instance_id: self.selected_instance_id.clone(),
            status: "pending".into(),
            finalized: false,
            reason: "reward_commit_not_finalized".into(),
            evidence: self.evidence.clone(),
            item_destination: self.item_destination.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RewardFlowTracker {
    next_reward_id: u64,
    active: Option<PendingRewardFlow>,
    outcomes: Vec<RewardOutcome>,
}

impl RewardFlowTracker {
    pub(crate) fn reset(&mut self) {
        self.next_reward_id = 0;
        self.active = None;
        self.outcomes.clear();
    }

    pub(crate) fn observe_state_transition(&mut self, from: &str, to: &str) {
        if to == "LootState" && self.active.is_none() {
            self.next_reward_id = self.next_reward_id.saturating_add(1);
            self.active = Some(PendingRewardFlow {
                reward_id: self.next_reward_id,
                candidate_instance_ids: Vec::new(),
                selected_instance_id: None,
                evidence: RewardEvidence::default(),
                item_destination: None,
            });
        }
        let item_outcome = if from == "LootState"
            && to != "LootState"
            && let Some(active) = self.active.as_mut()
        {
            active.evidence.loot_transition_seen = true;
            active.item_destination.as_ref().map(|_| {
                let mut outcome = active.outcome();
                outcome.status = "claimed".into();
                outcome.finalized = true;
                outcome.reason = "exact_item_purchase_destination_observed".into();
                outcome
            })
        } else {
            None
        };
        if let Some(outcome) = item_outcome {
            self.finish(outcome);
        }
    }

    pub(crate) fn observe_candidates(&mut self, app_state: &str, candidates: Vec<String>) {
        if app_state != "LootState" || candidates.is_empty() {
            return;
        }
        if let Some(active) = self.active.as_mut()
            && active.candidate_instance_ids.is_empty()
        {
            active.candidate_instance_ids = candidates;
        }
    }

    pub(crate) fn observe_selected_skill(&mut self, instance_id: &str) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active
            .candidate_instance_ids
            .iter()
            .any(|candidate| candidate == instance_id)
        {
            active.selected_instance_id = Some(instance_id.into());
            active.evidence.selected_skill_seen = true;
        }
    }

    pub(crate) fn observe_item_purchase(&mut self, instance_id: &str, section: &str, socket: u8) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active
            .candidate_instance_ids
            .iter()
            .any(|candidate| candidate == instance_id)
        {
            active.selected_instance_id = Some(instance_id.into());
            active.item_destination = Some(RewardItemDestination {
                section: section.into(),
                socket,
            });
            active.evidence.exact_card_purchased_destination_seen = true;
        }
    }

    pub(crate) fn observe_select_skill_command_sent(&mut self) {
        if let Some(active) = self.active.as_mut()
            && active.evidence.selected_skill_seen
        {
            active.evidence.select_skill_command_sent = true;
        }
    }

    pub(crate) fn observe_select_skill_command_response(&mut self) {
        if let Some(active) = self.active.as_mut()
            && active.evidence.select_skill_command_sent
        {
            active.evidence.select_skill_command_response = true;
        }
    }

    pub(crate) fn observe_exit_command_sent(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.evidence.exit_command_sent = true;
        }
    }

    pub(crate) fn observe_exit_command_response(&mut self) {
        if let Some(active) = self.active.as_mut()
            && active.evidence.exit_command_sent
        {
            active.evidence.exit_command_response = true;
        }
    }

    pub(crate) fn observe_disposed(&mut self, disposed_instance_ids: &[String]) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.evidence.loot_transition_seen {
            return;
        }
        if active.evidence.exit_command_sent && active.evidence.exit_command_response {
            let all_candidates_disposed = !active.candidate_instance_ids.is_empty()
                && active.candidate_instance_ids.iter().all(|candidate| {
                    disposed_instance_ids
                        .iter()
                        .any(|disposed| disposed == candidate)
                });
            if all_candidates_disposed {
                let mut outcome = active.outcome();
                outcome.status = "discarded".into();
                outcome.finalized = true;
                outcome.reason = "exit_disposed_pending_reward".into();
                self.finish(outcome);
            }
            return;
        }
        let Some(selected_instance_id) = active.selected_instance_id.as_deref() else {
            return;
        };
        if !active.evidence.selected_skill_seen
            || !active.evidence.select_skill_command_sent
            || !active.evidence.select_skill_command_response
        {
            return;
        }
        let selected_was_disposed = disposed_instance_ids
            .iter()
            .any(|disposed| disposed == selected_instance_id);
        active.evidence.selected_instance_disposed = Some(selected_was_disposed);
        let mut outcome = active.outcome();
        if selected_was_disposed {
            outcome.status = "unresolved".into();
            outcome.reason = "selected_skill_was_disposed".into();
        } else {
            outcome.status = "claimed".into();
            outcome.reason = "exact_skill_commit_survived_post_loot_disposal".into();
        }
        outcome.finalized = true;
        self.finish(outcome);
    }

    pub(crate) fn outcomes(&self) -> Vec<RewardOutcome> {
        let mut outcomes = self.outcomes.clone();
        if let Some(active) = self.active.as_ref() {
            outcomes.push(active.outcome());
        }
        outcomes
    }

    fn finish(&mut self, outcome: RewardOutcome) {
        self.active = None;
        if self.outcomes.len() == MAX_REWARD_OUTCOMES {
            self.outcomes.remove(0);
        }
        self.outcomes.push(outcome);
    }
}
