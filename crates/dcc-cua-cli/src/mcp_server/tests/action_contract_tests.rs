use super::*;
use rstest::rstest;

#[rstest]
fn action_schema_exposes_required_fields_and_coordinate_contract() {
    let call = tool_definitions()
        .into_iter()
        .find(|tool| tool["name"] == "dcc_cua_task_call")
        .unwrap();
    let branch = call["inputSchema"]["allOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["if"]["properties"]["method"]["const"] == "execute_action")
        .unwrap();
    let params = &branch["then"]["properties"]["params"];
    assert_eq!(params["required"], json!(["observation_id", "action"]));
    let action = &params["properties"]["action"];
    assert_eq!(
        action["required"],
        json!(["action", "input_kind", "intent"])
    );
    assert!(
        action["properties"]["x"]["description"]
            .as_str()
            .unwrap()
            .contains("snapshot.coordinate_space")
    );
    assert!(
        action["properties"]["keys"]["description"]
            .as_str()
            .unwrap()
            .contains("not key")
    );
    assert_eq!(action["properties"]["path"]["minItems"], 2);
    assert_eq!(action["properties"]["path"]["maxItems"], 201);
    assert_eq!(action["properties"]["duration_ms"]["maximum"], 10000);
    assert!(
        params["properties"]["capture_after"]["description"]
            .as_str()
            .unwrap()
            .contains("do not call snapshot again")
    );
    assert_eq!(
        params["allOf"][0]["then"]["required"],
        json!(["accessibility_state_id"])
    );
}
