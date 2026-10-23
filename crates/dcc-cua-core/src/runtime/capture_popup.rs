//! Explicit secondary observations for keyboard input that stays on the root.
use super::*;

#[cfg(windows)]
pub(super) struct CapturePopupMetadata {
    pub(super) observation_id: String,
    pub(super) session_id: String,
    pub(super) read_at: Instant,
    pub(super) authorization_deadline: Instant,
    pub(super) proof: dcc_cua_platform_windows::NativeCapturePopupProof,
}

#[cfg(any(windows, test))]
fn stale(message: &str) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::StaleObservation, message)
}

pub(super) fn validate_selection(selected: &[u64]) -> ComputerUseResult<()> {
    let unique = selected
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if selected.is_empty()
        || selected.len() > 8
        || unique.len() != selected.len()
        || unique.contains(&0)
    {
        return Err(ComputerUseError::new(
            ComputerUseErrorCode::InvalidAction,
            "native capture popup selection requires one through eight unique positive HWNDs",
        ));
    }
    Ok(())
}

/// Rectangle subtraction keeps every occluded pixel excluded. An explicit
/// collection bound fails closed rather than approximating complete coverage.
#[cfg(any(windows, test))]
fn subtract(rect: [i32; 4], cover: [i32; 4]) -> ComputerUseResult<Vec<[i32; 4]>> {
    let edges = |r: [i32; 4]| -> ComputerUseResult<[i32; 4]> {
        if r[2] <= 0 || r[3] <= 0 {
            return Err(stale("invalid composite pixel rectangle"));
        }
        Ok([
            r[0],
            r[1],
            r[0].checked_add(r[2])
                .ok_or_else(|| stale("pixel extent overflow"))?,
            r[1].checked_add(r[3])
                .ok_or_else(|| stale("pixel extent overflow"))?,
        ])
    };
    let a = edges(rect)?;
    let b = edges(cover)?;
    let i = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    if i[0] >= i[2] || i[1] >= i[3] {
        return Ok(vec![rect]);
    }
    Ok([
        [a[0], a[1], a[2] - a[0], i[1] - a[1]],
        [a[0], i[3], a[2] - a[0], a[3] - i[3]],
        [a[0], i[1], i[0] - a[0], i[3] - i[1]],
        [i[2], i[1], a[2] - i[2], i[3] - i[1]],
    ]
    .into_iter()
    .filter(|r| r[2] > 0 && r[3] > 0)
    .collect())
}

#[cfg(any(windows, test))]
fn subtract_all(
    mut regions: Vec<[i32; 4]>,
    covers: &[[i32; 4]],
) -> ComputerUseResult<Vec<[i32; 4]>> {
    if regions.iter().chain(covers).any(|r| {
        r[2] <= 0
            || r[3] <= 0
            || r[0].checked_add(r[2]).is_none()
            || r[1].checked_add(r[3]).is_none()
    }) {
        return Err(stale("invalid composite pixel rectangle"));
    }
    for cover in covers {
        let mut remaining = Vec::new();
        for region in regions {
            remaining.extend(subtract(region, *cover)?);
        }
        if remaining.len() > 4096 {
            return Err(stale("composite rectangle proof exceeded its bound"));
        }
        regions = remaining;
    }
    Ok(regions)
}

