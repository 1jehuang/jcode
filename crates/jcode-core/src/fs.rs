use std::path::Path;

#[cfg(all(test, windows))]
#[path = "fs_windows_tests.rs"]
mod windows_tests;

/// Set file permissions to owner-only read/write (0o600).
/// On Windows, replaces the DACL with a protected full-control ACE for the
/// current process user so inherited or explicit access for other principals
/// cannot expose secret-bearing files.
pub fn set_permissions_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        set_windows_acl_owner_only(path, 0)
    }
}

/// Set directory permissions to owner-only read/write/execute (0o700).
/// New Windows child objects inherit the same current-user-only access rule.
/// Existing descendants are not modified. Secret files must be hardened
/// individually, including legacy files discovered during read-time repair.
pub fn set_directory_permissions_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::SUB_CONTAINERS_AND_OBJECTS_INHERIT;
        set_windows_acl_owner_only(path, SUB_CONTAINERS_AND_OBJECTS_INHERIT)
    }
}

#[cfg(windows)]
fn set_windows_acl_owner_only(
    path: &Path,
    inheritance: windows_sys::Win32::Security::ACE_FLAGS,
) -> std::io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, SET_ACCESS, SetEntriesInAclW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetTokenInformation, InitializeSecurityDescriptor,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, SECURITY_DESCRIPTOR,
        SetFileSecurityW, SetSecurityDescriptorControl, SetSecurityDescriptorDacl, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let result = (|| {
        let mut needed = 0u32;
        unsafe {
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(std::io::Error::last_os_error());
        }

        // TOKEN_USER contains pointer-aligned data followed by its SID. A usize
        // buffer gives the cast the alignment required by the Windows ABI.
        let word = std::mem::size_of::<usize>();
        let mut token_user = vec![0usize; (needed as usize).div_ceil(word)];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_user.as_mut_ptr().cast::<c_void>(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let user = unsafe { &*(token_user.as_ptr().cast::<TOKEN_USER>()) };

        let trustee = TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: user.User.Sid.cast::<u16>(),
        };
        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
            grfAccessMode: SET_ACCESS,
            grfInheritance: inheritance,
            Trustee: trustee,
        };
        let mut acl = null_mut();
        let acl_status = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
        if acl_status != 0 {
            return Err(std::io::Error::from_raw_os_error(acl_status as i32));
        }

        // SetNamedSecurityInfoW propagates inheritable ACEs to existing children,
        // turning an auth.json write into a permission walk of the entire home.
        // SetFileSecurityW changes only this object. Keep inheritable access for
        // future children, and protect the DACL against permissive ancestors.
        let apply_result = (|| -> std::io::Result<()> {
            let mut descriptor: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
            let descriptor_ptr = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast::<c_void>();
            // SECURITY_DESCRIPTOR_REVISION is 1 in the Windows descriptor ABI.
            if unsafe { InitializeSecurityDescriptor(descriptor_ptr, 1) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, acl, 0) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if unsafe {
                SetSecurityDescriptorControl(descriptor_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED)
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let wide_path = path
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            if unsafe {
                SetFileSecurityW(
                    wide_path.as_ptr(),
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    descriptor_ptr,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })();
        unsafe {
            LocalFree(acl.cast::<c_void>());
        }
        apply_result
    })();

    unsafe {
        CloseHandle(token);
    }
    result
}
