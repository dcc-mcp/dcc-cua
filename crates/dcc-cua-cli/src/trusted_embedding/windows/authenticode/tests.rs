use rstest::rstest;

use super::*;

#[rstest]
fn rejects_the_unsigned_test_binary() {
    let executable = std::env::current_exe().unwrap();
    let error = match verify(executable.to_str().unwrap()) {
        Err(error) => error,
        Ok(_) => panic!("the unsigned test binary must not be trusted"),
    };
    assert!(error.to_string().contains("valid Authenticode trust chain"));
}

#[rstest]
fn reads_identity_from_a_signed_windows_executable() {
    let executable = std::path::PathBuf::from(std::env::var_os("ProgramFiles").unwrap())
        .join("PowerShell")
        .join("7")
        .join("pwsh.exe");
    let identity = verify(executable.to_str().unwrap()).unwrap();
    assert!(!identity.publisher.is_empty());
    assert!(!identity.product_name.is_empty());
}
