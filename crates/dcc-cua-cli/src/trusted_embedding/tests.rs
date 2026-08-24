use rstest::rstest;

use super::*;

#[rstest]
fn accepts_exact_packaged_codex_parent() {
    let attestation = validate_codex_identity("CoDeX.ExE", TRUSTED_CODEX_PACKAGE_FAMILY).unwrap();
    assert_eq!(attestation.label(), "codex_desktop_windows_package");
}

#[rstest]
fn accepts_exact_packaged_claude_parent() {
    let attestation =
        validate_packaged_identity("Claude.exe", TRUSTED_CLAUDE_PACKAGE_FAMILY).unwrap();
    assert_eq!(attestation.label(), "claude_desktop_windows_package");
}

#[rstest]
fn accepts_verified_codebuddy_parent() {
    let attestation = validate_authenticode_identity(
        "CodeBuddy CN.exe",
        "CodeBuddy CN",
        TRUSTED_TENCENT_PUBLISHER,
    )
    .unwrap();
    assert_eq!(
        attestation.label(),
        "codebuddy_cn_desktop_windows_authenticode"
    );
}

#[rstest]
fn accepts_verified_workbuddy_parent() {
    let attestation =
        validate_authenticode_identity("WorkBuddy.exe", "WorkBuddy", TRUSTED_TENCENT_PUBLISHER)
            .unwrap();
    assert_eq!(
        attestation.label(),
        "workbuddy_desktop_windows_authenticode"
    );
}

#[rstest]
fn accepts_observed_codebuddy_without_a_package_identity() {
    let attestation = validate_observed_identity(
        "CodeBuddy CN.exe",
        None,
        Some(VerifiedAuthenticodeIdentity {
            product_name: "CodeBuddy CN",
            publisher: TRUSTED_TENCENT_PUBLISHER,
        }),
    )
    .unwrap();
    assert_eq!(
        attestation.label(),
        "codebuddy_cn_desktop_windows_authenticode"
    );
}

#[rstest]
fn rejects_a_renamed_tencent_binary_with_the_wrong_product() {
    let error = validate_authenticode_identity(
        "CodeBuddy CN.exe",
        "Unrelated Tencent Product",
        TRUSTED_TENCENT_PUBLISHER,
    )
    .unwrap_err();
    assert!(error.to_string().contains("identity is not supported"));
}

#[rstest]
fn rejects_a_codebuddy_binary_from_an_untrusted_publisher() {
    let error =
        validate_authenticode_identity("CodeBuddy CN.exe", "CodeBuddy CN", "Untrusted Publisher")
            .unwrap_err();
    assert!(error.to_string().contains("publisher is not trusted"));
}

#[rstest]
fn does_not_downgrade_an_untrusted_package_to_authenticode() {
    let error = validate_observed_identity(
        "CodeBuddy CN.exe",
        Some("Untrusted.Package_family"),
        Some(VerifiedAuthenticodeIdentity {
            product_name: "CodeBuddy CN",
            publisher: TRUSTED_TENCENT_PUBLISHER,
        }),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("supported packaged desktop runtime")
    );
}

#[rstest]
fn rejects_shell_parent_even_if_package_family_is_claimed() {
    let error = validate_codex_identity("pwsh.exe", TRUSTED_CODEX_PACKAGE_FAMILY).unwrap_err();
    assert!(error.to_string().contains("not the Codex desktop runtime"));
}

#[rstest]
fn rejects_untrusted_codex_package() {
    let error = validate_codex_identity("codex.exe", "OpenAI.Codex_untrusted").unwrap_err();
    assert!(error.to_string().contains("package identity"));
}

#[rstest]
fn rejects_untrusted_claude_package() {
    let error = validate_packaged_identity("claude.exe", "Claude_untrusted").unwrap_err();
    assert!(error.to_string().contains("Claude package identity"));
}
