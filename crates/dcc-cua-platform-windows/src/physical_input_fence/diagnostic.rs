use super::*;
use dcc_cua_protocol::native_input_diagnostic::{
    MAX_NATIVE_INPUT_DIAGNOSTIC_REGIONS, NativeInputFenceDiagnostic,
};

#[derive(Clone)]
pub(super) struct RejectedFenceDiagnostic {
    pub sample: NativeInputFenceDiagnostic,
    pub visibility_failure: Option<crate::VisibleWindowCaptureDiagnostic>,
}

impl RejectedFenceDiagnostic {
    pub(super) fn new(
        fence: WindowsPhysicalInputFence,
        point: Option<(i32, i32)>,
        evidence: ExactWindowPixelEvidence,
    ) -> Self {
        Self {
            sample: NativeInputFenceDiagnostic {
                schema_version: 1,
                authorizes_input: false,
                target_process_id: fence.target.process_id,
                target_window_handle: fence.target.window_handle,
                point: point.map(|(x, y)| [x, y]),
                unobscured: evidence.unobscured,
                point_proof_complete: false,
                point_visible: None,
                hit_window_handle: None,
                hit_process_id: None,
                hit_root_window_handle: None,
                hit_is_target_or_child: None,
                redacted_rectangles: Vec::new(),
                redaction_count: 0,
                redactions_complete: false,
            },
            visibility_failure: evidence.visibility_failure,
        }
    }

    pub(super) fn attach_point(
        &mut self,
        mut point: crate::visible_capture::ExactWindowPointEvidence,
    ) {
        self.sample.point_proof_complete = true;
        self.sample.point_visible = Some(point.visible);
        self.sample.hit_window_handle = point.hit_window_handle;
        self.sample.hit_process_id = point.hit_process_id;
        self.sample.hit_root_window_handle = point.hit_root_window_handle;
        self.sample.hit_is_target_or_child = Some(point.hit_is_target_or_child);
        self.sample.redaction_count = point.redacted_regions.len() as u32;
        self.sample.redactions_complete =
            point.redacted_regions.len() <= MAX_NATIVE_INPUT_DIAGNOSTIC_REGIONS;
        point
            .redacted_regions
            .truncate(MAX_NATIVE_INPUT_DIAGNOSTIC_REGIONS);
        self.sample.redacted_rectangles = point.redacted_regions;
    }
}

#[cfg(test)]
mod tests;
