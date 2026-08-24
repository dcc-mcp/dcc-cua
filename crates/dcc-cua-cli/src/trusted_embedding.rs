//! Fail-closed authentication for the process that embeds the MCP Apps bridge.
//!
//! MCP Apps visibility metadata controls which tools a host presents to the
//! model, but it is not a cryptographic invocation-origin credential. The
//! authorization issuer therefore exists only when the bridge was launched
//! directly by the packaged Codex desktop process.

use thiserror::Error;

#[cfg(any(windows, test))]
const TRUSTED_CODEX_PACKAGE_FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";
#[cfg(any(windows, test))]
const TRUSTED_CODEX_EXECUTABLE: &str = "codex.exe";
#[cfg(any(windows, test))]
const TRUSTED_CLAUDE_PACKAGE_FAMILY: &str = "Claude_pzs8sxrjxfjjc";
#[cfg(any(windows, test))]
const TRUSTED_CLAUDE_EXECUTABLE: &str = "claude.exe";
#[cfg(any(windows, test))]
const TRUSTED_TENCENT_PUBLISHER: &str = "Tencent Technology (Shenzhen) Company Limited";
#[cfg(any(windows, test))]
const TRUSTED_CODEBUDDY_EXECUTABLE: &str = "CodeBuddy CN.exe";
#[cfg(any(windows, test))]
const TRUSTED_CODEBUDDY_PRODUCT: &str = "CodeBuddy CN";
#[cfg(any(windows, test))]
const TRUSTED_WORKBUDDY_EXECUTABLE: &str = "WorkBuddy.exe";
#[cfg(any(windows, test))]
const TRUSTED_WORKBUDDY_PRODUCT: &str = "WorkBuddy";

#[derive(Clone, Copy, Debug)]
pub(super) struct TrustedEmbeddingAttestation {
    label: &'static str,
}

impl TrustedEmbeddingAttestation {
    pub(super) fn label(self) -> &'static str {
        self.label
    }
}

#[derive(Debug, Error)]
#[error("trusted embedding unavailable: {reason}")]
pub(super) struct TrustedEmbeddingError {
    reason: String,
}

impl TrustedEmbeddingError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[cfg(any(windows, test))]
pub(super) fn validate_codex_identity(
    executable_name: &str,
    package_family: &str,
) -> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
    if !executable_name.eq_ignore_ascii_case(TRUSTED_CODEX_EXECUTABLE) {
        return Err(TrustedEmbeddingError::new(
            "the immediate parent is not the Codex desktop runtime",
        ));
    }
    if package_family != TRUSTED_CODEX_PACKAGE_FAMILY {
        return Err(TrustedEmbeddingError::new(
            "the immediate parent does not have the trusted Codex package identity",
        ));
    }
    Ok(TrustedEmbeddingAttestation {
        label: "codex_desktop_windows_package",
    })
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug)]
pub(super) struct VerifiedAuthenticodeIdentity<'a> {
    product_name: &'a str,
    publisher: &'a str,
}

#[cfg(any(windows, test))]
pub(super) fn validate_packaged_identity(
    executable_name: &str,
    package_family: &str,
) -> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
    if executable_name.eq_ignore_ascii_case(TRUSTED_CODEX_EXECUTABLE) {
        return validate_codex_identity(executable_name, package_family);
    }
    if executable_name.eq_ignore_ascii_case(TRUSTED_CLAUDE_EXECUTABLE) {
        if package_family != TRUSTED_CLAUDE_PACKAGE_FAMILY {
            return Err(TrustedEmbeddingError::new(
                "the immediate parent does not have the trusted Claude package identity",
            ));
        }
        return Ok(TrustedEmbeddingAttestation {
            label: "claude_desktop_windows_package",
        });
    }
    Err(TrustedEmbeddingError::new(
        "the immediate parent is not a supported packaged desktop runtime",
    ))
}

#[cfg(any(windows, test))]
pub(super) fn validate_authenticode_identity(
    executable_name: &str,
    product_name: &str,
    publisher: &str,
) -> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
    if publisher != TRUSTED_TENCENT_PUBLISHER {
        return Err(TrustedEmbeddingError::new(
            "the embedding executable publisher is not trusted",
        ));
    }
    if executable_name.eq_ignore_ascii_case(TRUSTED_CODEBUDDY_EXECUTABLE)
        && product_name == TRUSTED_CODEBUDDY_PRODUCT
    {
        return Ok(TrustedEmbeddingAttestation {
            label: "codebuddy_cn_desktop_windows_authenticode",
        });
    }
    if executable_name.eq_ignore_ascii_case(TRUSTED_WORKBUDDY_EXECUTABLE)
        && product_name == TRUSTED_WORKBUDDY_PRODUCT
    {
        return Ok(TrustedEmbeddingAttestation {
            label: "workbuddy_desktop_windows_authenticode",
        });
    }
    Err(TrustedEmbeddingError::new(
        "the signed embedding executable identity is not supported",
    ))
}

