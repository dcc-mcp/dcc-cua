use std::sync::Arc;

pub(crate) struct TaskAuthorizationConfirmationRequest {
    #[cfg(any(windows, test))]
    pub(crate) owner_window_handle: u64,
    #[cfg(any(windows, test))]
    pub(crate) message: String,
}

impl TaskAuthorizationConfirmationRequest {
    pub(crate) fn new(owner_window_handle: u64, message: String) -> Self {
        #[cfg(any(windows, test))]
        {
            Self {
                owner_window_handle,
                message,
            }
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = (owner_window_handle, message);
            Self {}
        }
    }
}

pub(crate) trait TaskAuthorizationConfirmationHost: Send + Sync {
    fn method(&self) -> &'static str;
    fn verify(&self, request: TaskAuthorizationConfirmationRequest) -> Result<(), String>;
}

pub(crate) fn native_task_authorization_confirmation_host()
-> Option<Arc<dyn TaskAuthorizationConfirmationHost>> {
    #[cfg(windows)]
    {
        use windows::Security::Credentials::UI::{
            UserConsentVerifier, UserConsentVerifierAvailability,
        };

        let windows_hello_available = UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|operation| operation.get())
            .is_ok_and(|availability| availability == UserConsentVerifierAvailability::Available);
        Some(if windows_hello_available {
            Arc::new(WindowsUserConsentConfirmationHost)
                as Arc<dyn TaskAuthorizationConfirmationHost>
        } else {
            Arc::new(WindowsPhysicalPresenceConfirmationHost)
                as Arc<dyn TaskAuthorizationConfirmationHost>
        })
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
struct WindowsUserConsentConfirmationHost;

#[cfg(windows)]
impl TaskAuthorizationConfirmationHost for WindowsUserConsentConfirmationHost {
    fn method(&self) -> &'static str {
        "windows_user_consent_verifier"
    }

    fn verify(&self, request: TaskAuthorizationConfirmationRequest) -> Result<(), String> {
        use windows::Security::Credentials::UI::{
            UserConsentVerificationResult, UserConsentVerifier,
        };
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
        use windows::core::{HSTRING, factory};

        let owner_window_handle = if request.owner_window_handle == 0 {
            (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as usize })
                as u64
        } else {
            request.owner_window_handle
        };
        if owner_window_handle == 0 {
            return Err("trusted user verification requires an active owner HWND".into());
        }
        let owner = HWND(owner_window_handle as usize as *mut _);
        let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(|_| "Windows user verification is unavailable".to_owned())?;
        let operation: windows_future::IAsyncOperation<UserConsentVerificationResult> = unsafe {
            interop.RequestVerificationForWindowAsync(owner, &HSTRING::from(request.message))
        }
        .map_err(|_| "Windows user verification could not start".to_owned())?;
        match operation.get() {
            Ok(result) if result == UserConsentVerificationResult::Verified => Ok(()),
            Ok(result) if result == UserConsentVerificationResult::Canceled => {
                Err("Windows user verification was cancelled".into())
            }
            Ok(_) => Err("Windows user verification was not approved".into()),
            Err(_) => Err("Windows user verification failed".into()),
        }
    }
}

#[cfg(windows)]
struct WindowsPhysicalPresenceConfirmationHost;

#[cfg(windows)]
impl TaskAuthorizationConfirmationHost for WindowsPhysicalPresenceConfirmationHost {
    fn method(&self) -> &'static str {
        "windows_non_injected_keyboard_sequence"
    }

    fn verify(&self, request: TaskAuthorizationConfirmationRequest) -> Result<(), String> {
        verify_non_injected_keyboard_sequence(request)
    }
}

#[cfg(windows)]
struct PhysicalPresenceState {
    next_key: usize,
    verified: bool,
}

#[cfg(windows)]
thread_local! {
    static PHYSICAL_PRESENCE_STATE: std::cell::Cell<*mut PhysicalPresenceState> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
}

