//! Read-only checks of the launched fixture's effective Windows privileges.
//!
//! SAFER tokens can retain TokenElevation from their source. Match the official
//! Driver's access-control criteria instead of treating that flag as authority.

const STANDARD_USER_PRIVILEGES: &[&str] = &[
    "SeChangeNotifyPrivilege",
    "SeShutdownPrivilege",
    "SeUndockPrivilege",
    "SeIncreaseWorkingSetPrivilege",
    "SeTimeZonePrivilege",
];

fn verify_facts(integrity: u32, administrator_allow_capable: bool, privileges: &[String]) {
    assert!(
        integrity <= 0x2000,
        "fixture integrity {integrity:#x} exceeds Medium"
    );
    assert!(
        !administrator_allow_capable,
        "fixture retains an allow-capable Administrators SID"
    );
    for privilege in privileges {
        assert!(
            STANDARD_USER_PRIVILEGES.contains(&privilege.as_str()),
            "fixture retains non-standard privilege {privilege}"
        );
    }
}

#[cfg(all(windows, feature = "gui-e2e"))]
pub(super) fn assert_standard_user(pid: u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        CreateWellKnownSid, EqualSid, GetSidSubAuthority, GetSidSubAuthorityCount,
        GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeNameW, SID_AND_ATTRIBUTES,
        TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_PRIVILEGES,
        TOKEN_QUERY, TokenGroups, TokenIntegrityLevel, TokenPrivileges,
        WinBuiltinAdministratorsSid,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    fn information(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Vec<u64> {
        let mut needed = 0;
        unsafe { GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut needed) };
        assert!(needed > 0, "token information {class} has no size");
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        let read = unsafe {
            GetTokenInformation(
                token,
                class,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        assert_ne!(read, 0, "read token information {class}");
        assert!(needed as usize <= buffer.len() * 8);
        buffer
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    assert!(!process.is_null(), "open launched fixture process {pid}");
    let process = OwnedHandle(process);
    let mut token = std::ptr::null_mut();
    let opened = unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) };
    assert_ne!(opened, 0, "read launched fixture token {pid}");
    let token = OwnedHandle(token);

    let label = information(token.0, TokenIntegrityLevel);
    let label = unsafe { &*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
    let count = unsafe { GetSidSubAuthorityCount(label.Label.Sid) };
    assert!(!count.is_null() && unsafe { *count } > 0);
    let rid = unsafe { GetSidSubAuthority(label.Label.Sid, u32::from(*count - 1)) };
    assert!(!rid.is_null());
    let integrity = unsafe { *rid };

    let mut administrator_sid = [0u64; 12];
    let mut sid_size = std::mem::size_of_val(&administrator_sid) as u32;
    let sid = administrator_sid.as_mut_ptr().cast();
    assert_ne!(
        unsafe {
            CreateWellKnownSid(
                WinBuiltinAdministratorsSid,
                std::ptr::null_mut(),
                sid,
                &mut sid_size,
            )
        },
        0
    );
    let groups = information(token.0, TokenGroups);
    let header = groups.as_ptr().cast::<TOKEN_GROUPS>();
    let count = unsafe { (*header).GroupCount } as usize;
    let first = unsafe { std::ptr::addr_of!((*header).Groups).cast::<SID_AND_ATTRIBUTES>() };
    let mut administrator_allow_capable = false;
    for index in 0..count {
        let group = unsafe { *first.add(index) };
        if unsafe { EqualSid(group.Sid, sid) } != 0 {
            // Even a disabled allow-capable group can be re-enabled. Only
            // deny-only (or absence) is acceptable for a derived user token.
            administrator_allow_capable = group.Attributes & 0x10 == 0;
        }
    }
    let privileges = information(token.0, TokenPrivileges);
    let header = privileges.as_ptr().cast::<TOKEN_PRIVILEGES>();
    let count = unsafe { (*header).PrivilegeCount } as usize;
    let first = unsafe { std::ptr::addr_of!((*header).Privileges).cast::<LUID_AND_ATTRIBUTES>() };
    let mut names = Vec::new();
    for index in 0..count {
        let luid = unsafe { (*first.add(index)).Luid };
        let mut name = [0u16; 128];
        let mut length = name.len() as u32;
        assert_ne!(
            unsafe {
                LookupPrivilegeNameW(std::ptr::null(), &luid, name.as_mut_ptr(), &mut length)
            },
            0,
            "unresolved fixture privilege must fail closed"
        );
        names.push(String::from_utf16(&name[..length as usize]).unwrap());
    }
    verify_facts(integrity, administrator_allow_capable, &names);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn standard_user_access_facts_are_accepted() {
        verify_facts(0x2000, false, &["SeChangeNotifyPrivilege".into()]);
    }

    #[rstest]
    #[case(0x3000, false, "SeChangeNotifyPrivilege")]
    #[case(0x2100, false, "SeChangeNotifyPrivilege")]
    #[case(0x2000, true, "SeChangeNotifyPrivilege")]
    #[case(0x2000, false, "SeDebugPrivilege")]
    #[case(0x2000, false, "SeImpersonatePrivilege")]
    #[case(0x2000, false, "unresolved privilege")]
    fn unsafe_fixture_facts_fail_closed(
        #[case] integrity: u32,
        #[case] admin: bool,
        #[case] privilege: &str,
    ) {
        assert!(
            std::panic::catch_unwind(|| verify_facts(integrity, admin, &[privilege.into()]))
                .is_err()
        );
    }
}
