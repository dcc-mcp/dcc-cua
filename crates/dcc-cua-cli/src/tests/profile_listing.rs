use rstest::rstest;

use super::*;

#[rstest]
fn defaults_to_valid_entries_and_keeps_invalid_diagnostics_explicit() {
    let store_root = tempfile::tempdir().expect("Profile store");
    let invalid_root = store_root.path().join("artstation");
    std::fs::create_dir(&invalid_root).expect("invalid Profile directory");
    let store = dcc_cua_profiles::ProfileStore::open(store_root.path()).expect("Profile store");

    assert_eq!(
        profile_package::profile_listing_state(&[]).expect("default listing state"),
        profile_package::ProfileListingState::Valid
    );
    assert!(
        profile_package::installed_profile_summaries(
            &store,
            profile_package::ProfileListingState::Valid,
        )
        .is_empty(),
        "the default listing must not advertise unusable Profile packages"
    );

    let invalid = profile_package::installed_profile_summaries(
        &store,
        profile_package::ProfileListingState::Invalid,
    );
    assert_eq!(invalid.len(), 1);
    assert_eq!(invalid[0]["id"], "artstation");
    assert_eq!(invalid[0]["status"], "invalid");
    assert!(
        invalid[0]["error"]
            .as_str()
            .expect("invalid reason")
            .contains("profile-package.json"),
        "the diagnostic listing must name the missing package manifest"
    );

    let all = profile_package::installed_profile_summaries(
        &store,
        profile_package::ProfileListingState::All,
    );
    assert_eq!(all, invalid);
    assert!(
        profile_package::profile_listing_state(&strings(["--state", "broken"])).is_err(),
        "unknown listing states must fail closed"
    );
}
