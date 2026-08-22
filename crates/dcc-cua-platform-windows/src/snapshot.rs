use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::UiaError;

pub(crate) const TOKEN_PREFIX: &str = "dcc-wuia";

#[derive(Debug, Clone)]
pub(crate) struct ElementFence {
    pub control_id: String,
    pub identity: String,
    pub is_password: bool,
    pub name: String,
    pub automation_id: String,
    pub class_name: String,
    pub policy_tier: String,
}

#[derive(Debug)]
pub(crate) struct SnapshotState {
    pub id: String,
    pub fences: Vec<ElementFence>,
}

pub(crate) fn normalize(raw: &Value) -> Result<(Value, SnapshotState), UiaError> {
    let root = raw
        .get("root")
        .ok_or_else(|| UiaError::OperationFailed("snapshot returned no scoped root".into()))?;
    let snapshot_id = Uuid::new_v4().simple().to_string();
    let mut elements = Vec::new();
    let mut fences = Vec::new();
    flatten(root, 0, &snapshot_id, &mut elements, &mut fences);
    let node_count = elements.len();
    let value = json!({
        "accessibility_available": true,
        "backend": "windows_uia",
        "degraded": false,
        "element_bounds_coordinate_space": "virtual_desktop",
        "elements": elements,
        "focus_runtime_id": raw.get("focus_runtime_id").cloned().unwrap_or(Value::Null),
        "node_count": node_count,
        "snapshot_id": snapshot_id,
    });
    Ok((
        value,
        SnapshotState {
            id: snapshot_id,
            fences,
        },
    ))
}

fn flatten(
    node: &Value,
    depth: u32,
    snapshot_id: &str,
    elements: &mut Vec<Value>,
    fences: &mut Vec<ElementFence>,
) {
    let index = elements.len() as u32;
    let runtime_id = string(node, "runtime_id");
    let fallback_path = string(node, "fallback_path");
    let (control_id, identity) = if runtime_id.is_empty() {
        (format!("uia:path:{fallback_path}"), fallback_path)
    } else {
        (format!("uia:{runtime_id}"), runtime_id)
    };
    let name = string(node, "name");
    let automation_id = string(node, "automation_id");
    let class_name = string(node, "class_name");
    let policy_tier = string(node, "policy_tier");
    fences.push(ElementFence {
        control_id,
        identity,
        is_password: node["is_password"].as_bool().unwrap_or(false),
        name: name.clone(),
        automation_id: automation_id.clone(),
        class_name: class_name.clone(),
        policy_tier: policy_tier.clone(),
    });

    let mut element = Map::new();
    element.insert("element_index".into(), json!(index));
    element.insert(
        "element_token".into(),
        json!(format!("{TOKEN_PREFIX}:{snapshot_id}:{index}")),
    );
    element.insert("role".into(), json!(role(node)));
    element.insert("name".into(), json!(name));
    element.insert("label".into(), json!(string(node, "name")));
    element.insert("value".into(), node["value"].clone());
    element.insert("enabled".into(), node["enabled"].clone());
    element.insert("checked".into(), node["checked"].clone());
    element.insert("bounds".into(), node["bounds"].clone());
    element.insert("depth".into(), json!(depth));
    element.insert("automation_id".into(), json!(automation_id));
    element.insert("class_name".into(), json!(class_name));
    element.insert("is_password".into(), node["is_password"].clone());
    element.insert("offscreen".into(), node["offscreen"].clone());
    element.insert("focused".into(), node["focused"].clone());
    element.insert("policy_tier".into(), json!(policy_tier));
    element.insert("backend".into(), json!("windows_uia"));
    elements.push(Value::Object(element));

    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            flatten(
                child,
                depth.saturating_add(1),
                snapshot_id,
                elements,
                fences,
            );
        }
    }
}

pub(crate) fn resolve_index(
    state: &SnapshotState,
    _element_index: Option<u32>,
    element_token: Option<&str>,
) -> Result<usize, UiaError> {
    if let Some(token) = element_token {
        let mut parts = token.split(':');
        let valid_prefix = parts.next() == Some(TOKEN_PREFIX);
        let snapshot_id = parts.next();
        let index = parts.next().and_then(|value| value.parse::<usize>().ok());
        if !valid_prefix || parts.next().is_some() || snapshot_id != Some(state.id.as_str()) {
            return Err(UiaError::StaleSnapshot(
                "element token does not belong to the current snapshot".into(),
            ));
        }
        return index
            .filter(|index| *index < state.fences.len())
            .ok_or_else(|| UiaError::StaleSnapshot("element token index is invalid".into()));
    }
    Err(UiaError::InvalidAction(
        "a current element token is required for mutation".into(),
    ))
}

fn role(node: &Value) -> String {
    let control_type = string(node, "control_type");
    control_type
        .strip_prefix("ControlType.")
        .unwrap_or(&control_type)
        .to_owned()
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
