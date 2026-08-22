use rstest::rstest;

use super::parse_pairs;

#[rstest]
fn duplicate_pair_is_rejected() {
    let error =
        parse_pairs(&["app=excel".into(), "app=powerpoint".into()], "identity").unwrap_err();
    assert!(error.to_string().contains("duplicate"));
}

#[rstest]
fn portable_pairs_are_parsed_by_the_cli_adapter() {
    let pairs = parse_pairs(
        &["document=sha256:ABC".into(), "kind=Workbook".into()],
        "identity",
    )
    .expect("portable pairs");
    assert_eq!(pairs["document"], "sha256:ABC");
    assert_eq!(pairs["kind"], "Workbook");
}
