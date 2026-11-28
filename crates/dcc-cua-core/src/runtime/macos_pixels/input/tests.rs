use super::Chord;
use rstest::rstest;

#[rstest]
#[case("ctrl+f01", false)]
#[case("f13", false)]
#[case("ctrl+f1", true)]
#[case("f12", true)]
fn function_keys_use_canonical_sdk_names(#[case] step: &str, #[case] supported: bool) {
    assert_eq!(Chord::parse(step).is_ok(), supported);
}
