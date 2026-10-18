//! A process-lifetime, hidden top-level window receives session broadcasts even
//! when lightweight mode has destroyed every webview. A message-only window
//! would not receive these broadcasts.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;
use windows_sys::core::w;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::runtime::proxy::ProxyRuntime;
use crate::runtime::AppRuntime;

// Return before Windows' five-second unresponsive-application threshold. The
// blocking registry task retains its controller/lease if this wait times out.
const QUERY_BUDGET: Duration = Duration::from_secs(2);

struct Context {
    app: tauri::AppHandle,
    proxy: Arc<ProxyRuntime>,
    query: Cell<Option<u64>>,
    reason: Vec<u16>,
}

struct SessionWindow(usize);

pub(crate) fn install(app: &tauri::AppHandle) -> std::io::Result<()> {
    let context = Rc::new(Context {
        app: app.clone(),
        proxy: app.state::<AppRuntime>().proxy.clone(),
        query: Cell::new(None),
        // Read configuration before the window can receive QUERYENDSESSION.
        // The shutdown callback must never wait for configuration locks or disk.
        reason: crate::i18n::t(
            crate::i18n::app_lang(app),
            crate::i18n::key::NATIVE_SESSION_PROXY_PENDING,
        )
        .encode_utf16()
        .chain(Some(0))
        .collect(),
    });
    // SAFETY: fixed class name, a valid WNDPROC, and the calling UI thread owns
    // both registration and window creation. There is no parent/message-only HWND.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        if instance.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("PolarisSessionEnd"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.lpszClassName,
            w!("Polaris"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            Rc::as_ptr(&context).cast(),
        )
    };
    if hwnd.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    app.manage(SessionWindow(hwnd as usize));
    Ok(())
}

pub(crate) fn destroy(app: &tauri::AppHandle) {
    if let Some(window) = app.try_state::<SessionWindow>() {
        // SAFETY: called by RunEvent::Exit on the same UI thread as install.
        // WM_NCDESTROY releases the window's one Context reference.
        if unsafe { DestroyWindow(window.0 as HWND) } == 0 {
            log::warn!(
                "session-end window destruction failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Never unwind through the OS callback. A query panic is an unknown cleanup
    // outcome and cannot authorize ordinary shutdown.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: Windows supplies this window's valid message parameters.
        unsafe { dispatch(hwnd, message, wparam, lparam) }
    }))
    .unwrap_or_else(|_| {
        log::error!("session-end callback panicked; proxy restoration unconfirmed");
        0
    })
}

