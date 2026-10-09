use super::*;
use rstest::rstest;

mod native_frame_tests {
    use super::*;

    #[rstest]
    fn physical_frame_request_requires_exact_bounded_integers() {
        let request = ComputerUseWindowFrameRequest {
            x: 50.0,
            y: 800.0,
            width: 926.0,
            height: 680.0,
        };
        assert_eq!(
            exact_physical_window_frame(&request).unwrap(),
            [50, 800, 926, 680]
        );
        for values in [
            [0.5, 0.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, -1.0],
            [f64::NAN, 0.0, 1.0, 1.0],
            [0.0, f64::INFINITY, 1.0, 1.0],
            [i32::MAX as f64, 0.0, 1.0, 1.0],
            [0.0, 0.0, i32::MAX as f64 + 1.0, 1.0],
            [0.0, i32::MAX as f64, 1.0, 1.0],
        ] {
            assert!(
                exact_physical_window_frame(&ComputerUseWindowFrameRequest {
                    x: values[0],
                    y: values[1],
                    width: values[2],
                    height: values[3]
                })
                .is_err()
            );
        }
    }

    #[cfg(windows)]
    fn metadata() -> NativeWindowFrameMetadata {
        NativeWindowFrameMetadata {
            id: "native-state-1".into(),
            session_id: "session-1".into(),
            read_at: Instant::now(),
            state: dcc_cua_platform_windows::ExactWindowNativeState {
                process_id: 42,
                window_handle: 99,
                bounds: Some([80, 80, 1500, 1400]),
                visible_bounds: Some([93, 80, 1474, 1387]),
                dpi: 240,
                visible: true,
                minimized: false,
                foreground: false,
                instance: dcc_cua_platform_windows::ExactWindowPixelInstanceEvidence {
                    process_creation_time_100ns: 7,
                    window_thread_id: 8,
                    window_class_hash: 9,
                    owner_window_handle: 0,
                },
            },
        }
    }

