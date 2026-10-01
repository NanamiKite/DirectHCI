#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod service;

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        native_windows_gui::fatal_message("DirectHCI Control Panel", &error);
    }
}

#[cfg(windows)]
fn run() -> Result<(), String> {
    if !ensure_elevated()? {
        return Ok(());
    }
    app::run()
}

#[cfg(windows)]
fn ensure_elevated() -> Result<bool, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Security::{
        CheckTokenMembership, CreateWellKnownSid, PSID, WinBuiltinAdministratorsSid,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{BOOL, PCWSTR};

    let mut storage = [0u32; 17];
    let mut size = std::mem::size_of_val(&storage) as u32;
    let sid = PSID(storage.as_mut_ptr().cast());
    // SAFETY: storage remains live and is large enough for a well-known SID.
    unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(sid), &mut size) }
        .map_err(|error| format!("create administrators SID: {error}"))?;
    let mut member = BOOL(0);
    // SAFETY: a null token asks Win32 to check this process's effective token;
    // sid points to the live well-known-SID storage above.
    unsafe { CheckTokenMembership(None, sid, &mut member) }
        .map_err(|error| format!("check administrator membership: {error}"))?;
    if member.as_bool() {
        return Ok(true);
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("locate Control Panel executable: {error}"))?;
    let mut executable_wide: Vec<u16> = executable.as_os_str().encode_wide().collect();
    executable_wide.push(0);
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    // ShellExecuteW expects COM to be initialized on the calling thread.
    // SAFETY: this is the GUI startup thread, before NWG initializes its UI.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() {
        return Err(format!("initialize COM for elevation: {initialized:?}"));
    }
    // SAFETY: the verb and executable path are live, NUL-terminated UTF-16
    // strings; no privileged GUI action occurs in this unelevated process.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(executable_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // SAFETY: balances the successful CoInitializeEx call above.
    unsafe { CoUninitialize() };
    if (result.0 as isize) <= 32 {
        return Err(format!(
            "Control Panel requires administrator approval (ShellExecuteW result {})",
            result.0 as isize
        ));
    }
    // The elevated child opens the UI; this unelevated parent exits.
    Ok(false)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("DirectHCI Control Panel is available only on Windows");
}
