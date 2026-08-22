use rstest::rstest;
use serde_json::json;

use crate::windows_uia_fallback::accessibility_has_closed_policy_tiers;

#[rstest]
#[case(json!({}), false)]
#[case(json!({"elements": []}), false)]
#[case(json!({"elements": [{"policy_tier": ""}]}), false)]
#[case(json!({"elements": [{"policy_tier": "unknown"}]}), false)]
#[case(json!({"elements": [{"policy_tier": "task_grant"}, {"policy_tier": "hard_deny"}]}), true)]
fn semantic_action_evidence_requires_a_closed_tier_for_every_element(
    #[case] accessibility: serde_json::Value,
    #[case] expected: bool,
) {
    assert_eq!(
        accessibility_has_closed_policy_tiers(&accessibility),
        expected
    );
}
