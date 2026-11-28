use super::*;

impl ComputerUseSession {
    pub fn banner_status(&self) -> Value {
        if self
            .live_observation
            .as_ref()
            .is_some_and(|observation| !observation.is_active())
        {
            self.set_banner_live_observation(false);
        }
        self.control_banner.as_ref().map_or_else(
            || {
                json!({
                    "backend": "unavailable",
                    "healthy": false,
                    "running": false,
                    "last_error": "control banner is not running",
                    "failure": {
                        "kind": "backend",
                        "message": "control banner is not running",
                    },
                    "visible": false,
                    "target_frame_visible": false,
                    "interrupted": false,
                    "stop_key": "Escape",
                    "activity": "stopping",
                    "activity_label": "Stopping…",
                    "recording": false,
                    "live_observation": false,
                    "placement": "unavailable",
                })
            },
            |banner| json!(banner.status()),
        )
    }

    pub fn control_banner_interrupted(&self) -> bool {
        self.control_banner
            .as_ref()
            .is_some_and(ControlBanner::interrupted)
    }

    pub fn control_banner_failure(&self) -> Option<ComputerUseError> {
        let failure = self.control_banner.as_ref()?.failure()?;
        let code = match failure.kind {
            BannerFailureKind::TargetLost => ComputerUseErrorCode::InvalidTarget,
            BannerFailureKind::Backend => ComputerUseErrorCode::BackendUnavailable,
            BannerFailureKind::Rendering => return None,
        };
        Some(ComputerUseError::new(code, failure.message))
    }

    pub fn set_banner_activity(&self, activity: BannerActivity) {
        if let Some(banner) = &self.control_banner {
            banner.set_activity(activity);
        }
    }

    pub(super) fn begin_banner_activity(
        &self,
        activity: BannerActivity,
    ) -> Option<BannerActivityGuard> {
        self.control_banner
            .as_ref()
            .map(|banner| banner.begin_activity(activity))
    }

    pub(super) fn set_banner_recording(&self, recording: bool) {
        if let Some(banner) = &self.control_banner {
            banner.set_recording(recording);
        }
    }

    pub(super) fn set_banner_live_observation(&self, live_observation: bool) {
        if let Some(banner) = &self.control_banner {
            banner.set_live_observation(live_observation);
        }
    }
}
