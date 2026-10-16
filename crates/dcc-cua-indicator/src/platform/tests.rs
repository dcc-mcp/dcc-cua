use super::*;
use rstest::rstest;

mod capture_source_tests {
    use super::*;

    #[rstest]
    fn inactive_capture_source_refuses_before_native_exclusion_and_clone_does_not_revive_it() {
        let active = Arc::new(AtomicBool::new(false));
        let source = PlatformCaptureExclusionSource {
            active: Arc::clone(&active),
        };
        let cloned = source.clone();
        assert!(source.begin().is_err());
        active.store(true, Ordering::Release);
        assert!(cloned.validate_active().is_ok());
        active.store(false, Ordering::Release);
        assert!(source.validate_active().is_err());
        assert!(cloned.begin().is_err());
    }
}