#[cfg(windows)]
pub(crate) const PHYSICAL_CONFIRMATION_KEYS: [u32; 3] = [0x7b, 0x7a, 0x79]; // F12, F11, F10

#[cfg(windows)]
unsafe extern "system" fn physical_presence_keyboard_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, PostMessageW, WM_CLOSE, WM_KEYDOWN,
        WM_SYSKEYDOWN,
    };

    if code == HC_ACTION as i32 && matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let keyboard = unsafe { (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref() };
        if let Some(keyboard) = keyboard {
            let consumed = PHYSICAL_PRESENCE_STATE.with(|slot| {
                let state = slot.get();
                if state.is_null() {
                    return false;
                }
                let state = unsafe { &mut *state };
                let (next_key, consumed) = physical_confirmation_transition(
                    state.next_key,
                    keyboard.vkCode,
                    keyboard.flags.0,
                );
                state.next_key = next_key;
                if state.next_key == PHYSICAL_CONFIRMATION_KEYS.len() {
                    state.verified = true;
                    let window = unsafe { GetActiveWindow() };
                    if !window.0.is_null() {
                        let _ = unsafe {
                            PostMessageW(
                                Some(window),
                                WM_CLOSE,
                                windows::Win32::Foundation::WPARAM(0),
                                windows::Win32::Foundation::LPARAM(0),
                            )
                        };
                    }
                }
                consumed
            });
            if consumed {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(windows)]
fn verify_non_injected_keyboard_sequence(
    request: TaskAuthorizationConfirmationRequest,
) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
        SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    };
    use windows::core::{HSTRING, PCWSTR};

    let owner_window_handle = if request.owner_window_handle == 0 {
        (unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as usize })
            as u64
    } else {
        request.owner_window_handle
    };
    if owner_window_handle == 0 {
        return Err("trusted user verification requires an active owner HWND".into());
    }
    let owner = HWND(owner_window_handle as usize as *mut _);
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(physical_presence_keyboard_hook),
            None,
            0,
        )
    }
    .map_err(|_| "could not install the protected physical-input verifier".to_owned())?;
    let mut state = PhysicalPresenceState {
        next_key: 0,
        verified: false,
    };
    PHYSICAL_PRESENCE_STATE.with(|slot| slot.set(&raw mut state));
    let body = HSTRING::from(format!(
        "{}\n\nTo authorize, press F12, then F11, then F10 on a physical keyboard.\n\nInjected or automated keystrokes are rejected. OK/Cancel clicks never authorize.",
        request.message
    ));
    let title = HSTRING::from("DCC-CUA protected task authorization");
    let _ = unsafe {
        MessageBoxW(
            Some(owner),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    PHYSICAL_PRESENCE_STATE.with(|slot| slot.set(std::ptr::null_mut()));
    let _ = unsafe { UnhookWindowsHookEx(hook) };
    if state.verified {
        Ok(())
    } else {
        Err("protected physical user verification was cancelled".into())
    }
}

#[cfg(windows)]
pub(crate) fn physical_confirmation_transition(
    expected_index: usize,
    virtual_key: u32,
    flags: u32,
) -> (usize, bool) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED};
        if flags & (LLKHF_INJECTED.0 | LLKHF_LOWER_IL_INJECTED.0) != 0 {
            return (expected_index, false);
        }
        let consumed = PHYSICAL_CONFIRMATION_KEYS.contains(&virtual_key);
        let next = if PHYSICAL_CONFIRMATION_KEYS.get(expected_index) == Some(&virtual_key) {
            expected_index + 1
        } else if virtual_key == PHYSICAL_CONFIRMATION_KEYS[0] {
            1
        } else {
            0
        };
        (next, consumed)
    }
    #[cfg(not(windows))]
    {
        let _ = (expected_index, virtual_key, flags);
        (expected_index, false)
    }
}