unsafe fn dispatch(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE provides CREATESTRUCTW; install keeps the Rc live
        // through CreateWindowExW. Successful creation lends one reference to HWND.
        unsafe {
            let create = &*(lparam as *const CREATESTRUCTW);
            let context = create.lpCreateParams as *const Context;
            windows_sys::Win32::Foundation::SetLastError(0);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, context as isize);
            if windows_sys::Win32::Foundation::GetLastError() != 0 {
                return 0;
            }
            Rc::increment_strong_count(context);
        }
        return 1;
    }
    // SAFETY: only WM_NCCREATE writes userdata, retaining a reference until
    // WM_NCDESTROY clears it. All dispatches occur on this window's UI thread.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Context;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let context = unsafe { &*pointer };
    match message {
        WM_QUERYENDSESSION => {
            // SAFETY: owned UI-thread HWND and live, NUL-terminated reason.
            if unsafe { ShutdownBlockReasonCreate(hwnd, context.reason.as_ptr()) } == 0 {
                log::error!(
                    "cannot publish system proxy shutdown reason: {}",
                    std::io::Error::last_os_error()
                );
            }
            let epoch = match context.proxy.begin_windows_session_query() {
                Ok(epoch) => epoch,
                Err(error) => {
                    log::error!("session query admission unconfirmed: {error}");
                    return 0;
                }
            };
            context.query.set(Some(epoch));
            // OS-backed wait stays bounded even if the async pool is busy or a
            // registry worker hangs. Timeout does not cancel retained restoration.
            let restored = crate::session_restore::wait(QUERY_BUDGET, |send| {
                let proxy = context.proxy.clone();
                let app = context.app.clone();
                tauri::async_runtime::spawn(async move {
                    let result = proxy.restore_system_proxy_for_exit().await;
                    crate::session_restore::complete(send, result, |late| match late {
                        Ok(()) => cancelled(app, proxy, epoch),
                        Err(error) => {
                            log::error!("late system proxy restoration unconfirmed: {error}");
                            failed(app, proxy, epoch);
                        }
                    });
                });
            });
            match restored {
                Ok(Ok(())) => {
                    // SAFETY: same owned UI-thread window. Persistent proxy is safe;
                    // DNS cache completion is deliberately absent from this decision.
                    unsafe {
                        ShutdownBlockReasonDestroy(hwnd);
                    }
                    1
                }
                Ok(Err(error)) => {
                    log::error!("ordinary session end refused: {error}");
                    context.query.set(None);
                    failed(context.app.clone(), context.proxy.clone(), epoch);
                    0
                }
                Err(error) => {
                    log::error!(
                        "ordinary session end refused: proxy restoration unconfirmed: {error}"
                    );
                    // FALSE need not receive ENDSESSION. The retained task handles
                    // its late result; keep admission closed until it actually settles.
                    context.query.set(None);
                    notice(&context.app, crate::i18n::key::NATIVE_SESSION_PROXY_PENDING);
                    0
                }
            }
        }

        WM_ENDSESSION => {
            if wparam != 0 {
                polaris_system_integration::windows_session::begin_final_exit();
                crate::exit_lifecycle::final_exit_best_effort(&context.app);
            } else {
                unsafe {
                    ShutdownBlockReasonDestroy(hwnd);
                }
                if let Some(epoch) = context.query.take() {
                    cancelled(context.app.clone(), context.proxy.clone(), epoch);
                }
            }
            0
        }
        WM_NCDESTROY => {
            // SAFETY: clear first; consume exactly the reference retained at NCCREATE.
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Rc::from_raw(pointer));
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn failed(app: tauri::AppHandle, proxy: Arc<ProxyRuntime>, epoch: u64) {
    // Queue the potentially contended admission lock outside the UI callback.
    // Until this worker completes, the query gate remains closed.
    proxy.defer_windows_session_query_cancel(epoch, move |reopened| {
        if reopened {
            notice(&app, crate::i18n::key::NATIVE_SESSION_PROXY_PENDING);
        }
    });
}

fn cancelled(app: tauri::AppHandle, proxy: Arc<ProxyRuntime>, epoch: u64) {
    tauri::async_runtime::spawn(async move {
        if !proxy.windows_session_query_is_current(epoch) {
            return;
        }
        // Ordinary Stop retains retryable ownership and does not permanently
        // fence Start. Do not re-enable a proxy over another program's changes.
        let stopped = proxy.stop_for_session_cancel().await;
        if !proxy.cancel_windows_session_query(epoch) {
            return;
        }
        let key = if stopped.is_ok() {
            crate::i18n::key::NATIVE_SESSION_CANCELLED
        } else {
            log::error!("cancelled session: proxy stop unconfirmed: {stopped:?}");
            crate::i18n::key::NATIVE_SESSION_CANCEL_FAILED
        };
        notice(&app, key);
    });
}

fn notice(app: &tauri::AppHandle, key: &'static str) {
    let app = app.clone();
    // Always queue from a worker; opening a webview/dialog must not extend the
    // bounded QUERYENDSESSION callback even if run_on_main_thread runs inline.
    tauri::async_runtime::spawn(async move {
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if crate::exit_lifecycle::exit_is_committed(&handle) {
                return;
            }
            crate::show_main_window(&handle);
            use tauri_plugin_dialog::DialogExt;
            handle
                .dialog()
                .message(crate::i18n::t(crate::i18n::app_lang(&handle), key))
                .title("Polaris")
                .show(|_| {});
        }) {
            log::error!("cannot show session cleanup outcome: {error}");
        }
    });
}
