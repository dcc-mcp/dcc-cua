//! Host-free discovery projections through the SDK's real in-memory envelope channel.
use super::*;
use rstest::rstest;

struct DiscoveryChannel {
    envelope: Value,
    pending: bool,
}

#[async_trait::async_trait]
impl DriverEnvelopeChannel for DiscoveryChannel {
    async fn negotiate(&self) -> Result<DriverChannelCapabilities, String> {
        Ok(DriverChannelCapabilities {
            minimum_envelope_version: DRIVER_ENVELOPE_VERSION,
            maximum_envelope_version: DRIVER_ENVELOPE_VERSION,
            supports_cancellation: true,
        })
    }

    async fn exchange(
        &self,
        request: DriverRequestEnvelope,
    ) -> Result<DriverResponseEnvelope, String> {
        assert!(matches!(
            request.name.as_deref(),
            Some("list_apps" | "list_windows")
        ));
        assert_eq!(request.arguments, Some(json!({})));
        if self.pending {
            std::future::pending::<()>().await;
        }
        Ok(DriverResponseEnvelope {
            envelope_version: DRIVER_ENVELOPE_VERSION,
            request_id: request.request_id,
            ok: true,
            result: Some(self.envelope.clone()),
            error: None,
            error_code: None,
            completion_known: true,
        })
    }

    async fn bind_session(
        &self,
        _options: TrustedSessionOptions,
    ) -> Result<Arc<dyn DriverEnvelopeChannel>, String> {
        panic!("discovery must not create a task session")
    }
    async fn close(&self) -> Result<(), String> {
        Ok(())
    }
    async fn cancel(&self, _request_id: &str) -> Result<(), String> {
        Ok(())
    }
    fn authenticated_principal(&self) -> &str {
        "discovery-fixture"
    }
    fn connection_generation(&self) -> &str {
        "discovery-fixture-generation"
    }
}

fn driver(envelope: Value, pending: bool) -> ComputerUseDriver {
    let raw = CuaDriver::connect_remote(Arc::new(DiscoveryChannel { envelope, pending })).unwrap();
    ComputerUseDriver::from_driver((raw, false))
}

#[rstest]
#[tokio::test]
async fn discovery_reuse_apps_preserves_unknown_fields_and_public_shape() {
    let content = json!({
        "apps":[{"pid":42,"name":"Editor", "future":{"opaque":"未知 🧪", "nullable":null}, "active":true}],
        "provider_extension":{"future": [true, 18446744073709551615_u64]},
    });
    let runtime = driver(
        json!({"isError":false,"content":[{"type":"text","text":"unused envelope text"}],"structuredContent":content}),
        false,
    );
    assert_eq!(runtime.list_apps().await.unwrap(), content);
}

#[rstest]
#[case(json!({}))]
#[case(json!({"apps":"opaque provider payload"}))]
#[case(json!({"apps":null}))]
#[tokio::test]
async fn discovery_reuse_apps_keeps_the_open_object_contract(#[case] content: Value) {
    let runtime = driver(json!({"structuredContent":content}), false);
    assert_eq!(runtime.list_apps().await.unwrap(), content);
}

#[rstest]
#[tokio::test]
async fn discovery_reuse_windows_preserves_opaque_entries_without_typed_validation() {
    // These intentionally include future/partial provider entries that closed
    // typed WindowInfo cannot represent. Native Windows inventory is never invoked.
    let windows = json!([{"window_id":73,"pid":null,"z_index":null,"extension":{"opaque":"a\u{200d}b"}}, "opaque", null]);
    let runtime = driver(
        json!({"structuredContent":{"windows":windows,"provider_extension":true}}),
        false,
    );
    let result = call_driver_tool(
        &runtime.driver,
        "list_windows",
        "{}".into(),
        "list CUA windows",
    )
    .await
    .unwrap();
    ensure_tool_ok("list CUA windows", &result).unwrap();
    assert_eq!(
        sdk_window_entries(&result).unwrap(),
        windows.as_array().unwrap().clone()
    );
}

#[rstest]
#[case(Value::Null)]
#[case(json!([]))]
#[case(json!("opaque"))]
#[tokio::test]
async fn discovery_reuse_apps_preserves_missing_object_error(#[case] structured: Value) {
    let runtime = driver(json!({"structuredContent":structured}), false);
    let error = runtime.list_apps().await.unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    assert_eq!(error.message, "CUA list_apps omitted structuredContent");
}

#[rstest]
#[case(json!({}))]
#[case(json!({"windows":42}))]
#[tokio::test]
async fn discovery_reuse_windows_preserves_missing_array_error(#[case] structured: Value) {
    let runtime = driver(json!({"structuredContent":structured}), false);
    let result = call_driver_tool(
        &runtime.driver,
        "list_windows",
        "{}".into(),
        "list CUA windows",
    )
    .await
    .unwrap();
    let error = sdk_window_entries(&result).unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
    assert_eq!(
        error.message,
        "CUA list_windows omitted structuredContent.windows"
    );
}

#[rstest]
#[tokio::test]
async fn discovery_reuse_preserves_refusal_text_and_error_code() {
    let runtime = driver(
        json!({"isError":true,"content":[{"type":"text","text":"fixture refusal"}],"structuredContent":{"code":"synthetic_refusal","message":"structured refusal"}}),
        false,
    );
    let error = runtime.list_apps().await.unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert_eq!(error.message, "call CUA list_apps: fixture refusal");
    assert!(error.details.is_none());
}

#[rstest]
#[tokio::test(start_paused = true)]
async fn discovery_reuse_preserves_timeout_details_without_native_work() {
    let runtime = driver(json!({}), true);
    let error = runtime.list_apps().await.unwrap_err();
    assert_eq!(error.code, ComputerUseErrorCode::InputFailed);
    assert_eq!(error.details.unwrap().timed_out, Some(true));
    assert!(
        error
            .message
            .starts_with("CUA call CUA list_apps timed out after ")
    );
}
