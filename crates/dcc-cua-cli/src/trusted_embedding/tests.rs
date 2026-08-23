use rstest::rstest;

use super::*;

#[rstest]
fn accepts_exact_packaged_codex_parent() {
    let attestation = validate_codex_identity("CoDeX.ExE", TRUSTED_CODEX_PACKAGE_FAMILY).unwrap();
    assert_eq!(attestation.label(), "codex_desktop_windows_package");
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
