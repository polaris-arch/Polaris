//! Native token identity. Pipe credentials are captured while impersonating,
//! then reverted before token/registry reads or privileged command dispatch.
use crate::user_control::NativePeer;
use polaris_helper_proto::core_protocol::CallerIdentity;
use std::io;
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE},
    Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW},
    Security::{
        CheckTokenMembership, GetTokenInformation, RevertToSelf, TokenElevation, TokenUser,
        TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
    },
    System::Pipes::ImpersonateNamedPipeClient,
    System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
};

struct Token(HANDLE);
impl Drop for Token {
    #[allow(unsafe_code, reason = "close owned native token")]
    fn drop(&mut self) {
        // SAFETY: the successful open transferred one handle to this owner.
        unsafe { CloseHandle(self.0) };
    }
}
struct Impersonation(bool);
impl Impersonation {
    #[allow(
        unsafe_code,
        reason = "revert only this worker thread's client impersonation"
    )]
    fn revert(&mut self) -> io::Result<()> {
        if self.0 {
            // SAFETY: only this worker entered impersonation; failure must abort dispatch.
            if unsafe { RevertToSelf() } == 0 {
                return Err(io::Error::last_os_error());
            }
            self.0 = false;
        }
        Ok(())
    }
}
impl Drop for Impersonation {
    fn drop(&mut self) {
        let _ = self.revert();
    }
}

#[allow(
    unsafe_code,
    reason = "read one live token's SID with correctly aligned bounded storage"
)]
fn identity(token: &Token) -> io::Result<CallerIdentity> {
    let mut length = 0;
    // SAFETY: null output requests the required size; length is live.
    unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut length) };
    if length < std::mem::size_of::<TOKEN_USER>() as u32 || length > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid token user size",
        ));
    }
    let mut storage = vec![0u64; (length as usize).div_ceil(8)];
    let capacity = (storage.len() * 8) as u32;
    // SAFETY: u64 storage aligns TOKEN_USER, capacity is the actual allocation size.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            capacity,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if length < std::mem::size_of::<TOKEN_USER>() as u32 || length > capacity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid returned token user size",
        ));
    }
    // SAFETY: successful TOKEN_USER output remains alive for the complete conversion.
    let sid = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() }.User.Sid;
    let mut raw = std::ptr::null_mut();
    // SAFETY: SID belongs to the live token output buffer; API owns its output allocation.
    if unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut count = 0;
    // SAFETY: successful conversion returns a NUL-terminated SID string; bound is above native SID maximum.
    unsafe {
        while count < 200 && *raw.add(count) != 0 {
            count += 1;
        }
    }
    // SAFETY: at most the bounded valid SID string; owned String survives allocation free below.
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(raw, count) });
    // SAFETY: allocation returned by ConvertSidToStringSidW, released exactly once.
    unsafe { LocalFree(raw.cast()) };
    value
        .ok()
        .and_then(|sid| CallerIdentity::windows(&sid))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid native SID"))
}

#[allow(
    unsafe_code,
    reason = "inspect effective client administrator membership, without modifying token"
)]
fn administrator(token: &Token, caller: &CallerIdentity) -> io::Result<bool> {
    if caller == &CallerIdentity::Windows("S-1-5-18".into()) {
        return Ok(true);
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut length = 0;
    // SAFETY: exact native output structure and size, token is live.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let wide: Vec<_> = "S-1-5-32-544".encode_utf16().chain(Some(0)).collect();
    let mut sid = std::ptr::null_mut();
    // SAFETY: NUL terminated well-known SID and live output pointer.
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut member = 0;
    // SAFETY: captured impersonation token, live well-known SID and output pointer.
    let ok = unsafe { CheckTokenMembership(token.0, sid, &mut member) };
    let error = (ok == 0).then(io::Error::last_os_error);
    // SAFETY: the successful SID conversion allocation has one owner.
    unsafe { LocalFree(sid) };
    if let Some(error) = error {
        return Err(error);
    }
    Ok(elevation.TokenIsElevated != 0 && member != 0)
}

#[allow(
    unsafe_code,
    reason = "capture authenticated named-pipe token before privileged dispatch"
)]
pub fn from_pipe(pipe: HANDLE) -> io::Result<NativePeer> {
    // SAFETY: caller retains the connected pipe handle throughout this synchronous capture.
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut impersonation = Impersonation(true);
    let mut raw = std::ptr::null_mut();
    // SAFETY: read only this worker's current client token; OpenAsSelf permits querying it.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Token(raw);
    let identity = identity(&token)?;
    let administrator = administrator(&token, &identity)?;
    impersonation.revert()?;
    Ok(NativePeer {
        identity,
        administrator,
    })
}

#[allow(
    unsafe_code,
    reason = "non-privileged App reads its own current process SID"
)]
pub fn current_user() -> io::Result<CallerIdentity> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: current process pseudo handle, read-only access and live output pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    identity(&Token(raw))
}