#[cfg(any(windows, test))]
pub(super) fn validate_observed_identity(
    executable_name: &str,
    package_family: Option<&str>,
    authenticode: Option<VerifiedAuthenticodeIdentity<'_>>,
) -> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
    if let Some(package_family) = package_family {
        return validate_packaged_identity(executable_name, package_family);
    }
    let authenticode = authenticode.ok_or_else(|| {
        TrustedEmbeddingError::new(
            "the embedding parent has no verifiable package or Authenticode identity",
        )
    })?;
    validate_authenticode_identity(
        executable_name,
        authenticode.product_name,
        authenticode.publisher,
    )
}

#[cfg(windows)]
pub(super) fn verify_trusted_embedding_parent()
-> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
    windows::verify_parent()
}

#[cfg(not(windows))]
pub(super) fn verify_trusted_embedding_parent()
-> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
    Err(TrustedEmbeddingError::new(
        "this platform has no verified DCC-CUA embedding attestor",
    ))
}

#[cfg(windows)]
mod windows {
    mod authenticode;

    use std::mem::size_of;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        APPMODEL_ERROR_NO_PACKAGE, CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    use super::{
        TrustedEmbeddingAttestation, TrustedEmbeddingError, VerifiedAuthenticodeIdentity,
        validate_observed_identity,
    };

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: OwnedHandle is constructed only from a successful Win32
            // handle-returning call and is neither copied nor manually closed.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    pub(super) fn verify_parent() -> Result<TrustedEmbeddingAttestation, TrustedEmbeddingError> {
        let parent_pid = immediate_parent_pid(std::process::id())?;
        let parent = open_process(parent_pid)?;
        let executable_path = process_image_path(parent.0)?;
        let executable_name = Path::new(&executable_path)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| TrustedEmbeddingError::new("parent executable name is not Unicode"))?;
        let package_family = package_family_name(parent.0)?;
        let authenticode = if package_family.is_none() {
            Some(authenticode::verify(&executable_path)?)
        } else {
            None
        };
        validate_observed_identity(
            executable_name,
            package_family.as_deref(),
            authenticode
                .as_ref()
                .map(|identity| VerifiedAuthenticodeIdentity {
                    product_name: &identity.product_name,
                    publisher: &identity.publisher,
                }),
        )
    }

    fn immediate_parent_pid(process_id: u32) -> Result<u32, TrustedEmbeddingError> {
        // SAFETY: The returned snapshot handle is checked before it is wrapped.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(last_os_error(
                "could not inspect the embedding process tree",
            ));
        }
        let snapshot = OwnedHandle(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: entry has the required size and remains valid for enumeration.
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err(last_os_error(
                "could not enumerate the embedding process tree",
            ));
        }
        loop {
            if entry.th32ProcessID == process_id {
                return (entry.th32ParentProcessID != 0)
                    .then_some(entry.th32ParentProcessID)
                    .ok_or_else(|| TrustedEmbeddingError::new("embedding parent was not found"));
            }
            // SAFETY: snapshot and entry remain valid until enumeration finishes.
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
        Err(TrustedEmbeddingError::new(
            "current process was not found in the process snapshot",
        ))
    }

    fn open_process(process_id: u32) -> Result<OwnedHandle, TrustedEmbeddingError> {
        // SAFETY: OpenProcess is called with query-only rights and a concrete PID.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if handle.is_null() {
            return Err(last_os_error("could not open the embedding parent process"));
        }
        Ok(OwnedHandle(handle))
    }

    fn process_image_path(handle: HANDLE) -> Result<String, TrustedEmbeddingError> {
        let mut buffer = vec![0u16; 32_768];
        let mut length = u32::try_from(buffer.len()).expect("process path buffer fits in u32");
        // SAFETY: handle is open with query rights and buffer/length describe valid storage.
        if unsafe {
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length)
        } == 0
        {
            return Err(last_os_error(
                "could not read the embedding executable path",
            ));
        }
        buffer.truncate(length as usize);
        String::from_utf16(&buffer)
            .map_err(|_| TrustedEmbeddingError::new("embedding executable path is not UTF-16"))
    }

    fn package_family_name(handle: HANDLE) -> Result<Option<String>, TrustedEmbeddingError> {
        let mut length = 0u32;
        // SAFETY: The documented sizing call accepts a null output buffer.
        let sizing = unsafe { GetPackageFamilyName(handle, &mut length, ptr::null_mut()) };
        if sizing == APPMODEL_ERROR_NO_PACKAGE {
            return Ok(None);
        }
        if sizing != ERROR_INSUFFICIENT_BUFFER || length == 0 {
            return Err(TrustedEmbeddingError::new(
                "could not inspect the embedding parent package identity",
            ));
        }
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: buffer has the exact size requested by the sizing call.
        let result = unsafe { GetPackageFamilyName(handle, &mut length, buffer.as_mut_ptr()) };
        if result != ERROR_SUCCESS {
            return Err(TrustedEmbeddingError::new(
                "could not verify the embedding parent package identity",
            ));
        }
        let content_length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16(&buffer[..content_length])
            .map(Some)
            .map_err(|_| TrustedEmbeddingError::new("package family name is not UTF-16"))
    }

    fn last_os_error(context: &str) -> TrustedEmbeddingError {
        TrustedEmbeddingError::new(format!("{context}: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(test)]
mod tests;
