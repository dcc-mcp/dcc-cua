//! Pure closed-scope validation; native source resolution belongs to the issuer.
use crate::TrustedTaskActionScope;
use dcc_cua_protocol::recording_audio::ProcessAudioGrant;

pub(crate) fn valid_scope(
    grant: Option<&ProcessAudioGrant>,
    pid: u32,
    hwnd: u64,
    directory: Option<&str>,
    actions: &[TrustedTaskActionScope],
    methods: &[String],
) -> bool {
    let requested = actions
        .iter()
        .any(TrustedTaskActionScope::is_recording_audio);
    requested == grant.is_some()
        && grant.is_none_or(|grant| {
            grant.validate().is_ok()
                && grant.target.process_id == pid
                && grant.target.window_handle == hwnd
                && directory.is_some()
                && ["recording_start", "recording_state", "recording_stop"]
                    .iter()
                    .all(|required| methods.iter().any(|method| method == required))
        })
}

#[cfg(test)]
mod tests;