#[cfg(any(windows, test))]
fn require_complete_coverage(
    root_redactions: &[[i32; 4]],
    frames: &[([i32; 4], Vec<[i32; 4]>)],
) -> ComputerUseResult<()> {
    let mut visible = Vec::new();
    for (source, redactions) in frames {
        visible.extend(subtract_all(vec![*source], redactions)?);
        if visible.len() > 4096 {
            return Err(stale("composite visible pixel proof exceeded its bound"));
        }
    }
    if !subtract_all(root_redactions.to_vec(), &visible)?.is_empty() {
        return Err(stale(
            "selected popup observations do not cover every redacted root pixel",
        ));
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn validate_root_capture_coverage(
    provenance: &Value,
    native_redactions: &[[i32; 4]],
    frames: &[([i32; 4], Vec<[i32; 4]>)],
) -> ComputerUseResult<()> {
    let desktop_crop = match provenance["capture_mode"].as_str() {
        Some("window_content") => false,
        Some("visible_desktop_crop" | "redacted_desktop_crop") => true,
        _ => {
            return Err(stale(
                "root capture mode metadata is unavailable or unsupported",
            ));
        }
    };
    let root_redactions: Vec<[i32; 4]> =
        serde_json::from_value(provenance["visibility"]["redacted_rectangles"].clone())
            .map_err(|_| stale("root redaction metadata is unavailable"))?;
    if desktop_crop {
        let mut expected = native_redactions.to_vec();
        let mut observed = root_redactions.clone();
        expected.sort_unstable();
        observed.sort_unstable();
        if expected != observed {
            return Err(stale(
                "root pixel redactions differ from the native popup proof",
            ));
        }
    } else if !root_redactions.is_empty() {
        return Err(stale(
            "window-content root pixels cannot claim desktop redactions",
        ));
    }
    require_complete_coverage(&root_redactions, frames)
}

#[cfg(windows)]
pub(super) fn keyboard_context(
    metadata: Option<CapturePopupMetadata>,
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
    scope: &ComputerUseTargetScope,
    session_id: &str,
) -> ComputerUseResult<Option<CapturePopupMetadata>> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    if !matches!(
        action.action.as_str(),
        "keypress" | "keyboard_shortcut" | "type" | "type_chars"
    ) {
        return Ok(None);
    }
    validate_keyboard_binding(
        &metadata.observation_id,
        &metadata.session_id,
        (
            metadata.proof.root.process_id,
            metadata.proof.root.window_handle,
        ),
        action,
        observation,
        scope,
        (session_id, metadata.read_at.elapsed()),
    )?;
    Ok(Some(metadata))
}

#[cfg(any(windows, test))]
fn validate_keyboard_binding(
    id: &str,
    session: &str,
    root: (u32, u64),
    action: &ComputerUseAction,
    observation: &ComputerUseObservation,
    scope: &ComputerUseTargetScope,
    current: (&str, Duration),
) -> ComputerUseResult<()> {
    if id != observation.observation_id
        || session != current.0
        || action.observation_id.as_deref() != Some(id)
        || scope.process_id != Some(root.0)
        || scope.window_handle != Some(root.1)
        || current.1 > Duration::from_secs(5)
    {
        return Err(stale(
            "native capture popup keyboard requires the latest one-use composite observation, at most five seconds old",
        ));
    }
    Ok(())
}

impl ComputerUseSession {
    /// Host must independently require its constructor-authorized secondary
    /// observation ceiling. Secondary frames never change the session target.
    pub async fn screenshot_native_capture_popups(
        &mut self,
        selected: &[u64],
        diagnostics: bool,
        authorization_deadline: Instant,
    ) -> ComputerUseResult<(ComputerUseScreenshot, Vec<ComputerUseScreenshot>, Value)> {
        self.invalidate_action_observations();
        validate_selection(selected)?;
        #[cfg(not(windows))]
        {
            let _ = (diagnostics, authorization_deadline);
            Err(ComputerUseError::new(
                ComputerUseErrorCode::BackendUnavailable,
                "native capture popup observations require Windows",
            ))
        }
        #[cfg(windows)]
        {
            self.ensure_active()?;
            let generation = dcc_cua_interrupt::interrupt_generation();
            self.capture_popup_observation_gate(generation, authorization_deadline)?;
            if self.pixel_observation_route != Some(PixelObservationRoute::ExplicitPixelsOnly) {
                return Err(ComputerUseError::new(
                    ComputerUseErrorCode::InvalidAction,
                    "native capture popup observations require explicit pixels_only",
                ));
            }
            let target = self.require_observed_target_available().await?;
            let proof = {
                let _exclusion = self
                    .control_banner
                    .as_ref()
                    .map(ControlBanner::begin_capture_exclusion)
                    .transpose()
                    .map_err(|error| {
                        map_indicator_error(
                            "exclude task indicator during native popup proof",
                            error,
                        )
                    })?;
                dcc_cua_platform_windows::read_native_capture_popup_proof(
                    target.pid,
                    target.window_id,
                    selected,
                )
                .map_err(|error| {
                    map_windows_window_mutation_error(
                        "read selected native capture popup proof",
                        error,
                    )
                })?
            };
            if self.capture_popup_initial_root != Some(proof.root.instance) {
                return Err(stale(
                    "the root native instance differs from the initial task binding",
                ));
            }
            let root = self
                .screenshot_pixels_only_with_diagnostics(diagnostics)
                .await?;
            let root_instance = super::window_commands::exact_native_observation_instance(
                &root.observation,
                &root.observation.observation_id,
                &self.scope,
                &self.session_id,
            )?;
            if root_instance != proof.root.instance {
                self.invalidate_action_observations();
                return Err(stale("root changed during composite capture"));
            }
            let result = self
                .capture_secondary_popup_frames(
                    selected,
                    &proof,
                    &root,
                    generation,
                    authorization_deadline,
                )
                .await;
            let (overlays, public) = match result {
                Ok(result) => result,
                Err(error) => {
                    self.invalidate_action_observations();
                    return Err(error);
                }
            };
            self.capture_popup_metadata = Some(CapturePopupMetadata {
                observation_id: root.observation.observation_id.clone(),
                session_id: self.session_id.clone(),
                read_at: Instant::now(),
                authorization_deadline,
                proof,
            });
            Ok((root, overlays, public))
        }
    }

    #[cfg(windows)]
    fn capture_popup_observation_gate(
        &self,
        generation: u64,
        authorization_deadline: Instant,
    ) -> ComputerUseResult<()> {
        self.ensure_active()?;
        if Instant::now() >= authorization_deadline {
            return Err(ComputerUseError::new(
                ComputerUseErrorCode::SessionRefreshRequired,
                "native capture popup owner authorization expired",
            ));
        }
        super::windows_pixel_input::validate_pixel_interrupt(
            generation,
            dcc_cua_interrupt::interrupt_generation(),
            self.control_banner_interrupted(),
        )?;
        interactive_desktop::require_exact_window_observation_available()
    }

    #[cfg(windows)]
    async fn capture_secondary_popup_frames(
        &self,
        selected: &[u64],
        proof: &dcc_cua_platform_windows::NativeCapturePopupProof,
        root: &ComputerUseScreenshot,
        generation: u64,
        authorization_deadline: Instant,
    ) -> ComputerUseResult<(Vec<ComputerUseScreenshot>, Value)> {
        let _exclusion = self
            .control_banner
            .as_ref()
            .map(ControlBanner::begin_capture_exclusion)
            .transpose()
            .map_err(|error| {
                map_indicator_error(
                    "exclude task indicator during selected popup capture",
                    error,
                )
            })?;
        let mut frames = Vec::new();
        let mut public = Vec::new();
        let mut coverage = Vec::new();
        for handle in selected {
            let window = proof
                .overlays
                .iter()
                .find(|window| window.window_handle == *handle)
                .ok_or_else(|| stale("selected native popup is missing from the proof"))?;
            let first = gated_exact_window_observation(
                || self.capture_popup_observation_gate(generation, authorization_deadline),
                || {
                    capture_exact_window(
                        window.process_id,
                        window.window_handle,
                        crate::capture_diagnostics::FramePurpose::Validation,
                    )
                },
            )
            .await?;
            let last = gated_exact_window_observation(
                || self.capture_popup_observation_gate(generation, authorization_deadline),
                || {
                    capture_exact_window(
                        window.process_id,
                        window.window_handle,
                        crate::capture_diagnostics::FramePurpose::Publication,
                    )
                },
            )
            .await?;
            validate_native_exact_window_pixel_evidence(
                &first.native_evidence,
                &last.native_evidence,
                first.mode,
            )?;
            validate_redaction_fence(&first.redacted_regions, &last.redacted_regions)?;
            if first.mode != last.mode
                || first.source_rect != last.source_rect
                || last.native_evidence.instance != window.instance
                || Some(last.native_evidence.bounds) != window.bounds
                || Some(last.native_evidence.visible_bounds) != window.visible_bounds
                || last.native_evidence.dpi != window.dpi
                || last.generation <= first.generation
            {
                return Err(stale(
                    "selected native popup instance or pixel geometry changed",
                ));
            }
            let (width, height) = png_dimensions(&last.data)
                .ok_or_else(|| stale("selected popup frame is not a complete PNG"))?;
            if last.source_rect[2..] != [width as i32, height as i32] {
                return Err(stale(
                    "selected popup dimensions do not match actual pixels",
                ));
            }
            coverage.push((last.source_rect, last.redacted_regions.clone()));
            let id = format!(
                "{}-popup-{}",
                root.observation.observation_id, window.window_handle
            );
            let mut provenance = json!({"process_id":window.process_id,"window_handle":window.window_handle,"native_instance":window.instance,
                "pixels_captured":true,"whole_desktop_capture":false,"scope":"window","secondary_observation":true,
                "backend":last.backend,"fallback":last.fallback,"accessibility_available":false,
                "capture_generation":last.generation,"native_window_bounds":last.native_evidence.bounds,"native_visible_bounds":last.native_evidence.visible_bounds,
                "window_dpi":last.dpi,"observation_mode":"pixels_only"});
            if let Some(geometry) = last.wgc_geometry {
                provenance["wgc_geometry"] = json!(geometry);
            }
            if last.mode != ExactWindowPixelCaptureMode::WindowContent {
                provenance["desktop_crop_bounds"] = json!(last.source_rect);
            }
            attach_capture_visibility(
                &mut provenance,
                last.mode,
                last.native_evidence,
                &last.redacted_regions,
            );
            let observation = ComputerUseObservation {
                observation_id: id.clone(),
                window_handle: window.window_handle,
                process_id: window.process_id,
                window_title: String::new(),
                width,
                height,
                source_rect: last.source_rect,
                capture_backend: last.backend.into(),
                capture_provenance: provenance,
                session_id: self.session_id.clone(),
            };
            public.push(json!({"window_handle":window.window_handle,"native_instance":window.instance,"observation_id":id,
                "source_rect":last.source_rect,"width":width,"height":height,"observation":observation}));
            frames.push(ComputerUseScreenshot {
                data: last.data,
                observation,
                accessibility: json!({"accessibility_available":false}),
            });
        }
        validate_root_capture_coverage(
            &root.observation.capture_provenance,
            &proof.redacted_regions,
            &coverage,
        )?;
        let after = dcc_cua_platform_windows::read_native_capture_popup_proof(
            proof.root.process_id,
            proof.root.window_handle,
            selected,
        )
        .map_err(|error| {
            map_windows_window_mutation_error("recheck selected native capture popup proof", error)
        })?;
        if proof != &after {
            return Err(stale(
                "native capture episode or selected root occlusion changed during capture",
            ));
        }
        self.capture_popup_observation_gate(generation, authorization_deadline)?;
        Ok((
            frames,
            json!({"purpose":"main_root_keyboard","one_use":true,"max_age_ms":5000,
            "root_observation_id":root.observation.observation_id,"capture_window_handle":proof.capture.window_handle,
            "selected_window_handles":selected,"overlays":public}),
        ))
    }
}

#[cfg(test)]
mod tests;
