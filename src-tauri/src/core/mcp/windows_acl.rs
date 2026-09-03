use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, GetTokenInformation, IsValidSid,
    NO_INHERITANCE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::error::{AppError, AppResult};

pub(super) fn set_current_user_only(path: &Path, directory: bool) -> AppResult<()> {
    let user = current_user_sid(path)?;
    let acl = create_private_acl(path, user.sid(), directory)?;
    let path_wide = wide_path(path)?;
    let security_information = OWNER_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | PROTECTED_DACL_SECURITY_INFORMATION;
    let status = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            security_information,
            user.sid(),
            null_mut(),
            acl.as_ptr(),
            null(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(status_error("SetNamedSecurityInfoW", path, status));
    }
    Ok(())
}

fn current_user_sid(path: &Path) -> AppResult<CurrentUserSid> {
    let mut raw_token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) } == 0 {
        return Err(last_error("OpenProcessToken", path));
    }
    let token = OwnedHandle(raw_token);

    let mut required_length = 0;
    let initial_result =
        unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required_length) };
    if initial_result != 0 {
        return Err(AppError::Config(format!(
            "Failed to query the current user SID for MCP discovery data during \
             GetTokenInformation(size) for '{}': unexpected result",
            path.display()
        )));
    }
    let error = unsafe { GetLastError() };
    if error != ERROR_INSUFFICIENT_BUFFER || required_length == 0 {
        return Err(status_error("GetTokenInformation(size)", path, error));
    }

    // TOKEN_USER contains pointer-sized fields. A usize buffer keeps the allocation aligned
    // while still providing the variable-length storage required for the trailing SID.
    let word_count = (required_length as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0usize; word_count];
    let buffer_length = required_length;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            buffer_length,
            &mut required_length,
        )
    } == 0
    {
        return Err(last_error("GetTokenInformation(TokenUser)", path));
    }

    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let sid = token_user.User.Sid;
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(AppError::Config(format!(
            "Failed to query the current user SID for MCP discovery data during \
             GetTokenInformation(TokenUser) for '{}': Windows returned an invalid SID",
            path.display()
        )));
    }

    Ok(CurrentUserSid {
        _buffer: buffer,
        sid,
    })
}

fn create_private_acl(path: &Path, sid: PSID, directory: bool) -> AppResult<LocalAllocation<ACL>> {
    let inheritance = if directory {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        NO_INHERITANCE
    };
    let trustee = TRUSTEE_W {
        pMultipleTrustee: null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_USER,
        ptstrName: sid.cast(),
    };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: inheritance,
        Trustee: trustee,
    };
    let mut raw_acl: *mut ACL = null_mut();
    let status = unsafe { SetEntriesInAclW(1, &access, null(), &mut raw_acl) };
    if status != ERROR_SUCCESS {
        if !raw_acl.is_null() {
            drop(LocalAllocation(raw_acl));
        }
        return Err(status_error("SetEntriesInAclW", path, status));
    }
    if raw_acl.is_null() {
        return Err(AppError::Config(format!(
            "Failed to build a current-user-only ACL for MCP discovery data during \
             SetEntriesInAclW for '{}': Windows returned a null ACL",
            path.display()
        )));
    }
    Ok(LocalAllocation(raw_acl))
}

fn wide_path(path: &Path) -> AppResult<Vec<u16>> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(AppError::Config(format!(
            "Failed to apply a current-user-only ACL to MCP discovery data: path contains an \
             embedded NUL: '{}'",
            path.display()
        )));
    }
    wide.push(0);
    Ok(wide)
}

fn last_error(operation: &str, path: &Path) -> AppError {
    status_error(operation, path, unsafe { GetLastError() })
}

fn status_error(operation: &str, path: &Path, status: u32) -> AppError {
    let source = std::io::Error::from_raw_os_error(status as i32);
    AppError::Config(format!(
        "Failed to apply a current-user-only ACL to MCP discovery data during {operation} for \
         '{}': win32_error={status} ({source})",
        path.display()
    ))
}

struct CurrentUserSid {
    _buffer: Vec<usize>,
    sid: PSID,
}

impl CurrentUserSid {
    const fn sid(&self) -> PSID {
        self.sid
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

struct LocalAllocation<T>(*mut T);

impl<T> LocalAllocation<T> {
    const fn as_ptr(&self) -> *mut T {
        self.0
    }
}

impl<T> Drop for LocalAllocation<T> {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { LocalFree(self.0.cast::<c_void>()) };
        }
    }
}

#[cfg(test)]
pub(super) fn assert_current_user_only(path: &Path, directory: bool) {
    use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation, EqualSid, GetAce,
        GetAclInformation, GetSecurityDescriptorControl, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };

    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    let user = current_user_sid(path).unwrap();
    let path_wide = wide_path(path).unwrap();
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut security_descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut security_descriptor,
        )
    };
    assert_eq!(
        status,
        ERROR_SUCCESS,
        "{}",
        status_error("GetNamedSecurityInfoW", path, status)
    );
    let _security_descriptor = LocalAllocation(security_descriptor);
    assert!(!security_descriptor.is_null());
    assert!(!owner.is_null());
    assert!(!dacl.is_null());
    assert_ne!(unsafe { EqualSid(owner, user.sid()) }, 0);

    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(security_descriptor, &mut control, &mut revision) },
        0
    );
    assert_ne!(control & SE_DACL_PROTECTED, 0);

    let mut acl_info = ACL_SIZE_INFORMATION::default();
    assert_ne!(
        unsafe {
            GetAclInformation(
                dacl,
                (&raw mut acl_info).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        },
        0
    );
    assert_eq!(acl_info.AceCount, 1);

    let mut raw_ace: *mut c_void = null_mut();
    assert_ne!(unsafe { GetAce(dacl, 0, &mut raw_ace) }, 0);
    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
    assert_eq!(unsafe { (*ace).Header.AceType }, ACCESS_ALLOWED_ACE_TYPE);
    assert_eq!(unsafe { (*ace).Mask }, FILE_ALL_ACCESS);
    let expected_flags = if directory {
        OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
    } else {
        NO_INHERITANCE
    };
    assert_eq!(u32::from(unsafe { (*ace).Header.AceFlags }), expected_flags);
    let ace_sid = unsafe {
        std::ptr::addr_of!((*ace).SidStart)
            .cast_mut()
            .cast::<c_void>()
    };
    assert_ne!(unsafe { EqualSid(ace_sid, user.sid()) }, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_current_user_only_acl_to_directory_and_file() {
        let root =
            std::env::temp_dir().join(format!("nyaterm-windows-acl-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        set_current_user_only(&root, true).unwrap();

        let file = root.join("discovery.json");
        std::fs::write(&file, b"{}").unwrap();
        set_current_user_only(&file, false).unwrap();

        assert_current_user_only(&root, true);
        assert_current_user_only(&file, false);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_path_returns_contextual_win32_error() {
        let path = std::env::temp_dir()
            .join(format!("nyaterm-missing-acl-test-{}", uuid::Uuid::new_v4()))
            .join("discovery.json");
        let error = set_current_user_only(&path, false).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("SetNamedSecurityInfoW"));
        assert!(message.contains("win32_error="));
        assert!(message.contains(&path.display().to_string()));
    }
}
