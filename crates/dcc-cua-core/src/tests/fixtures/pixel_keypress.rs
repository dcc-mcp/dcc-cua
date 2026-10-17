// Shared pure request fixture. The native E2E and Core policy test use this
// same builder; it does not expose or change the production action policy.
use super::ComputerUseAction;
#[allow(unused_imports)]
use rstest::rstest;

pub(super) fn request(observation_id: String) -> ComputerUseAction {
    ComputerUseAction {
        action: "keypress".into(),
        keys: vec!["F8".into()],
        observation_id: Some(observation_id),
        ..Default::default()
    }
}