    #[cfg(windows)]
    #[rstest]
    fn native_frame_metadata_is_session_target_and_five_second_bound() {
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            window_title: None,
        };
        metadata()
            .validate(
                "native-state-1",
                &scope,
                "session-1",
                Duration::from_secs(5),
            )
            .unwrap();
        for change in 0..12 {
            let mut proof = metadata();
            let mut actual_scope = scope.clone();
            let mut age = Duration::ZERO;
            let mut id = "native-state-1";
            let mut session = "session-1";
            match change {
                0 => id = "snapshot-1",
                1 => id = "",
                2 => session = "session-2",
                3 => age = Duration::from_secs(5) + Duration::from_nanos(1),
                4 => actual_scope.process_id = Some(43),
                5 => actual_scope.window_handle = Some(100),
                6 => proof.state.instance.process_creation_time_100ns = 0,
                7 => proof.state.instance.window_thread_id = 0,
                8 => proof.state.dpi = 0,
                9 => proof.state.bounds = None,
                10 => proof.state.visible = false,
                _ => proof.state.minimized = true,
            }
            assert!(
                proof.validate(id, &actual_scope, session, age).is_err(),
                "change {change}"
            );
        }
    }

    #[cfg(windows)]
    #[rstest]
    fn native_frame_failure_projection_never_claims_confirmed_effect() {
        for attempted in [false, true] {
            let error = native_frame_attempt_error(
                ComputerUseError::new(ComputerUseErrorCode::UserInterrupted, "stopped"),
                attempted,
            );
            assert_eq!(error.code, ComputerUseErrorCode::UserInterrupted);
            let details = error.details.unwrap();
            assert_eq!(details.action_attempted, Some(attempted));
            assert_eq!(details.effect_unknown, Some(attempted));
            assert_eq!(details.input_sent, Some(ComputerUseInputState::NotSent));
            assert_eq!(
                details.completion,
                Some(if attempted {
                    ComputerUseCompletionState::Unknown
                } else {
                    ComputerUseCompletionState::Known
                })
            );
            assert_eq!(details.automatic_input, Some(false));
            assert_eq!(details.blind_retry, Some(false));
            assert_eq!(details.fresh_observation_required, Some(true));
        }
    }

    #[cfg(windows)]
    #[rstest]
    fn native_frame_availability_transition_never_extends_or_reconstructs_authority() {
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            window_title: None,
        };
        let original = metadata();
        let read_at = original.read_at;
        let kept = retain_fresh_native_frame_metadata(
            Some(original),
            "native-state-1",
            &scope,
            "session-1",
        )
        .unwrap();
        assert_eq!(kept.read_at, read_at);
        let mut expired = metadata();
        expired.read_at = Instant::now() - Duration::from_secs(6);
        assert!(
            retain_fresh_native_frame_metadata(
                Some(expired),
                "native-state-1",
                &scope,
                "session-1"
            )
            .is_none()
        );
        assert!(
            retain_fresh_native_frame_metadata(Some(metadata()), "snapshot-1", &scope, "session-1")
                .is_none()
        );
        assert!(
            retain_fresh_native_frame_metadata(None, "native-state-1", &scope, "session-1")
                .is_none()
        );
        let mut missing = metadata();
        missing.state.visible_bounds = None;
        assert!(
            retain_fresh_native_frame_metadata(
                Some(missing),
                "native-state-1",
                &scope,
                "session-1"
            )
            .is_none()
        );
    }

    #[cfg(windows)]
    #[rstest]
    fn native_frame_confirmed_wire_uses_integer_frame_and_no_new_metadata_token() {
        let requested = [50, 800, 926, 680];
        let mut actual = metadata().state;
        actual.bounds = Some(requested);
        let wire = native_frame_confirmed_result(requested, "consumed-native-state", actual);
        assert_eq!(
            wire["requested_frame"],
            json!({"x":50,"y":800,"width":926,"height":680})
        );
        assert!(wire["requested_frame"]["x"].is_i64());
        assert_eq!(wire["applied_frame"], json!(requested));
        assert_eq!(wire["state"]["native_instance"], wire["native_instance"]);
        assert_eq!(wire["state"]["bounds"], wire["applied_frame"]);
        assert!(wire["state"].get("window_state_id").is_none());
        assert!(wire["state"].get("instance").is_none());
        assert_eq!(wire["window_state_id"], "consumed-native-state");
        assert_eq!(wire["success"], true);
        assert_eq!(wire["effect"], "confirmed");
    }
}

#[cfg(all(test, windows))]
mod minimize_tests {
    use super::*;
    use rstest::rstest;

    fn observation() -> ComputerUseObservation {
        ComputerUseObservation {
            observation_id: "obs-1".into(),
            window_handle: 99,
            process_id: 42,
            window_title: String::new(),
            width: 1500,
            height: 1400,
            source_rect: [80, 80, 1500, 1400],
            capture_backend: "dcc-cua-wgc-exact-window".into(),
            session_id: "test".into(),
            capture_provenance: json!({"process_id":42,"window_handle":99,"pixels_captured":true,
                "whole_desktop_capture":false,"scope":"window","capture_generation":1,"window_dpi":240,
                "native_instance":{"process_creation_time_100ns":7,"window_thread_id":8,
                    "window_class_hash":9,"owner_window_handle":0}}),
        }
    }

