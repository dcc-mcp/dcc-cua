use serde_json::Value;
use uuid::Uuid;

pub(crate) fn new_runtime_session_id(scope: &str) -> String {
    format!("dcc-cua-{scope}-{}", Uuid::new_v4())
}

pub(crate) fn rewrite_session_aliases(value: &mut Value, aliases: &[(&str, &str)]) {
    match value {
        Value::Array(values) => {
            for value in values {
                rewrite_session_aliases(value, aliases);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "session" | "session_id" | "_session_id")
                    && let Value::String(session_id) = value
                    && let Some((_, public_id)) = aliases
                        .iter()
                        .find(|(runtime_id, _)| session_id.as_str() == *runtime_id)
                {
                    *session_id = (*public_id).to_owned();
                }
                rewrite_session_aliases(value, aliases);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
