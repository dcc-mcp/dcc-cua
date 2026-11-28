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
    output_image_base64_bytes: u64,
    output_images: u64,
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
    cached_input_tokens: u64,
    cache_measured_input_tokens: u64,
    cache_samples: u64,
    cache_hits: u64,
    model_elapsed_ms: u64,
    model_latency_samples: u64,
    task_outcomes: u64,
    tasks_succeeded: u64,
    task_wall_ms: u64,
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
            output_image_base64_bytes: 0,
            output_images: 0,
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
            cached_input_tokens: 0,
            cache_measured_input_tokens: 0,
            cache_samples: 0,
            cache_hits: 0,
            model_elapsed_ms: 0,
            model_latency_samples: 0,
            task_outcomes: 0,
            tasks_succeeded: 0,
            task_wall_ms: 0,
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

    pub(crate) fn record_transport_output(&mut self, bytes: usize, response: &Value) {
        self.transport_output_bytes_total = self
            .transport_output_bytes_total
            .saturating_add(bytes as u64);
        self.transport_output_tokens_estimated_total = self
            .transport_output_tokens_estimated_total
            .saturating_add(estimate_tokens(bytes));
        let replies = response
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(std::slice::from_ref(response));
        for reply in replies {
            if let Some(blocks) = reply.pointer("/result/content").and_then(Value::as_array) {
                for block in blocks {
                    if block["type"] == "image"
                        && let Some(data) = block["data"].as_str()
                    {
                        self.output_images = self.output_images.saturating_add(1);
                        self.output_image_base64_bytes = self
                            .output_image_base64_bytes
                            .saturating_add(data.len() as u64);
                    }
                }
            }
        }
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
                .get("observation_id")
                .and_then(Value::as_str)
                .is_some_and(|id| self.post_observation_ids.remove(id))
        {
            self.post_snapshot_reuses_total = self.post_snapshot_reuses_total.saturating_add(1);
        }
        self.record_observation_required(response);
        if let Some(id) = response
            .pointer("/post_snapshot/observation_id")
            .and_then(Value::as_str)
        {
            self.post_action_snapshots_total = self.post_action_snapshots_total.saturating_add(1);
            if self.post_observation_ids.len() == MAX_POST_OBSERVATIONS {
                let oldest = self.post_observation_ids.iter().next().cloned();
                if let Some(oldest) = oldest {
                    self.post_observation_ids.remove(&oldest);
                }
            }
            self.post_observation_ids.insert(id.to_owned());
        }
    }

    pub(crate) fn record_observation_required(&mut self, response: &Value) {
        if response["observation_required"] == true
            || response.pointer("/details/fresh_observation_required") == Some(&Value::Bool(true))
        {
            self.observation_required_total = self.observation_required_total.saturating_add(1);
        }
    }

    pub(crate) fn record_model_usage_message(&mut self, message: &Value) {
        if let Some(usage) = message
            .pointer("/params/_meta/dcc-cua/model_usage")
            .or_else(|| message.pointer("/_meta/dcc-cua/model_usage"))
        {
            self.record_model_usage(usage);
        }
    }

    pub(crate) fn record_model_usage_notification(&mut self, message: &Value) {
        let usage = message.get("params");
        if let Some(usage) = usage {
            self.record_model_usage(usage);
        }
    }

    pub(crate) fn record_model_usage(&mut self, usage: &Value) -> bool {
        let Some(usage) = usage.as_object() else {
            return false;
        };
        let count = |name: &str| usage.get(name).and_then(Value::as_u64);
        let input = count("input_tokens").or_else(|| count("prompt_tokens"));
        let output = count("output_tokens").or_else(|| count("completion_tokens"));
        if input.is_none() && output.is_none() {
            return false;
        }
        self.reported_model_calls_total = self.reported_model_calls_total.saturating_add(1);
        self.reported_input_tokens_total = self
            .reported_input_tokens_total
            .saturating_add(input.unwrap_or(0));
        self.reported_output_tokens_total = self
            .reported_output_tokens_total
            .saturating_add(output.unwrap_or(0));
        if let (Some(input), Some(cached)) = (input, count("cached_input_tokens"))
            && cached <= input
        {
            self.cache_samples = self.cache_samples.saturating_add(1);
            self.cache_hits = self.cache_hits.saturating_add(u64::from(cached > 0));
            self.cached_input_tokens = self.cached_input_tokens.saturating_add(cached);
            self.cache_measured_input_tokens =
                self.cache_measured_input_tokens.saturating_add(input);
        }
        if let Some(ms) = usage.get("model_elapsed_ms").and_then(Value::as_u64) {
            self.model_latency_samples = self.model_latency_samples.saturating_add(1);
            self.model_elapsed_ms = self.model_elapsed_ms.saturating_add(ms);
        }
        true
    }

    pub(crate) fn record_task_outcome(&mut self, params: &Value) {
        if let (Some(success), Some(ms)) =
            (params["success"].as_bool(), params["elapsed_ms"].as_u64())
        {
            self.task_outcomes = self.task_outcomes.saturating_add(1);
            self.tasks_succeeded = self.tasks_succeeded.saturating_add(u64::from(success));
            self.task_wall_ms = self.task_wall_ms.saturating_add(ms);
        }
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
                "output_native_images_total": self.output_images,
                "output_image_base64_bytes_total": self.output_image_base64_bytes,
                "output_other_bytes_total": self.transport_output_bytes_total.saturating_sub(self.output_image_base64_bytes),
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
                "available": self.reported_model_calls_total > 0,
                "cached_input_tokens_total": self.cached_input_tokens,
                "cache_reported_calls": self.cache_samples,
                "cache_hit_calls": self.cache_hits,
                "cache_hit_rate": fraction(self.cache_hits, self.cache_samples),
                "cache_measured_input_tokens_total": self.cache_measured_input_tokens,
                "cached_input_fraction": fraction(self.cached_input_tokens, self.cache_measured_input_tokens),
                "model_latency_samples": self.model_latency_samples,
                "model_elapsed_ms_total": self.model_elapsed_ms,
                "reported_model_calls_total": self.reported_model_calls_total,
                "reported_input_tokens_total": self.reported_input_tokens_total,
                "reported_output_tokens_total": self.reported_output_tokens_total,
                "reported_total_tokens": self.reported_input_tokens_total.saturating_add(self.reported_output_tokens_total),
            },
            "agent_tasks": {
                "reported_outcomes": self.task_outcomes,
                "succeeded": self.tasks_succeeded,
                "success_rate": fraction(self.tasks_succeeded, self.task_outcomes),
                "elapsed_ms_total": self.task_wall_ms,
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

fn fraction(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
