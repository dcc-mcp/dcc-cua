//! Pure ordering core, adapted from the reviewed asynchronous lease reducer.
use super::*;

pub(super) fn effective_deadline(
    issued_ms: u64,
    lifetime_ms: u64,
    authorization_deadline_ms: u64,
) -> Result<u64, PreparationError> {
    if lifetime_ms == 0 || lifetime_ms > MAX_PREPARATION_LIFETIME_MS {
        return Err(PreparationError::new(PreparationFailure::InvalidBinding));
    }
    if authorization_deadline_ms <= issued_ms {
        return Err(PreparationError::new(PreparationFailure::Expired));
    }
    let requested = issued_ms
        .checked_add(lifetime_ms)
        .ok_or_else(|| PreparationError::new(PreparationFailure::InvalidBinding))?;
    Ok(requested.min(authorization_deadline_ms))
}

pub(super) struct PreparationState {
    pub status: CapturePreparationStatus,
    pub promotion_returned: bool,
}

impl PreparationState {
    pub fn new(id: [u8; 16], deadline_ms: u64, original: Vec<PreparedWindowState>) -> Self {
        Self {
            status: CapturePreparationStatus {
                preparation_id: id,
                phase: PreparationPhase::PendingPromotion,
                deadline_ms,
                pending_sequence: Some(1),
                capture_revoked: false,
                cleanup_verified: false,
                original,
                last_mutation: None,
                failure: None,
                journal_path: std::path::PathBuf::new(),
                affected_readback: vec![],
                last_completed_sequence: None,
            },
            promotion_returned: false,
        }
    }

    pub fn revoke(&mut self, reason: PreparationFailure) {
        self.status.capture_revoked = true;
        self.status
            .failure
            .get_or_insert_with(|| PreparationError::new(reason));
        if self.promotion_returned
            && self.status.pending_sequence.is_none()
            && !self.status.cleanup_verified
        {
            self.status.phase = PreparationPhase::RestorePending;
        }
    }

    pub fn expire(&mut self, now_ms: u64) {
        if now_ms >= self.status.deadline_ms {
            self.revoke(PreparationFailure::Expired);
        }
    }

    pub fn promotion_completed(
        &mut self,
        receipt: PreparationMutationReceipt,
        now_ms: u64,
    ) -> bool {
        if self.status.pending_sequence != Some(1)
            || receipt.sequence != 1
            || receipt.kind != PreparationMutationKind::Promote
        {
            self.unknown(PreparationFailure::ProtocolMismatch);
            return false;
        }
        self.promotion_returned = true;
        self.status.pending_sequence = None;
        self.expire(now_ms);
        if !receipt.api_success || receipt.failure.is_some() {
            self.revoke(PreparationFailure::MutationFailed);
        }
        self.status.phase = if self.status.capture_revoked {
            PreparationPhase::RestorePending
        } else {
            PreparationPhase::Active
        };
        self.status.last_mutation = Some(receipt);
        self.status.affected_readback =
            self.status.last_mutation.as_ref().unwrap().readback.clone();
        self.status.last_completed_sequence = Some(1);
        !self.status.capture_revoked
    }

    pub fn dispatch_restore(&mut self) -> bool {
        if !self.promotion_returned
            || self.status.pending_sequence.is_some()
            || self.status.cleanup_verified
            || !self.status.capture_revoked
        {
            return false;
        }
        let Some(sequence) = self
            .status
            .last_completed_sequence
            .unwrap_or(1)
            .checked_add(1)
        else {
            self.unknown(PreparationFailure::ProtocolMismatch);
            return false;
        };
        self.status.pending_sequence = Some(sequence);
        self.status.phase = PreparationPhase::RestorePending;
        true
    }

    pub fn restore_completed(&mut self, receipt: PreparationMutationReceipt, verified: bool) {
        if self.status.pending_sequence != Some(receipt.sequence)
            || receipt.sequence < 2
            || receipt.kind != PreparationMutationKind::Restore
        {
            self.unknown(PreparationFailure::ProtocolMismatch);
            return;
        }
        let verified = verified && receipt.api_success && receipt.failure.is_none();
        self.status.pending_sequence = None;
        self.status.last_mutation = Some(receipt);
        self.status.affected_readback =
            self.status.last_mutation.as_ref().unwrap().readback.clone();
        self.status.last_completed_sequence =
            self.status.last_mutation.as_ref().map(|item| item.sequence);
        if verified {
            self.status.cleanup_verified = true;
            self.status.phase = PreparationPhase::Restored;
        } else {
            self.unknown(PreparationFailure::ReadbackFailed);
        }
    }

    pub fn unknown(&mut self, reason: PreparationFailure) {
        self.status.capture_revoked = true;
        self.status.cleanup_verified = false;
        self.status.phase = PreparationPhase::CleanupUnknown;
        self.status.failure = Some(PreparationError::new(reason));
        // In-flight sequence is retained: a lost worker is not actual return.
    }

    pub fn await_settlement(&mut self) -> CapturePreparationStatus {
        let fixed = self.status.clone();
        self.status.cleanup_verified = false;
        if self.status.phase == PreparationPhase::Restored {
            self.status.phase = PreparationPhase::RestorePending;
        }
        fixed
    }

    pub fn persisted_settlement(
        &mut self,
        expected: &CapturePreparationStatus,
        actual: &CapturePreparationStatus,
    ) -> bool {
        if expected != actual
            || !actual.cleanup_verified
            || !actual.capture_revoked
            || !matches!(
                actual.phase,
                PreparationPhase::Restored | PreparationPhase::Refused
            )
            || actual.pending_sequence.is_some()
            || actual.preparation_id != self.status.preparation_id
            || actual.original != self.status.original
            || actual.last_mutation != self.status.last_mutation
            || actual.deadline_ms != self.status.deadline_ms
            || actual.journal_path != self.status.journal_path
            || actual.affected_readback != self.status.affected_readback
            || actual.last_completed_sequence != self.status.last_completed_sequence
        {
            self.unknown(PreparationFailure::JournalFailed);
            return false;
        }
        self.status.cleanup_verified = true;
        self.status.phase = actual.phase;
        true
    }
}
