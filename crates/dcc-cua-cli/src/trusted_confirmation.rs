use std::sync::Arc;

#[cfg(windows)]
use async_trait::async_trait;
use dcc_cua_host::TrustedActionConfirmationHost;
#[cfg(any(windows, test))]
use dcc_cua_host::TrustedActionConfirmationRequest;
#[cfg(windows)]
use dcc_cua_host::{
    TrustedActionConfirmationAction, TrustedActionConfirmationDecision,
    TrustedActionConfirmationHostError,
};

pub(crate) fn native_confirmation_host() -> Option<Arc<dyn TrustedActionConfirmationHost>> {
    #[cfg(windows)]
    {
        Some(Arc::new(NativeUserConfirmationHost {
            prompt_turn: tokio::sync::Mutex::new(()),
        }))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(any(windows, test))]
pub(crate) fn prompt_text(request: &TrustedActionConfirmationRequest) -> String {
    let action = request.action["action"].as_str().unwrap_or("unknown");
    let target = match (request.target_process_id, request.target_window_handle) {
        (Some(process_id), Some(window_handle)) => {
            format!("PID: {process_id}\nHWND: {window_handle:#x}")
        }
        _ => "Target: granted desktop scope".to_owned(),
    };
    format!(
        "DCC-CUA requests one action on the exact observed target.\n\n{target}\nAction: {action}\nDigest: {}\n\nInput text and secrets are intentionally hidden. Approve this action once?",
        request.request_digest
    )
}

#[cfg(windows)]
struct NativeUserConfirmationHost {
    prompt_turn: tokio::sync::Mutex<()>,
}

#[cfg(windows)]
#[async_trait]
impl TrustedActionConfirmationHost for NativeUserConfirmationHost {
    async fn confirm(
        &self,
        request: TrustedActionConfirmationRequest,
    ) -> Result<TrustedActionConfirmationDecision, TrustedActionConfirmationHostError> {
        let _prompt_turn = self.prompt_turn.lock().await;
        let request_digest = request.request_digest.clone();
        let action = tokio::task::spawn_blocking(move || show_native_prompt(&request))
            .await
            .map_err(|error| TrustedActionConfirmationHostError {
                reason: format!("native user confirmation worker failed: {error}"),
            })?;
        Ok(TrustedActionConfirmationDecision {
            action,
            request_digest,
        })
    }
}

#[cfg(windows)]
fn show_native_prompt(
    request: &TrustedActionConfirmationRequest,
) -> TrustedActionConfirmationAction {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDCANCEL, IDNO, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_TOPMOST,
        MB_YESNOCANCEL, MessageBoxW,
    };

    let body = prompt_text(request)
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let title = "DCC-CUA user authorization"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_YESNOCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    match result {
        IDYES => TrustedActionConfirmationAction::Allow,
        IDNO => TrustedActionConfirmationAction::Deny,
        IDCANCEL | 0 => TrustedActionConfirmationAction::Cancel,
        _ => TrustedActionConfirmationAction::Cancel,
    }
}
