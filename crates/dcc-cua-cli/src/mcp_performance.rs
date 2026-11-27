//! Connection-scoped MCP performance and token accounting.
//!
//! Protocol token counts are estimates based on UTF-8 message bytes. Exact
//! model usage belongs to the Agent Host and can be reported through the
//! content-free `notifications/dcc-cua/model_usage` notification or the
//! `_meta.dcc-cua.model_usage` envelope field.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub(crate) const SCHEMA: &str = "dcc-cua.mcp-performance.v1";
const MAX_POST_OBSERVATIONS: usize = 256;

#[derive(Debug)]
pub(crate) struct McpPerformance {
    started: Instant,
    transport_messages_total: u64,
    transport_input_bytes_total: u64,
    transport_output_bytes_total: u64,
    transport_input_tokens_estimated_total: u64,
    transport_output_tokens_estimated_total: u64,
    transport_latency_samples_total: u64,
    transport_latency_ms_total: u64,
    transport_latency_ms_max: u64,
    tool_calls_total: u64,
    tool_latency_ms_total: u64,
    tool_latency_ms_max: u64,
    methods: BTreeMap<String, u64>,
    standalone_snapshots_total: u64,
    post_action_snapshots_total: u64,
    observation_required_total: u64,
    post_snapshot_reuses_total: u64,
    reported_model_calls_total: u64,
    reported_input_tokens_total: u64,
    reported_output_tokens_total: u64,
    post_observation_ids: BTreeSet<String>,
}

impl Default for McpPerformance {
    fn default() -> Self {
        Self::new()
    }
}

