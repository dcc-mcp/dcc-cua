use super::*;
use rstest::rstest;

#[rstest]
fn report_separates_protocol_estimates_and_reported_model_usage() {
    let mut metrics = McpPerformance::new();
    metrics.record_transport_input(5);
    metrics.record_transport_output(8, &Value::Null);
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

#[rstest]
fn batched_responses_attribute_only_native_image_data_to_images() {
    let response = json!([
        {"result":{"content":[{"type":"image","data":"QUJD"},{"type":"text","text":"tree"}]}},
        {"result":{"content":[{"type":"image","data":"REVG"}]}},
        {"error":{"message":"failed"}}
    ]);
    let bytes = response.to_string().len() + 1;
    let mut metrics = McpPerformance::new();
    metrics.record_transport_output(bytes, &response);
    let transport = &metrics.report()["transport"];
    assert_eq!(transport["output_native_images_total"], 2);
    assert_eq!(transport["output_image_base64_bytes_total"], 8);
    assert_eq!(transport["output_other_bytes_total"], bytes - 8);
}

#[rstest]
fn host_measurements_keep_unreported_samples_out_of_rate_denominators() {
    let mut metrics = McpPerformance::new();
    assert!(metrics.report()["agent_tasks"]["success_rate"].is_null());
    assert_eq!(metrics.report()["model_tokens"]["available"], false);
    metrics.record_model_usage_message(&json!({"params":{"_meta":{"dcc-cua":{
        "model_usage":{"input_tokens":20,"cached_input_tokens":5,"model_elapsed_ms":120}
    }}}}));
    metrics.record_model_usage(&json!({"input_tokens":80}));
    metrics.record_task_outcome(&json!({"success":true,"elapsed_ms":400}));
    metrics.record_task_outcome(&json!({"success":false,"elapsed_ms":170}));
    metrics.record_task_outcome(&json!({"elapsed_ms":30}));
    let report = metrics.report();
    assert_eq!(report["model_tokens"]["cached_input_fraction"], 0.25);
    assert_eq!(report["model_tokens"]["model_elapsed_ms_total"], 120);
    assert_eq!(report["model_tokens"]["cache_reported_calls"], 1);
    assert_eq!(report["model_tokens"]["cache_hit_rate"], 1.0);
    assert_eq!(report["agent_tasks"]["success_rate"], 0.5);
    assert_eq!(report["agent_tasks"]["elapsed_ms_total"], 570);
}

#[rstest]
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