    #[rstest]
    #[case("valid", true)]
    #[case("wrong_id", false)]
    #[case("wrong_session", false)]
    #[case("wrong_pid", false)]
    #[case("wrong_hwnd", false)]
    #[case("missing_instance", false)]
    #[case("zero_creation", false)]
    #[case("zero_thread", false)]
    #[case("thread_overflow", false)]
    #[case("missing_owner", false)]
    #[case("no_pixels", false)]
    #[case("wrong_backend", false)]
    #[case("zero_generation", false)]
    fn exact_minimize_requires_actual_latest_capture_instance(
        #[case] scenario: &str,
        #[case] valid: bool,
    ) {
        let mut observed = observation();
        let requested = if scenario == "wrong_id" {
            "obs-old"
        } else {
            "obs-1"
        };
        match scenario {
            "wrong_session" => observed.session_id = "other".into(),
            "wrong_pid" => observed.process_id = 43,
            "wrong_hwnd" => observed.window_handle = 100,
            "missing_instance" => observed.capture_provenance["native_instance"] = Value::Null,
            "zero_creation" => {
                observed.capture_provenance["native_instance"]["process_creation_time_100ns"] =
                    json!(0)
            }
            "zero_thread" => {
                observed.capture_provenance["native_instance"]["window_thread_id"] = json!(0)
            }
            "thread_overflow" => {
                observed.capture_provenance["native_instance"]["window_thread_id"] =
                    json!(4294967296_u64)
            }
            "missing_owner" => {
                observed.capture_provenance["native_instance"]
                    .as_object_mut()
                    .unwrap()
                    .remove("owner_window_handle");
            }
            "no_pixels" => observed.capture_provenance["pixels_captured"] = json!(false),
            "wrong_backend" => observed.capture_backend = "cua-driver-sdk".into(),
            "zero_generation" => observed.capture_provenance["capture_generation"] = json!(0),
            _ => {}
        }
        let scope = ComputerUseTargetScope {
            process_id: Some(42),
            window_handle: Some(99),
            window_title: None,
        };
        assert_eq!(
            exact_native_observation_instance(&observed, requested, &scope, "test").is_ok(),
            valid
        );
    }

    #[rstest]
    fn native_observation_geometry_requires_actual_unscaled_wgc_proof() {
        use dcc_cua_platform_windows::*;
        let mut observed = observation();
        observed.width = 646;
        observed.height = 495;
        observed.source_rect = [73, 80, 646, 495];
        let native = NativeWindowGeometry {
            win32_bounds: [60, 80, 672, 508],
            dwm_bounds: Some(observed.source_rect),
            dpi: 240,
        };
        let shape = WgcFrameGeometry {
            item_size_before: [646, 495],
            item_size_after: [646, 495],
            pool_size: [646, 495],
            content_size: [646, 495],
            texture_size: [646, 495],
            row_pitch_bytes: 2688,
        };
        let proof = resolve_exact_wgc_geometry(native, native, shape, 646 * 495 * 4).unwrap();
        observed.capture_provenance["native_window_bounds"] = json!(native.win32_bounds);
        observed.capture_provenance["native_visible_bounds"] = json!(native.dwm_bounds.unwrap());
        observed.capture_provenance["wgc_geometry"] = json!(proof);
        assert_eq!(
            exact_native_observation_geometry(&observed).unwrap(),
            native
        );
        for change in 0..7 {
            let mut invalid = observed.clone();
            match change {
                0 => invalid.width -= 1,
                1 => invalid.source_rect[0] -= 1,
                2 => invalid.capture_provenance["native_visible_bounds"] = Value::Null,
                3 => invalid.capture_provenance["wgc_geometry"] = Value::Null,
                4 => {
                    invalid.capture_provenance["wgc_geometry"]["frame"]["item_size_after"][0] =
                        json!(647)
                }
                5 => invalid.capture_provenance["wgc_geometry"]["bgra_byte_len"] = json!(1),
                _ => invalid.capture_provenance["wgc_geometry"]["origin"] = json!("win32_window"),
            }
            assert!(
                exact_native_observation_geometry(&invalid).is_err(),
                "change {change}"
            );
        }
        let instance = ExactWindowPixelInstanceEvidence {
            process_creation_time_100ns: 7,
            window_thread_id: 8,
            window_class_hash: 9,
            owner_window_handle: 0,
        };
        let current = ExactWindowPixelEvidence {
            process_id: 42,
            window_handle: 99,
            bounds: native.win32_bounds,
            visible_bounds: native.dwm_bounds.unwrap(),
            dpi: 240,
            visible: true,
            minimized: false,
            unobscured: false,
            visibility_failure: None,
            instance,
        };
        validate_observed_minimize_geometry(native, instance, &current).unwrap();
        for index in 0..4 {
            let mut changed = current;
            changed.visible_bounds[index] += 1;
            assert!(validate_observed_minimize_geometry(native, instance, &changed).is_err());
        }
    }
}