impl McpPerformance {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            transport_messages_total: 0,
            transport_input_bytes_total: 0,
            transport_output_bytes_total: 0,
            transport_input_tokens_estimated_total: 0,
            transport_output_tokens_estimated_total: 0,
            transport_latency_samples_total: 0,
            transport_latency_ms_total: 0,
            transport_latency_ms_max: 0,
            tool_calls_total: 0,
            tool_latency_ms_total: 0,
            tool_latency_ms_max: 0,
            methods: BTreeMap::new(),
            standalone_snapshots_total: 0,
            post_action_snapshots_total: 0,
            observation_required_total: 0,
            post_snapshot_reuses_total: 0,
            reported_model_calls_total: 0,
            reported_input_tokens_total: 0,
            reported_output_tokens_total: 0,
            post_observation_ids: BTreeSet::new(),
        }
    }

    pub(crate) fn record_transport_input(&mut self, bytes: usize) {
        self.transport_messages_total = self.transport_messages_total.saturating_add(1);
        self.transport_input_bytes_total = self
            .transport_input_bytes_total
            .saturating_add(bytes as u64);
        self.transport_input_tokens_estimated_total = self
            .transport_input_tokens_estimated_total
            .saturating_add(estimate_tokens(bytes));
    }

    pub(crate) fn record_transport_output(&mut self, bytes: usize) {
        self.transport_output_bytes_total = self
            .transport_output_bytes_total
            .saturating_add(bytes as u64);
        self.transport_output_tokens_estimated_total = self
            .transport_output_tokens_estimated_total
            .saturating_add(estimate_tokens(bytes));
    }

    pub(crate) fn record_transport_latency(&mut self, elapsed: Duration) {
        let elapsed_ms = duration_ms(elapsed);
        self.transport_latency_samples_total =
            self.transport_latency_samples_total.saturating_add(1);
        self.transport_latency_ms_total =
            self.transport_latency_ms_total.saturating_add(elapsed_ms);
        self.transport_latency_ms_max = self.transport_latency_ms_max.max(elapsed_ms);
    }

    pub(crate) fn record_tool_call(&mut self, name: &str, elapsed: Duration) {
        self.tool_calls_total = self.tool_calls_total.saturating_add(1);
        self.tool_latency_ms_total = self
            .tool_latency_ms_total
            .saturating_add(duration_ms(elapsed));
        self.tool_latency_ms_max = self.tool_latency_ms_max.max(duration_ms(elapsed));
        *self.methods.entry(name.to_owned()).or_default() += 1;
    }

    pub(crate) fn record_task_call(&mut self, method: &str, params: &Value, response: &Value) {
        if method == "snapshot" {
            self.standalone_snapshots_total = self.standalone_snapshots_total.saturating_add(1);
        }
        if method == "execute_action"
            && params
                .get("capture_after")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            self.post_action_snapshots_total = self.post_action_snapshots_total.saturating_add(1);
        }
        if method == "execute_action"
            && params
                .get("observation_id")
                .and_then(Value::as_str)
                .is_some_and(|id| self.post_observation_ids.remove(id))
        {
            self.post_snapshot_reuses_total = self.post_snapshot_reuses_total.saturating_add(1);
        }
        if response
            .get("observation_required")
            .and_then(Value::as_bool)
            == Some(true)
        {
            self.observation_required_total = self.observation_required_total.saturating_add(1);
        }
        if let Some(id) = response
            .pointer("/post_snapshot/observation_id")
            .and_then(Value::as_str)
        {
            if self.post_observation_ids.len() == MAX_POST_OBSERVATIONS {
                let oldest = self.post_observation_ids.iter().next().cloned();
                if let Some(oldest) = oldest {
                    self.post_observation_ids.remove(&oldest);
                }
            }
            self.post_observation_ids.insert(id.to_owned());
        }
    }

    pub(crate) fn record_model_usage_message(&mut self, message: &Value) {
        if let Some(usage) = message.pointer("/_meta/dcc-cua/model_usage") {
            self.record_model_usage(usage);
        }
    }

    pub(crate) fn record_model_usage_notification(&mut self, message: &Value) {
        let usage = message.get("params");
        if let Some(usage) = usage {
            self.record_model_usage(usage);
        }
    }

    pub(crate) fn record_model_usage(&mut self, usage: &Value) {
        let Some(usage) = usage.as_object() else {
            return;
        };
        let input = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input == 0 && output == 0 {
            return;
        }
        self.reported_model_calls_total = self.reported_model_calls_total.saturating_add(1);
        self.reported_input_tokens_total = self.reported_input_tokens_total.saturating_add(input);
        self.reported_output_tokens_total =
            self.reported_output_tokens_total.saturating_add(output);
    }

    pub(crate) fn report(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "scope": "mcp_connection",
            "elapsed_ms": duration_ms(self.started.elapsed()),
            "transport": {
                "messages_total": self.transport_messages_total,
                "input_bytes_total": self.transport_input_bytes_total,
                "output_bytes_total": self.transport_output_bytes_total,
                "input_tokens_estimated_total": self.transport_input_tokens_estimated_total,
                "output_tokens_estimated_total": self.transport_output_tokens_estimated_total,
                "latency_samples_total": self.transport_latency_samples_total,
                "latency_ms_total": self.transport_latency_ms_total,
                "latency_ms_max": self.transport_latency_ms_max,
            },
            "tool_calls": {
                "total": self.tool_calls_total,
                "latency_ms_total": self.tool_latency_ms_total,
                "latency_ms_max": self.tool_latency_ms_max,
                "methods": self.methods,
            },
            "observations": {
                "standalone_snapshots_total": self.standalone_snapshots_total,
                "post_action_snapshots_total": self.post_action_snapshots_total,
                "post_snapshot_reuses_total": self.post_snapshot_reuses_total,
                "observation_required_total": self.observation_required_total,
            },
            "model_tokens": {
                "reported_model_calls_total": self.reported_model_calls_total,
                "reported_input_tokens_total": self.reported_input_tokens_total,
                "reported_output_tokens_total": self.reported_output_tokens_total,
                "reported_total_tokens": self.reported_input_tokens_total.saturating_add(self.reported_output_tokens_total),
            },
            "token_accounting": {
                "estimated_protocol_tokens_are_ceil_utf8_bytes_divided_by_four": true,
                "model_tokens_require_agent_host_report": true,
            },
        })
    }

    pub(crate) fn annotate_call_result(
        &self,
        result: &mut Value,
        call_name: &str,
        elapsed: Duration,
    ) {
        result["_meta"] = json!({
            "dcc-cua": {
                "performance": {
                    "schema": SCHEMA,
                    "call": call_name,
                    "elapsed_ms": duration_ms(elapsed),
                    "connection": self.report(),
                }
            }
        });
    }
}

fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).saturating_add(3) / 4
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_separates_protocol_estimates_and_reported_model_usage() {
        let mut metrics = McpPerformance::new();
        metrics.record_transport_input(5);
        metrics.record_transport_output(8);
        metrics.record_model_usage(&json!({"input_tokens": 11, "output_tokens": 7}));

        let report = metrics.report();
        assert_eq!(report["transport"]["input_tokens_estimated_total"], 2);
        assert_eq!(report["transport"]["output_tokens_estimated_total"], 2);
        assert_eq!(report["model_tokens"]["reported_total_tokens"], 18);
        assert_eq!(
            report["token_accounting"]["model_tokens_require_agent_host_report"],
            true
        );
    }

    #[test]
    fn post_snapshot_observation_is_counted_once_when_reused() {
        let mut metrics = McpPerformance::new();
        let response = json!({"post_snapshot":{"observation_id":"obs-2"}});
        metrics.record_task_call("execute_action", &json!({"capture_after":true}), &response);
        metrics.record_task_call(
            "execute_action",
            &json!({"observation_id":"obs-2"}),
            &json!({}),
        );

        let report = metrics.report();
        assert_eq!(report["observations"]["post_action_snapshots_total"], 1);
        assert_eq!(report["observations"]["post_snapshot_reuses_total"], 1);
    }
}
