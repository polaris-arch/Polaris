use windows::core::{s, w};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};

pub(super) struct NativeApi {
    module: HMODULE,
    flush: unsafe extern "system" fn() -> i32,
}

#[allow(
    unsafe_code,
    reason = "loads the optional DNS API from System32 and checks its export"
)]
pub(super) fn load() -> Result<NativeApi, String> {
    // SAFETY: a fixed DLL name and System32-only search prohibit app/PATH DLL loading.
    let module = unsafe { LoadLibraryExW(w!("dnsapi.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
        .map_err(|error| format!("System32 DNS cache API unavailable: {error}"))?;
    // SAFETY: module is live; the static NUL-terminated export name is valid.
    let export = unsafe { GetProcAddress(module, s!("DnsFlushResolverCache")) };
    let Some(export) = export else {
        // SAFETY: this load owns exactly one library reference and no call is active.
        let _ = unsafe { FreeLibrary(module) };
        return Err("System32 DNS cache API export unavailable".into());
    };
    // SAFETY: this optional Windows DNS export uses BOOL WINAPI(void), also used by
    // SagerNet/sing common/windnsapi. It is undocumented, so absence is a normal error.
    let flush = unsafe {
        std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            unsafe extern "system" fn() -> i32,
        >(export)
    };
    Ok(NativeApi { module, flush })
}

impl NativeApi {
    #[allow(
        unsafe_code,
        reason = "calls the checked DNS export while its DLL is owned"
    )]
    pub(super) fn flush(&self) -> Result<(), String> {
        // SAFETY: the checked function takes no pointers; self holds the module reference.
        if unsafe { (self.flush)() } != 0 {
            Ok(())
        } else {
            // No documented last-error contract: do not report a stale GetLastError value.
            Err("native DNS cache flush failed".into())
        }
    }
}

impl Drop for NativeApi {
    #[allow(
        unsafe_code,
        reason = "releases this worker's DLL reference after its call finishes"
    )]
    fn drop(&mut self) {
        // SAFETY: the call has returned and this is the one owned loader reference.
        let _ = unsafe { FreeLibrary(self.module) };
    }
}
