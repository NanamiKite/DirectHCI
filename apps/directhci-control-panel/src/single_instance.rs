//! Session-local panel ownership and a show-only activation message. A duplicate
//! process never constructs a Panel and therefore cannot execute its close path.

use std::marker::PhantomData;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use native_windows_gui as nwg;
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, EnumWindows, FLASHW_TRAY, FLASHWINFO, FlashWindowEx,
    GetLastActivePopup, GetPropW, GetWindowThreadProcessId, RegisterWindowMessageW, RemovePropW,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, SetForegroundWindow, SetPropW,
};
use windows::core::{BOOL, w};

const ACK: usize = 0x4448_4349;
const WINDOW_PROPERTY: windows::core::PCWSTR = w!("DirectHCI.ControlPanel.ActivationWindow.v1");

pub struct SingleInstance {
    mutex: HANDLE,
    // The creating UI thread must also release the mutex.
    _thread: PhantomData<Rc<()>>,
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: this thread acquired and still owns the mutex.
        let _ = unsafe { ReleaseMutex(self.mutex) };
        let _ = unsafe { CloseHandle(self.mutex) };
    }
}

/// Call only after elevation, before any panel/worker/IPC creation. None means
/// an existing panel acknowledged activation; the caller should simply return.
pub fn enter() -> Result<Option<SingleInstance>, String> {
    // Local namespace scopes the singleton to the interactive Windows session.
    // Use ownership, not ERROR_ALREADY_EXISTS: abandoned ownership is recoverable.
    let mutex = unsafe { CreateMutexW(None, false, w!("Local\\DirectHCI.ControlPanel.v1")) }
        .map_err(|error| format!("open Control Panel instance guard: {error}"))?;
    match unsafe { WaitForSingleObject(mutex, 0) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(SingleInstance {
            mutex,
            _thread: PhantomData,
        })),
        WAIT_TIMEOUT => {
            let _ = unsafe { CloseHandle(mutex) };
            activate_existing()?;
            Ok(None)
        }
        _ => {
            let error = windows::core::Error::from_thread();
            let _ = unsafe { CloseHandle(mutex) };
            Err(format!("check Control Panel instance: {error}"))
        }
    }
}

fn activation_message() -> Result<u32, String> {
    let message = unsafe { RegisterWindowMessageW(w!("DirectHCI.ControlPanel.Show.v1")) };
    if message == 0 {
        Err(format!(
            "register Control Panel activation: {}",
            windows::core::Error::from_thread()
        ))
    } else {
        Ok(message)
    }
}

unsafe extern "system" fn find_window(window: HWND, parameter: LPARAM) -> BOOL {
    // SAFETY: EnumWindows invokes us synchronously with a live HWND output slot.
    if unsafe { GetPropW(window, WINDOW_PROPERTY) }.0 as usize == 1 {
        unsafe { *(parameter.0 as *mut HWND) = window };
    }
    true.into()
}

fn activate_existing() -> Result<(), String> {
    let message = activation_message()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut window = HWND::default();
        // Enumerates the calling desktop, including hidden tray windows. No
        // window-title match and no broadcast of messages to unrelated windows.
        unsafe {
            EnumWindows(
                Some(find_window),
                LPARAM((&mut window as *mut HWND) as isize),
            )
        }
        .map_err(|error| format!("find existing Control Panel: {error}"))?;
        if !window.0.is_null() {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            if pid != 0 {
                // Best effort: foreground policy can still deny focus stealing.
                let _ = unsafe { AllowSetForegroundWindow(pid) };
            }
            let mut response = 0;
            let sent = unsafe {
                SendMessageTimeoutW(
                    window,
                    message,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG | SMTO_BLOCK,
                    1_000,
                    Some(&mut response),
                )
            };
            if sent.0 != 0 && response == ACK {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err("DirectHCI Control Panel is already running but did not acknowledge activation. It may be starting, closing, or unresponsive. No second panel was opened and no service was stopped; retry after it finishes.".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Unbind before the window/Rc<Panel> is destroyed, including error returns.
pub struct ActivationRegistration {
    window: HWND,
    handler: nwg::RawEventHandler,
}

impl Drop for ActivationRegistration {
    fn drop(&mut self) {
        let _ = unsafe { RemovePropW(self.window, WINDOW_PROPERTY) };
        let _ = nwg::unbind_raw_event_handler(&self.handler);
    }
}

pub fn bind(
    window: &nwg::Window,
    show: impl Fn() + 'static,
) -> Result<ActivationRegistration, String> {
    let message = activation_message()?;
    let hwnd = HWND(
        window
            .handle
            .hwnd()
            .ok_or("Control Panel window is not initialized")?
            .cast(),
    );
    let handler = nwg::bind_raw_event_handler(
        &window.handle,
        0x4448_0001,
        move |_, msg, wparam, lparam| {
            if msg != message || wparam != 0 || lparam != 0 {
                return None;
            }
            // Show-only notification on the existing UI thread. Never dispatch an
            // Action, stop the service, close a dialog, or reset an in-flight operation.
            show();
            let popup = unsafe { GetLastActivePopup(hwnd) };
            let target = if popup.0.is_null() { hwnd } else { popup };
            if !unsafe { SetForegroundWindow(target) }.as_bool() {
                let flash = FLASHWINFO {
                    cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                    hwnd,
                    dwFlags: FLASHW_TRAY,
                    uCount: 3,
                    dwTimeout: 0,
                };
                unsafe { FlashWindowEx(&flash) };
            }
            Some(ACK as isize)
        },
    )
    .map_err(|error| format!("bind Control Panel activation: {error}"))?;
    // Advertise only after the receiver is registered. The second process waits
    // briefly if the first has acquired the singleton but is still creating UI.
    if let Err(error) = unsafe { SetPropW(hwnd, WINDOW_PROPERTY, Some(HANDLE(1usize as *mut _))) } {
        let _ = nwg::unbind_raw_event_handler(&handler);
        return Err(format!("publish Control Panel activation window: {error}"));
    }
    Ok(ActivationRegistration {
        window: hwnd,
        handler,
    })
}
