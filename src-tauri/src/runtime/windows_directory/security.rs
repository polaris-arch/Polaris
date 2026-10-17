//! Read-only strict owner/protected-DACL admission. Never repair or widen ACLs.
use super::*;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
fn current_user() -> io::Result<Vec<usize>> {
    let mut raw = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut size = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut size,
        )
    };
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || size == 0
        || size > 65536
    {
        return Err(unknown());
    }
    let mut storage = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            storage.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(storage)
}
pub(super) fn check_private(file: &File) -> io::Result<()> {
    let storage = current_user()?;
    let user = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut owner: PSID = std::ptr::null_mut();
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let error = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut acl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    if descriptor.is_null() {
        return Err(unknown());
    }
    let result = (|| {
        if owner.is_null()
            || acl.is_null()
            || unsafe { IsValidSid(owner) } == 0
            || unsafe { EqualSid(owner, user) } == 0
        {
            return Err(unknown());
        }
        let mut control = 0u16;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
            || unsafe { IsValidAcl(acl) } == 0
        {
            return Err(unknown());
        }
        let count = unsafe { (*acl).AceCount };
        if count != 2 {
            return Err(unknown());
        }
        let (mut owner_allowed, mut system_allowed) = (false, false);
        for index in 0..count {
            let mut ace = std::ptr::null_mut();
            if unsafe { GetAce(acl, u32::from(index), &mut ace) } == 0 || ace.is_null() {
                return Err(unknown());
            }
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            if header.AceType != 0
                || header.AceFlags & !0x03 != 0
                || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
            {
                return Err(unknown());
            }
            let allow = ace.cast::<ACCESS_ALLOWED_ACE>();
            if unsafe { (*allow).Mask } != FILE_ALL_ACCESS {
                return Err(unknown());
            }
            let sid = unsafe { std::ptr::addr_of!((*allow).SidStart).cast_mut().cast() };
            if unsafe { IsValidSid(sid) } == 0 {
                return Err(unknown());
            }
            if unsafe { EqualSid(sid, user) } != 0 {
                if owner_allowed {
                    return Err(unknown());
                }
                owner_allowed = true;
            } else if unsafe { IsWellKnownSid(sid, WinLocalSystemSid) } != 0 {
                if system_allowed {
                    return Err(unknown());
                }
                system_allowed = true;
            } else {
                return Err(unknown());
            }
        }
        if !owner_allowed || !system_allowed {
            return Err(unknown());
        }
        Ok(())
    })();
    if !unsafe { LocalFree(descriptor.cast()) }.is_null() {
        return Err(unknown());
    }
    result
}
