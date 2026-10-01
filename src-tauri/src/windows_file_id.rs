//! Stable Win32 file identity for the proxy's otherwise safe file-snapshot
//! checks. This FFI boundary lives outside `runtime::proxy`, which forbids
//! unsafe code; callers receive only an owned volume/128-bit file ID value.

use std::fs::File;
use std::io;
use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
};

#[allow(
    unsafe_code,
    reason = "GetFileInformationByHandleEx reads identity from a live File handle into a local FILE_ID_INFO"
)]
pub(crate) fn identity(file: &File) -> io::Result<(u64, [u8; 16])> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `file` owns a live handle and `info` is a writable FILE_ID_INFO
    // for the duration of this synchronous call. A failed query is not trust.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}
