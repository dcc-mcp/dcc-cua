use std::ffi::{OsStr, c_void};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW,
};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_SAFER_FLAG,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTD_UICONTEXT_EXECUTE,
    WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData,
    WinVerifyTrustEx,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

use super::super::TrustedEmbeddingError;

pub(super) struct VerifiedExecutableIdentity {
    pub(super) product_name: String,
    pub(super) publisher: String,
}

pub(super) fn verify(
    executable_path: &str,
) -> Result<VerifiedExecutableIdentity, TrustedEmbeddingError> {
    let path = wide(executable_path);
    let publisher = verify_publisher(&path)?;
    let product_name = product_name(&path)?;
    Ok(VerifiedExecutableIdentity {
        product_name,
        publisher,
    })
}

fn verify_publisher(path: &[u16]) -> Result<String, TrustedEmbeddingError> {
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        hFile: ptr::null_mut(),
        pgKnownSubject: ptr::null_mut(),
    };
    let mut trust_data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_SAFER_FLAG,
        dwUIContext: WTD_UICONTEXT_EXECUTE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: all structures have their documented sizes, point to live storage,
    // request no UI, and the returned state is closed before this function exits.
    let status = unsafe { WinVerifyTrustEx(ptr::null_mut(), &mut action, &mut trust_data) };
    if status != 0 {
        close_trust_state(&mut action, &mut trust_data);
        return Err(TrustedEmbeddingError::new(format!(
            "the embedding executable does not have a valid Authenticode trust chain (status 0x{:08X})",
            status as u32
        )));
    }
    let publisher = signer_display_name(&trust_data);
    close_trust_state(&mut action, &mut trust_data);
    publisher
}

fn signer_display_name(trust_data: &WINTRUST_DATA) -> Result<String, TrustedEmbeddingError> {
    // SAFETY: the helpers are called only while a successful WinVerifyTrust state is live.
    let provider = unsafe { WTHelperProvDataFromStateData(trust_data.hWVTStateData) };
    if provider.is_null() {
        return Err(TrustedEmbeddingError::new(
            "the Authenticode provider did not expose signer data",
        ));
    }
    // SAFETY: provider was returned for the live trust state; index zero is the primary signer.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
    if signer.is_null() {
        return Err(TrustedEmbeddingError::new(
            "the Authenticode signature has no primary signer",
        ));
    }
    // SAFETY: signer is live and index zero is its leaf signing certificate.
    let certificate = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if certificate.is_null() {
        return Err(TrustedEmbeddingError::new(
            "the Authenticode signer has no leaf certificate",
        ));
    }
    // SAFETY: certificate points to a provider-owned value for the live trust state.
    let context = unsafe { (*certificate).pCert };
    certificate_display_name(context)
}

fn certificate_display_name(context: *const CERT_CONTEXT) -> Result<String, TrustedEmbeddingError> {
    if context.is_null() {
        return Err(TrustedEmbeddingError::new(
            "the Authenticode leaf certificate is unavailable",
        ));
    }
    // SAFETY: context is a live leaf certificate; the sizing call accepts a null buffer.
    let length = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            ptr::null(),
            ptr::null_mut(),
            0,
        )
    };
    if length <= 1 {
        return Err(TrustedEmbeddingError::new(
            "the Authenticode signer publisher name is unavailable",
        ));
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: buffer has the exact character capacity returned by the sizing call.
    let written = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            ptr::null(),
            buffer.as_mut_ptr(),
            length,
        )
    };
    if written != length {
        return Err(TrustedEmbeddingError::new(
            "could not read the Authenticode signer publisher name",
        ));
    }
    String::from_utf16(&buffer[..buffer.len() - 1])
        .map_err(|_| TrustedEmbeddingError::new("publisher name is not UTF-16"))
}

fn close_trust_state(action: &mut windows_sys::core::GUID, trust_data: &mut WINTRUST_DATA) {
    trust_data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: this closes the state created by the matching WinVerifyTrustEx call.
    unsafe {
        let _ = WinVerifyTrustEx(ptr::null_mut(), action, trust_data);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Translation {
    language: u16,
    code_page: u16,
}

fn product_name(path: &[u16]) -> Result<String, TrustedEmbeddingError> {
    let mut ignored = 0u32;
    // SAFETY: path is a live, null-terminated UTF-16 executable path.
    let byte_length = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored) };
    if byte_length == 0 {
        return Err(last_os_error(
            "the signed embedding executable has no version metadata",
        ));
    }
    let word_size = size_of::<usize>();
    let mut storage = vec![0usize; (byte_length as usize).div_ceil(word_size)];
    // SAFETY: storage has at least byte_length bytes and remains live for all queries below.
    if unsafe {
        GetFileVersionInfoW(
            path.as_ptr(),
            0,
            byte_length,
            storage.as_mut_ptr().cast::<c_void>(),
        )
    } == 0
    {
        return Err(last_os_error(
            "could not read the signed embedding executable metadata",
        ));
    }
    let data = storage.as_ptr().cast::<c_void>();
    let translations = version_translations(data)?;
    for translation in translations {
        let query = wide(format!(
            "\\StringFileInfo\\{:04X}{:04X}\\ProductName",
            translation.language, translation.code_page
        ));
        if let Some(value) = version_string(data, &query) {
            return Ok(value);
        }
    }
    Err(TrustedEmbeddingError::new(
        "the signed embedding executable has no product name",
    ))
}

fn version_translations(data: *const c_void) -> Result<Vec<Translation>, TrustedEmbeddingError> {
    let query = wide("\\VarFileInfo\\Translation");
    let mut value = ptr::null_mut::<c_void>();
    let mut byte_length = 0u32;
    // SAFETY: data contains a live version resource and output pointers are valid.
    if unsafe { VerQueryValueW(data, query.as_ptr(), &mut value, &mut byte_length) } == 0
        || value.is_null()
        || byte_length < size_of::<Translation>() as u32
    {
        return Err(TrustedEmbeddingError::new(
            "the signed embedding executable has no version translation",
        ));
    }
    let count = byte_length as usize / size_of::<Translation>();
    // SAFETY: VerQueryValueW returned count complete Translation records in the live buffer.
    Ok(unsafe { std::slice::from_raw_parts(value.cast::<Translation>(), count) }.to_vec())
}

fn version_string(data: *const c_void, query: &[u16]) -> Option<String> {
    let mut value = ptr::null_mut::<c_void>();
    let mut char_length = 0u32;
    // SAFETY: data and query are live; output pointers are valid for this read-only query.
    if unsafe { VerQueryValueW(data, query.as_ptr(), &mut value, &mut char_length) } == 0
        || value.is_null()
        || char_length <= 1
    {
        return None;
    }
    // SAFETY: VerQueryValueW returned char_length UTF-16 units containing the value.
    let units = unsafe { std::slice::from_raw_parts(value.cast::<u16>(), char_length as usize) };
    let content_length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16(&units[..content_length]).ok()
}

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

fn last_os_error(context: &str) -> TrustedEmbeddingError {
    TrustedEmbeddingError::new(format!("{context}: {}", std::io::Error::last_os_error()))
}

#[cfg(test)]
mod tests;
