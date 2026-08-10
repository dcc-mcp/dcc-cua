use serde::Serialize;

const MAX_DECISION_OUTCOMES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceFence {
    pub run_id: String,
    pub state_tick_id: u64,
    pub selection_message_id: String,
    pub choice_kind: String,
    pub candidate_instance_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionEvidence {
    pub exit_command_sent: bool,
    pub exit_command_response: bool,
    pub choice_transition_seen: bool,
    pub exact_candidates_disposed: bool,
    pub candidate_purchase_seen: bool,
    pub batch_conflicted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionReceipt {
    pub choice_fence: ChoiceFence,
    pub source_selection_message_id: String,
    pub commit_message_id: Option<String>,
    pub requested_action: String,
    pub same_fence_policy: String,
    pub status: String,
    pub finalized: bool,
    pub reason: String,
    pub evidence: DecisionEvidence,
}

#[derive(Clone, Debug)]
struct PendingDiscard {
    choice_fence: ChoiceFence,
    evidence: DecisionEvidence,
    batch: CommitBatch,
}

impl PendingDiscard {
    fn receipt(&self) -> DecisionReceipt {
        DecisionReceipt {
            choice_fence: self.choice_fence.clone(),
            source_selection_message_id: self.choice_fence.selection_message_id.clone(),
            commit_message_id: None,
            requested_action: "discard".into(),
            same_fence_policy: "deny_repeat_use_cached_receipt".into(),
            status: "pending".into(),
            finalized: false,
            reason: if self.evidence.batch_conflicted {
                "discard_commit_batch_conflict".into()
            } else {
                "discard_commit_not_finalized".into()
            },
            evidence: self.evidence.clone(),
        }
    }

    fn mark_batch_conflict(&mut self) {
        self.evidence.batch_conflicted = true;
        self.batch.evidence.batch_conflicted = true;
        self.batch.stage = CommitStage::Conflicted;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CommitStage {
    #[default]
    AwaitingExitResponse,
    AwaitingMessageStart,
    AwaitingTransition,
    AwaitingDeal,
    AwaitingDisposal,
    AwaitingFinish,
    Conflicted,
}

#[derive(Clone, Debug)]
struct CommitBatch {
    evidence: DecisionEvidence,
    next_choice_deal_count: u8,
    started_message_id: Option<String>,
    stage: CommitStage,
}

impl CommitBatch {
    fn new() -> Self {
        Self {
            evidence: DecisionEvidence {
                exit_command_sent: true,
                ..DecisionEvidence::default()
            },
            next_choice_deal_count: 0,
            started_message_id: None,
            stage: CommitStage::AwaitingExitResponse,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageFinishResult {
    NoActive,
    Pending,
    Finalized,
}

impl MessageFinishResult {
    pub(crate) fn changed(self) -> bool {
        self != Self::NoActive
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ChoiceFlowTracker {
    active: Option<PendingDiscard>,
    outcomes: Vec<DecisionReceipt>,
}

impl ChoiceFlowTracker {
    pub(crate) fn reset(&mut self) {
        self.active = None;
        self.outcomes.clear();
    }

    pub(crate) fn observe_discard_requested(&mut self, choice_fence: ChoiceFence) {
        if let Some(active) = self.active.as_mut() {
            active.mark_batch_conflict();
            if active.choice_fence == choice_fence {
                return;
            }
            let source_selection_message_id = choice_fence.selection_message_id.clone();
            self.push_outcome(DecisionReceipt {
                choice_fence,
                source_selection_message_id,
                commit_message_id: None,
                requested_action: "discard".into(),
                same_fence_policy: "deny_repeat_use_cached_receipt".into(),
                status: "denied".into(),
                finalized: true,
                reason: "active_discard_fence_conflict".into(),
                evidence: DecisionEvidence {
                    exit_command_sent: true,
                    batch_conflicted: true,
                    ..DecisionEvidence::default()
                },
            });
            return;
        }
        if self.has_receipt_for(&choice_fence) {
            return;
        }
        self.active = Some(PendingDiscard {
            choice_fence,
            evidence: DecisionEvidence {
                exit_command_sent: true,
                ..DecisionEvidence::default()
            },
            batch: CommitBatch::new(),
        });
    }

    pub(crate) fn observe_exit_command_response(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.batch.stage != CommitStage::AwaitingExitResponse {
            active.mark_batch_conflict();
            return;
        }
        active.evidence.exit_command_response = true;
        active.batch.evidence.exit_command_response = true;
        active.batch.stage = CommitStage::AwaitingMessageStart;
    }

    pub(crate) fn observe_message_started(&mut self, message_id: &str) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.batch.stage != CommitStage::AwaitingMessageStart
            || active.batch.started_message_id.is_some()
        {
            active.mark_batch_conflict();
            return;
        }
        active.batch.started_message_id = Some(message_id.to_owned());
        active.batch.stage = CommitStage::AwaitingTransition;
    }

    pub(crate) fn observe_state_transition(&mut self, from: &str, to: &str) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.batch.stage == CommitStage::AwaitingTransition
            && active.choice_fence.choice_kind == "event_option"
            && from == "EncounterState"
            && to == "ChoiceState"
        {
            active.evidence.choice_transition_seen = true;
            active.batch.evidence.choice_transition_seen = true;
            active.batch.stage = CommitStage::AwaitingDeal;
        } else {
            active.mark_batch_conflict();
        }
    }

    pub(crate) fn observe_candidates_dealt(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.batch.stage != CommitStage::AwaitingDeal {
            active.mark_batch_conflict();
            return;
        }
        active.batch.next_choice_deal_count = active.batch.next_choice_deal_count.saturating_add(1);
        active.batch.stage = CommitStage::AwaitingDisposal;
    }

    pub(crate) fn observe_candidate_purchase(&mut self, instance_id: &str) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active
            .choice_fence
            .candidate_instance_ids
            .iter()
            .any(|candidate| candidate == instance_id)
        {
            active.evidence.candidate_purchase_seen = true;
            active.batch.evidence.candidate_purchase_seen = true;
            active.mark_batch_conflict();
        }
    }

    pub(crate) fn observe_disposed(&mut self, disposed_instance_ids: &[String]) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.batch.stage != CommitStage::AwaitingDisposal {
            active.mark_batch_conflict();
            return;
        }
        let exact_candidates_disposed = !active.choice_fence.candidate_instance_ids.is_empty()
            && active.choice_fence.candidate_instance_ids == disposed_instance_ids;
        if !exact_candidates_disposed {
            active.mark_batch_conflict();
            return;
        }
        active.evidence.exact_candidates_disposed = true;
        active.batch.evidence.exact_candidates_disposed = true;
        active.batch.stage = CommitStage::AwaitingFinish;
    }

    pub(crate) fn observe_message_finished(&mut self, message_id: &str) -> MessageFinishResult {
        let Some(active) = self.active.as_mut() else {
            return MessageFinishResult::NoActive;
        };
        let exact_message_owner = active.batch.started_message_id.as_deref() == Some(message_id);
        if active.batch.stage != CommitStage::AwaitingFinish
            || !exact_message_owner
            || !active.batch.evidence.exit_command_sent
            || !active.batch.evidence.exit_command_response
            || !active.batch.evidence.choice_transition_seen
            || !active.batch.evidence.exact_candidates_disposed
            || active.batch.next_choice_deal_count != 1
            || active.batch.evidence.candidate_purchase_seen
            || active.evidence.candidate_purchase_seen
            || active.batch.evidence.batch_conflicted
            || active.evidence.batch_conflicted
        {
            active.mark_batch_conflict();
            return MessageFinishResult::Pending;
        }
        let mut receipt = active.receipt();
        receipt.evidence = active.batch.evidence.clone();
        receipt.commit_message_id = Some(message_id.to_owned());
        receipt.status = "discarded".into();
        receipt.finalized = true;
        receipt.reason = "exit_disposed_exact_choice_without_purchase".into();
        self.finish(receipt);
        MessageFinishResult::Finalized
    }

    pub(crate) fn outcomes(&self) -> Vec<DecisionReceipt> {
        let mut outcomes = self.outcomes.clone();
        if let Some(active) = self.active.as_ref() {
            outcomes.push(active.receipt());
        }
        outcomes
    }

    pub(crate) fn has_receipt_for(&self, choice_fence: &ChoiceFence) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| &active.choice_fence == choice_fence)
            || self
                .outcomes
                .iter()
                .any(|outcome| &outcome.choice_fence == choice_fence)
    }

    fn finish(&mut self, receipt: DecisionReceipt) {
        self.active = None;
        self.push_outcome(receipt);
    }

    fn push_outcome(&mut self, receipt: DecisionReceipt) {
        if self.outcomes.len() == MAX_DECISION_OUTCOMES {
            self.outcomes.remove(0);
        }
        self.outcomes.push(receipt);
    }
}
