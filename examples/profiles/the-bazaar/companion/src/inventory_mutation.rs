use serde::Serialize;

const MAX_RECEIPTS: usize = 64;
const CORRELATION_WINDOW_MS: u64 = 2_000;
const CORRELATION_WINDOW_BYTES: u64 = 65_536;
const MILLIS_PER_DAY: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationFence {
    pub run_id: String,
    pub state_tick_id: u64,
    pub log_cursor: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationLocation {
    pub section: Option<String>,
    pub socket: Option<u8>,
}

impl MutationLocation {
    pub(crate) fn known(section: &str, socket: Option<u8>) -> Self {
        Self {
            section: Some(section.to_owned()),
            socket,
        }
    }

    fn removed() -> Self {
        Self {
            section: None,
            socket: None,
        }
    }

    fn is_exact_physical_location(&self) -> bool {
        matches!(self.section.as_deref(), Some("board" | "stash")) && self.socket.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationLocationChange {
    pub instance_id: String,
    pub before: MutationLocation,
    pub after: MutationLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationCommandEvidence {
    pub local_mutation_seen: bool,
    pub command_sent: bool,
    pub command_response: bool,
    pub commit_seen: bool,
    pub ambiguous: bool,
    pub command_send_attempts: u8,
    pub retryable_recovery_count: u8,
    pub request_id: Option<String>,
    pub message_id: Option<String>,
    pub message_owner_completed: bool,
}

impl Default for MutationCommandEvidence {
    fn default() -> Self {
        Self {
            local_mutation_seen: true,
            command_sent: false,
            command_response: false,
            commit_seen: false,
            ambiguous: false,
            command_send_attempts: 0,
            retryable_recovery_count: 0,
            request_id: None,
            message_id: None,
            message_owner_completed: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SellEffectExpectation {
    pub status: String,
    pub descriptions: Vec<String>,
}

impl Default for SellEffectExpectation {
    fn default() -> Self {
        Self {
            status: "unknown_template_or_on_sell_effect".into(),
            descriptions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SellExpectation {
    pub value_gold: Option<u32>,
    pub effect: SellEffectExpectation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryMutationReceipt {
    pub run_id: String,
    pub state_tick_id: u64,
    pub log_cursor: u64,
    pub mutation_fence: MutationFence,
    pub operation: String,
    pub exact_instance_ids: Vec<String>,
    pub locations: Vec<MutationLocationChange>,
    pub evidence: MutationCommandEvidence,
    pub sell_expectation: Option<SellExpectation>,
    pub finalized: bool,
    pub log_committed: bool,
    pub status: String,
    pub reason: String,
    pub requires_verified_observation: bool,
    pub same_fence_policy: String,
    pub correlation_window_ms: u64,
    pub correlation_window_bytes: u64,
    pub verified_observation_id: Option<String>,
    pub verified_observation_provenance: Option<String>,
    #[serde(skip)]
    pub(crate) sell_template_id: Option<String>,
}

impl InventoryMutationReceipt {
    fn pending(input: NewMutation<'_>) -> Self {
        let NewMutation {
            run_id,
            state_tick_id,
            log_cursor,
            operation,
            instance_id,
            before,
            after,
            sell_template_id,
        } = input;
        let before = before.unwrap_or(MutationLocation {
            section: None,
            socket: None,
        });
        let missing_before = !before.is_exact_physical_location();
        Self {
            run_id: run_id.to_owned(),
            state_tick_id,
            log_cursor,
            mutation_fence: MutationFence {
                run_id: run_id.to_owned(),
                state_tick_id: state_tick_id.saturating_sub(1),
                log_cursor,
            },
            operation: operation.as_str().into(),
            exact_instance_ids: vec![instance_id.to_owned()],
            locations: vec![MutationLocationChange {
                instance_id: instance_id.to_owned(),
                before,
                after,
            }],
            evidence: MutationCommandEvidence::default(),
            sell_expectation: (operation == Operation::Sell).then(|| SellExpectation {
                value_gold: None,
                effect: SellEffectExpectation::default(),
            }),
            finalized: false,
            log_committed: false,
            status: "pending".into(),
            reason: if missing_before {
                "missing_authoritative_before_location"
            } else {
                "awaiting_command_send_response_and_commit"
            }
            .into(),
            requires_verified_observation: true,
            same_fence_policy: "deny_repeat_use_cached_receipt".into(),
            correlation_window_ms: CORRELATION_WINDOW_MS,
            correlation_window_bytes: CORRELATION_WINDOW_BYTES,
            verified_observation_id: None,
            verified_observation_provenance: None,
            sell_template_id,
        }
    }

    pub(crate) fn set_sell_effect(&mut self, status: &str, descriptions: Vec<String>) {
        if let Some(expectation) = self.sell_expectation.as_mut() {
            expectation.effect = SellEffectExpectation {
                status: status.into(),
                descriptions,
            };
        }
    }

    fn has_exact_locations(&self) -> bool {
        self.locations.iter().all(|change| {
            change.before.is_exact_physical_location()
                && (self.operation == Operation::Sell.as_str()
                    || change.after.is_exact_physical_location())
        })
    }

    fn is_exact_bijective_pair_swap(&self) -> bool {
        if self.operation != Operation::SwapReorder.as_str()
            || self.exact_instance_ids.len() != 2
            || self.locations.len() != 2
            || self.exact_instance_ids[0] == self.exact_instance_ids[1]
        {
            return false;
        }
        let [left, right] = self.locations.as_slice() else {
            return false;
        };
        left.instance_id != right.instance_id
            && left.before == right.after
            && right.before == left.after
            && left.before != left.after
            && left.before.is_exact_physical_location()
            && left.after.is_exact_physical_location()
    }
}

struct NewMutation<'a> {
    run_id: &'a str,
    state_tick_id: u64,
    log_cursor: u64,
    operation: Operation,
    instance_id: &'a str,
    before: Option<MutationLocation>,
    after: MutationLocation,
    sell_template_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Sell,
    SwapReorder,
}

impl Operation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sell => "sell",
            Self::SwapReorder => "swap_reorder",
        }
    }
}

#[derive(Clone, Debug)]
struct PendingMutation {
    operation: Operation,
    start_timestamp_ms: u64,
    last_timestamp_ms: u64,
    command_send_timestamp_ms: Option<u64>,
    receipt: InventoryMutationReceipt,
}

impl PendingMutation {
    fn evidence_is_within_window(&self, timestamp_ms: u64, log_cursor: u64) -> bool {
        within_time_window(self.start_timestamp_ms, timestamp_ms, CORRELATION_WINDOW_MS)
            && log_cursor.saturating_sub(self.receipt.mutation_fence.log_cursor)
                <= CORRELATION_WINDOW_BYTES
    }

    fn observe_cursor(&mut self, state_tick_id: u64, timestamp_ms: u64, log_cursor: u64) {
        self.last_timestamp_ms = timestamp_ms;
        self.receipt.state_tick_id = state_tick_id;
        self.receipt.log_cursor = log_cursor;
    }

    fn mark_ambiguous(&mut self, state_tick_id: u64, timestamp_ms: u64, log_cursor: u64) {
        self.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        self.receipt.evidence.ambiguous = true;
        self.receipt.finalized = false;
        self.receipt.status = "pending".into();
        self.receipt.reason = "ambiguous_command_evidence".into();
    }

    fn missing_exact_location(&self) -> bool {
        !self.receipt.has_exact_locations()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InventoryMutationTracker {
    receipts: Vec<InventoryMutationReceipt>,
    pending: Option<PendingMutation>,
}

impl InventoryMutationTracker {
    pub(crate) fn reset(&mut self) {
        self.receipts.clear();
        self.pending = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn observe_reorder(
        &mut self,
        run_id: Option<&str>,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
        instance_id: &str,
        before: Option<MutationLocation>,
        after_socket: u8,
    ) -> bool {
        let (Some(run_id), Some(timestamp_ms)) = (run_id, timestamp_ms) else {
            return false;
        };
        let after = MutationLocation::known("board", Some(after_socket));
        if self.pending.as_ref().is_some_and(|pending| {
            pending.operation == Operation::SwapReorder
                && pending.receipt.evidence.command_send_attempts == 0
                && pending.evidence_is_within_window(timestamp_ms, log_cursor)
        }) {
            let pending = self.pending.as_mut().expect("checked pending mutation");
            pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
            if pending
                .receipt
                .exact_instance_ids
                .iter()
                .any(|known| known == instance_id)
            {
                pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
                return true;
            }
            let before = before.unwrap_or(MutationLocation {
                section: None,
                socket: None,
            });
            if !before.is_exact_physical_location() {
                pending.receipt.reason = "missing_authoritative_before_location".into();
            }
            pending.receipt.exact_instance_ids.push(instance_id.into());
            pending.receipt.locations.push(MutationLocationChange {
                instance_id: instance_id.into(),
                before,
                after,
            });
            return true;
        }
        self.archive_interrupted_pending(state_tick_id, timestamp_ms, log_cursor);
        self.reserve_for_pending();
        self.pending = Some(PendingMutation {
            operation: Operation::SwapReorder,
            start_timestamp_ms: timestamp_ms,
            last_timestamp_ms: timestamp_ms,
            command_send_timestamp_ms: None,
            receipt: InventoryMutationReceipt::pending(NewMutation {
                run_id,
                state_tick_id,
                log_cursor,
                operation: Operation::SwapReorder,
                instance_id,
                before,
                after,
                sell_template_id: None,
            }),
        });
        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn observe_sell_removed(
        &mut self,
        run_id: Option<&str>,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
        instance_id: &str,
        before: Option<MutationLocation>,
        template_id: Option<String>,
    ) -> bool {
        let (Some(run_id), Some(timestamp_ms)) = (run_id, timestamp_ms) else {
            return false;
        };
        self.archive_interrupted_pending(state_tick_id, timestamp_ms, log_cursor);
        self.reserve_for_pending();
        self.pending = Some(PendingMutation {
            operation: Operation::Sell,
            start_timestamp_ms: timestamp_ms,
            last_timestamp_ms: timestamp_ms,
            command_send_timestamp_ms: None,
            receipt: InventoryMutationReceipt::pending(NewMutation {
                run_id,
                state_tick_id,
                log_cursor,
                operation: Operation::Sell,
                instance_id,
                before,
                after: MutationLocation::removed(),
                sell_template_id: template_id,
            }),
        });
        true
    }

    pub(crate) fn observe_command_sent(
        &mut self,
        operation: &str,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
    ) -> bool {
        let Some(expected) = operation_from_command(operation) else {
            return false;
        };
        let Some(timestamp_ms) = timestamp_ms else {
            return self.mark_pending_ambiguous_without_timestamp(state_tick_id, log_cursor);
        };
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        if pending.operation != expected
            || !pending.evidence_is_within_window(timestamp_ms, log_cursor)
        {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            pending.receipt.reason = "mutation_evidence_outside_correlation_window".into();
            return true;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        pending.receipt.evidence.command_send_attempts = pending
            .receipt
            .evidence
            .command_send_attempts
            .saturating_add(1);
        if pending.receipt.evidence.command_sent {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
        } else {
            pending.receipt.evidence.command_sent = true;
            pending.command_send_timestamp_ms = Some(timestamp_ms);
            if !pending.missing_exact_location() {
                pending.receipt.reason = "awaiting_command_response_and_commit".into();
            }
        }
        true
    }

    pub(crate) fn observe_retryable_recovery(
        &mut self,
        operation: &str,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
    ) -> bool {
        let Some(expected) = operation_from_command(operation) else {
            return false;
        };
        let (Some(timestamp_ms), Some(pending)) = (timestamp_ms, self.pending.as_mut()) else {
            return false;
        };
        if pending.operation != expected || !pending.receipt.evidence.command_sent {
            return false;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        pending.receipt.evidence.retryable_recovery_count = pending
            .receipt
            .evidence
            .retryable_recovery_count
            .saturating_add(1);
        pending.receipt.evidence.command_sent = false;
        pending.command_send_timestamp_ms = None;
        pending.receipt.reason = "awaiting_retry_after_explicit_session_recovery".into();
        true
    }

    pub(crate) fn observe_command_response(
        &mut self,
        operation: &str,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
    ) -> bool {
        let Some(expected) = operation_from_command(operation) else {
            return false;
        };
        let Some(timestamp_ms) = timestamp_ms else {
            return self.mark_pending_ambiguous_without_timestamp(state_tick_id, log_cursor);
        };
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        if pending.receipt.evidence.request_id.is_none() {
            pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
            pending.receipt.evidence.command_response = true;
            if !pending.receipt.evidence.ambiguous
                && pending.receipt.reason != "missing_authoritative_before_location"
            {
                pending.receipt.evidence.ambiguous = true;
                pending.receipt.reason = "command_response_without_captured_request_id".into();
            }
            return true;
        }
        let response_is_bound = pending.operation == expected
            && pending.receipt.evidence.command_sent
            && pending
                .command_send_timestamp_ms
                .is_some_and(|sent| within_time_window(sent, timestamp_ms, CORRELATION_WINDOW_MS))
            && pending.evidence_is_within_window(timestamp_ms, log_cursor);
        if !response_is_bound || pending.receipt.evidence.command_response {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            return true;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        pending.receipt.evidence.command_response = true;
        if !pending.missing_exact_location() && !pending.receipt.evidence.ambiguous {
            pending.receipt.reason = if pending.operation == Operation::Sell {
                "awaiting_exact_sell_value_and_message_owner"
            } else {
                "awaiting_exact_message_owner_finish"
            }
            .into();
        }
        true
    }

    pub(crate) fn observe_request_id(
        &mut self,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
        request_id: &str,
    ) -> bool {
        let (Some(timestamp_ms), Some(pending)) = (timestamp_ms, self.pending.as_mut()) else {
            return false;
        };
        if !pending.evidence_is_within_window(timestamp_ms, log_cursor) {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            pending.receipt.reason = "request_id_outside_command_correlation_window".into();
            return true;
        }
        if !pending.receipt.evidence.command_sent
            || pending.receipt.evidence.command_response
            || request_id.is_empty()
        {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            return true;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        if pending.receipt.evidence.request_id.is_some() {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
        } else {
            pending.receipt.evidence.request_id = Some(request_id.into());
        }
        true
    }

    pub(crate) fn observe_message_started(
        &mut self,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
        message_id: &str,
    ) -> bool {
        let Some(timestamp_ms) = timestamp_ms else {
            return false;
        };
        if self.pending.is_none() {
            return self.invalidate_archived_message_owner(state_tick_id, log_cursor, message_id);
        }
        let pending = self.pending.as_mut().expect("checked pending mutation");
        if !pending.receipt.evidence.command_response
            || !pending.evidence_is_within_window(timestamp_ms, log_cursor)
            || message_id.is_empty()
        {
            return false;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        if pending.receipt.evidence.message_id.is_some() {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            pending.receipt.reason = "duplicate_message_owner_processing".into();
        } else {
            pending.receipt.evidence.message_id = Some(message_id.into());
        }
        true
    }

    pub(crate) fn observe_message_finished(
        &mut self,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
        message_id: &str,
    ) -> bool {
        let Some(timestamp_ms) = timestamp_ms else {
            return false;
        };
        if self.pending.is_none() {
            return self.invalidate_archived_message_owner(state_tick_id, log_cursor, message_id);
        }
        let pending = self.pending.as_mut().expect("checked pending mutation");
        if !pending.receipt.evidence.command_response {
            return false;
        }
        if pending.receipt.evidence.message_id.as_deref() != Some(message_id) {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            pending.receipt.reason = "message_owner_mismatch".into();
            let receipt = self.pending.take().expect("pending mutation").receipt;
            self.push_receipt(receipt);
            return true;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        pending.receipt.evidence.message_owner_completed = true;
        if pending.operation == Operation::SwapReorder {
            pending.receipt.evidence.commit_seen = true;
        }
        self.archive_log_commit();
        true
    }

    pub(crate) fn observe_sell_commit(
        &mut self,
        state_tick_id: u64,
        timestamp_ms: Option<u64>,
        log_cursor: u64,
        instance_id: &str,
        value_gold: u32,
    ) -> bool {
        let Some(timestamp_ms) = timestamp_ms else {
            return self.mark_pending_ambiguous_without_timestamp(state_tick_id, log_cursor);
        };
        let Some(pending) = self.pending.as_mut() else {
            return self.invalidate_late_sell_commit(state_tick_id, log_cursor);
        };
        let exact_instance = pending.operation == Operation::Sell
            && pending.receipt.exact_instance_ids.as_slice() == [instance_id];
        if !exact_instance
            || !pending.receipt.evidence.command_response
            || !pending.evidence_is_within_window(timestamp_ms, log_cursor)
        {
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            return true;
        }
        if pending.receipt.evidence.message_id.is_none()
            || pending.receipt.evidence.message_owner_completed
        {
            let already_ambiguous = pending.receipt.evidence.ambiguous;
            pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
            if !already_ambiguous {
                pending.receipt.reason = "sell_commit_outside_message_owner".into();
            }
            return true;
        }
        pending.observe_cursor(state_tick_id, timestamp_ms, log_cursor);
        pending.receipt.evidence.commit_seen = true;
        if let Some(expectation) = pending.receipt.sell_expectation.as_mut() {
            expectation.value_gold = Some(value_gold);
        }
        if !pending.receipt.evidence.ambiguous {
            pending.receipt.reason = "awaiting_exact_message_owner_finish".into();
        }
        true
    }

    pub(crate) fn receipts(&self) -> Vec<InventoryMutationReceipt> {
        let mut receipts = self.receipts.clone();
        if let Some(pending) = self.pending.as_ref() {
            if receipts.len() == MAX_RECEIPTS {
                receipts.remove(0);
            }
            receipts.push(pending.receipt.clone());
        }
        receipts
    }

    fn archive_log_commit(&mut self) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        let base_complete = !pending.receipt.evidence.ambiguous
            && pending.receipt.evidence.command_sent
            && pending.receipt.evidence.command_response
            && pending.receipt.evidence.commit_seen
            && pending.receipt.evidence.request_id.is_some()
            && pending.receipt.evidence.message_id.is_some()
            && pending.receipt.evidence.message_owner_completed
            && pending.receipt.has_exact_locations();
        let shape_complete = pending.operation != Operation::SwapReorder
            || pending.receipt.is_exact_bijective_pair_swap();
        if base_complete && shape_complete {
            pending.receipt.log_committed = true;
            pending.receipt.finalized = false;
            pending.receipt.status = "awaiting_verified_observation".into();
            pending.receipt.reason = "log_commit_requires_verified_observation".into();
        } else if pending.operation == Operation::SwapReorder && !shape_complete {
            pending.receipt.finalized = false;
            pending.receipt.status = "pending".into();
            pending.receipt.reason = "not_exact_two_instance_bijective_swap".into();
        }
        self.push_receipt(pending.receipt);
    }

    fn archive_interrupted_pending(
        &mut self,
        state_tick_id: u64,
        timestamp_ms: u64,
        log_cursor: u64,
    ) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        pending.mark_ambiguous(state_tick_id, timestamp_ms, log_cursor);
        pending.receipt.reason = "interrupted_by_new_mutation_evidence".into();
        self.push_receipt(pending.receipt);
    }

    fn mark_pending_ambiguous_without_timestamp(
        &mut self,
        state_tick_id: u64,
        log_cursor: u64,
    ) -> bool {
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        pending.receipt.state_tick_id = state_tick_id;
        pending.receipt.log_cursor = log_cursor;
        pending.receipt.evidence.ambiguous = true;
        pending.receipt.reason = "missing_authoritative_log_timestamp".into();
        true
    }

    fn reserve_for_pending(&mut self) {
        if self.receipts.len() == MAX_RECEIPTS {
            self.receipts.remove(0);
        }
    }

    fn push_receipt(&mut self, receipt: InventoryMutationReceipt) {
        if self.receipts.len() == MAX_RECEIPTS {
            self.receipts.remove(0);
        }
        self.receipts.push(receipt);
    }

    fn invalidate_archived_message_owner(
        &mut self,
        state_tick_id: u64,
        log_cursor: u64,
        message_id: &str,
    ) -> bool {
        let Some(receipt) = self.receipts.iter_mut().rev().find(|receipt| {
            receipt.evidence.message_id.as_deref() == Some(message_id)
                && log_cursor.saturating_sub(receipt.log_cursor) <= CORRELATION_WINDOW_BYTES
        }) else {
            return false;
        };
        receipt.state_tick_id = state_tick_id;
        receipt.log_cursor = log_cursor;
        receipt.evidence.ambiguous = true;
        receipt.log_committed = false;
        receipt.finalized = false;
        receipt.status = "pending".into();
        receipt.reason = "message_owner_reused_after_archive".into();
        receipt.verified_observation_id = None;
        receipt.verified_observation_provenance = None;
        true
    }

    fn invalidate_late_sell_commit(&mut self, state_tick_id: u64, log_cursor: u64) -> bool {
        let Some(receipt) = self.receipts.iter_mut().rev().find(|receipt| {
            receipt.operation == Operation::Sell.as_str()
                && receipt.evidence.message_owner_completed
                && log_cursor.saturating_sub(receipt.log_cursor) <= CORRELATION_WINDOW_BYTES
        }) else {
            return false;
        };
        receipt.state_tick_id = state_tick_id;
        receipt.log_cursor = log_cursor;
        receipt.evidence.ambiguous = true;
        receipt.log_committed = false;
        receipt.finalized = false;
        receipt.status = "pending".into();
        receipt.reason = "sell_commit_outside_message_owner".into();
        receipt.verified_observation_id = None;
        receipt.verified_observation_provenance = None;
        true
    }
}

fn operation_from_command(command: &str) -> Option<Operation> {
    match command {
        "SellCardCommand" => Some(Operation::Sell),
        "MoveItemCommand" => Some(Operation::SwapReorder),
        _ => None,
    }
}

fn within_time_window(start_ms: u64, end_ms: u64, window_ms: u64) -> bool {
    let elapsed = if end_ms >= start_ms {
        end_ms - start_ms
    } else {
        end_ms + MILLIS_PER_DAY - start_ms
    };
    elapsed <= window_ms
}
