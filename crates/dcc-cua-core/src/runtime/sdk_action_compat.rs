use super::*;
use std::collections::HashMap;

/// Only tokens actually returned with this observation may replace row selectors.
/// Duplicate or tokenless rows remain unavailable instead of inventing SDK tokens.
pub(super) struct ObservedSdkElementTokens {
    observation_id: String,
    session_id: String,
    process_id: u32,
    window_handle: u64,
    tokens: HashMap<u32, Option<String>>,
}

impl ComputerUseSession {
    pub(super) fn sdk_action_command(
        action: &ComputerUseAction,
        session: &str,
        target: &WindowTarget,
        local_coordinates: bool,
    ) -> ComputerUseResult<DriverActionCommand> {
        let mut command = action_arguments(action, session, target)?;
        if cfg!(target_os = "macos") && local_coordinates && action.action == "move" {
            // A visual/live fallback has already converted screenshot pixels
            // to native window units. It has no matching SDK screenshot-scale
            // evidence: use the window-scoped overlay route, never scale twice.
            let arguments = command.arguments.as_object_mut().expect("action arguments");
            arguments.remove("target");
            map_window_cursor_move(arguments, target)?;
        }
        Ok(command)
    }

    pub(super) fn remember_sdk_element_tokens(
        &mut self,
        accessibility: &Value,
        observation: &ComputerUseObservation,
    ) {
        let mut tokens = HashMap::new();
        for element in accessibility["elements"].as_array().into_iter().flatten() {
            let Some(index) = element["element_index"]
                .as_u64()
                .and_then(|index| u32::try_from(index).ok())
            else {
                continue;
            };
            let token = element["element_token"]
                .as_str()
                .filter(|token| {
                    !token.is_empty() && token.chars().count() <= MAX_ELEMENT_TOKEN_CHARS
                })
                .map(str::to_owned);
            tokens
                .entry(index)
                .and_modify(|value| *value = None)
                .or_insert(token);
        }
        self.sdk_element_tokens = Some(ObservedSdkElementTokens {
            observation_id: observation.observation_id.clone(),
            session_id: observation.session_id.clone(),
            process_id: observation.process_id,
            window_handle: observation.window_handle,
            tokens,
        });
    }

    pub(super) fn resolve_sdk_element_action(
        &self,
        action: &ComputerUseAction,
        observation: &ComputerUseObservation,
    ) -> ComputerUseResult<Option<ComputerUseAction>> {
        let Some(index) = action
            .element_index
            .filter(|_| action.element_token.is_none())
        else {
            return Ok(None);
        };
        let token = self.sdk_element_tokens.as_ref()
            .filter(|tokens| {
                tokens.observation_id == observation.observation_id
                    && tokens.session_id == observation.session_id
                    && tokens.process_id == observation.process_id
                    && tokens.window_handle == observation.window_handle
            })
            .and_then(|tokens| tokens.tokens.get(&index))
            .and_then(Option::as_ref)
            .ok_or_else(|| ComputerUseError::new(
                ComputerUseErrorCode::StaleObservation,
                "element_index has no unique SDK element_token in the current observation; take a fresh semantic snapshot",
            ))?;
        let mut resolved = action.clone();
        resolved.element_index = None;
        resolved.element_token = Some(token.clone());
        Ok(Some(resolved))
    }
}

#[cfg(test)]
mod tests;
