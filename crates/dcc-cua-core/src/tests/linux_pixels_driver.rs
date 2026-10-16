use rstest::rstest;

#[rstest]
fn read_only_pixels_runtime_disables_preparation_and_refuses_nested_startup() {
    let options = crate::driver_factory::read_only_pixels_host_options(false).unwrap();
    assert!(!options.prepare_desktop_environment);
    assert!(!options.cursor.enabled);
    assert!(options.register_host_tools.is_some());
    assert!(options.host_owns_permission_ux);
    assert!(crate::driver_factory::read_only_pixels_host_options(true).is_err());
}

#[rstest]
fn linux_read_only_runtime_mode_cannot_mix_with_general_runtime_in_either_order() {
    use crate::driver_factory::{LinuxRuntimeMode, claim_linux_runtime_mode};
    for (first, second) in [
        (LinuxRuntimeMode::General, LinuxRuntimeMode::ReadOnlyPixels),
        (LinuxRuntimeMode::ReadOnlyPixels, LinuxRuntimeMode::General),
    ] {
        let mode = std::sync::OnceLock::new();
        claim_linux_runtime_mode(&mode, first).unwrap();
        claim_linux_runtime_mode(&mode, first).unwrap();
        assert!(claim_linux_runtime_mode(&mode, second).is_err());
        assert_eq!(mode.get(), Some(&first));
    }
}
