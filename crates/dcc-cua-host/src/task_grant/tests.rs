use super::*;
use rstest::rstest;

mod recording_tests {
    use super::*;
    use serde_json::json;

    fn grant() -> serde_json::Value {
        json!({"task_grant_id":"grant","application_label":"Test",
            "observation_mode":"pixels_only","process_id":42,"window_handle":77,
            "allow_recording":true,"allow_live_observation":true,
            "recording_output_dir":std::env::temp_dir().join("owned-recording").to_string_lossy(),
            "task_authorization_id":"task-auth-test","task_authorization_window_capability":"window"})
    }

    #[rstest]
    fn native_recording_grant_requires_explicit_trusted_manual_directory() {
        serde_json::from_value::<TaskGrant>(grant())
            .unwrap()
            .validate_identity()
            .unwrap();
        for field in [
            "recording_output_dir",
            "task_authorization_id",
            "allow_live_observation",
        ] {
            let mut value = grant();
            value.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<TaskGrant>(value)
                    .unwrap()
                    .validate_identity()
                    .is_err(),
                "{field}"
            );
        }
        let mut automatic = grant();
        automatic["showcase_output_dir"] = automatic["recording_output_dir"].clone();
        assert!(
            serde_json::from_value::<TaskGrant>(automatic)
                .unwrap()
                .validate_identity()
                .is_err()
        );
    }

    #[rstest]
    fn native_recording_directory_refuses_relative_traversal_and_alternate_streams() {
        for value in ["relative", "", "  ", "bad\npath"] {
            assert!(validate_recording_output_dir(value).is_err());
        }
        let directory = std::env::temp_dir().join("owned-recording");
        for bad in [
            directory.join("..").join("foreign"),
            directory.join(".").join("foreign"),
            directory.join("output:stream"),
        ] {
            assert!(validate_recording_output_dir(bad.to_str().unwrap()).is_err());
        }
        #[cfg(windows)]
        for value in [
            r"\\server\share\recording",
            r"\\?\C:\recording",
            r"\\.\C:\recording",
            r"C:relative",
            r"C:\recording.\clip",
            r"C:\recording \clip",
        ] {
            assert!(validate_recording_output_dir(value).is_err(), "{value}");
        }
    }

    #[rstest]
    fn native_recording_public_method_scope_is_explicit_without_semantic_access() {
        for method in [
            "live_observation_start",
            "live_observation_state",
            "live_observation_stop",
            "recording_start",
            "recording_state",
            "recording_stop",
        ] {
            assert!(TaskObservationMode::PixelsOnly.permits_method(method));
        }
        assert!(!TaskObservationMode::PixelsOnly.permits_method("accessibility_snapshot"));
        assert!(!TaskObservationMode::PixelsOnly.permits_method("call_tool"));
    }
}
