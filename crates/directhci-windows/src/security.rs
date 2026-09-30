//! Small Windows file-security checks for the privileged service and journal.

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf, Prefix};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GENERIC_ALL, GENERIC_WRITE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
    INHERIT_ONLY_ACE, IsValidSid, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::Storage::FileSystem::{
    CreateDirectoryW, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, GetDriveTypeW, WRITE_DAC, WRITE_OWNER,
};
use windows::core::{HRESULT, PCWSTR, PWSTR};

const TRUSTED_INSTALLER_SID: &str =
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
const WRITE_RIGHTS: u32 = GENERIC_ALL.0 | GENERIC_WRITE.0 | FILE_WRITE_DATA.0
    | 0x0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
    | FILE_WRITE_EA.0 | FILE_WRITE_ATTRIBUTES.0 | FILE_DELETE_CHILD.0
    | DELETE.0 | WRITE_DAC.0 | WRITE_OWNER.0;
const REPLACE_RIGHTS: u32 =
    GENERIC_ALL.0 | DELETE.0 | FILE_DELETE_CHILD.0 | WRITE_DAC.0 | WRITE_OWNER.0;

pub fn validate_service_executable(path: &Path) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("resolve service executable: {error}"))?;
    if canonical.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        ["Users", "Downloads", "Desktop", "AppData", "Temp", "Tmp"]
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    }) {
        return Err("LocalSystem service executable is in a user or temporary directory".into());
    }
    let mut components = canonical.components();
    let root = match components.next() {
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) =>
        {
            let mut root = PathBuf::from(prefix.as_os_str());
            root.push("\\");
            root
        }
        _ => return Err("LocalSystem service executable must be on a local fixed drive".into()),
    };
    let root_wide = wide(&root);
    // 3 is DRIVE_FIXED; reject mapped VMware/network/removable drives.
    if unsafe { GetDriveTypeW(PCWSTR(root_wide.as_ptr())) } != 3 {
        return Err("LocalSystem service executable must be on a local fixed drive".into());
    }

    let mut current = Some(path);
    let mut first = true;
    while let Some(component) = current {
        reject_reparse(component)?;
        check_acl(
            component,
            if first { WRITE_RIGHTS } else { REPLACE_RIGHTS },
            false,
        )?;
        current = component.parent();
        first = false;
    }
    let parent = path
        .parent()
        .ok_or("service executable has no parent directory")?;
    // Creating a replacement file in the immediate parent is also unsafe.
    check_acl(parent, WRITE_RIGHTS, false)?;
    Ok(())
}

pub(crate) fn ensure_secure_journal_directory(path: &Path) -> Result<(), String> {
    validate_journal_parent(path)?;
    let sddl = wide_text("O:BAD:P(A;OICI;GA;;;SY)(A;OICI;GA;;;BA)");
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the SDDL buffer is terminated; the allocated descriptor is
    // freed after CreateDirectoryW returns.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|error| format!("build journal directory ACL: {error}"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let path_wide = wide(path);
    let created = unsafe { CreateDirectoryW(PCWSTR(path_wide.as_ptr()), Some(&attributes)) };
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    match created {
        Ok(()) => {}
        Err(error) if error.code() == HRESULT::from_win32(ERROR_ALREADY_EXISTS.0) => {}
        Err(error) => return Err(format!("create secure journal directory: {error}")),
    }
    validate_secure_journal_directory(path)
}

pub(crate) fn validate_secure_journal_directory(path: &Path) -> Result<(), String> {
    validate_journal_parent(path)?;
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect journal directory: {error}")),
        Ok(metadata) if !metadata.is_dir() => return Err("journal path is not a directory".into()),
        Ok(_) => {}
    }
    reject_reparse(path)?;
    check_acl(path, WRITE_RIGHTS, true)
}

fn validate_journal_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("journal directory has no parent")?;
    reject_reparse(parent)?;
    // A writable parent may let another user replace the protected child.
    check_acl(parent, REPLACE_RIGHTS, false)
}

pub(crate) fn reject_reparse(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(format!("reparse point is not trusted: {}", path.display()));
    }
    Ok(())
}

fn check_acl(path: &Path, unsafe_rights: u32, require_protected: bool) -> Result<(), String> {
    let path_wide = wide(path);
    let mut owner = PSID::default();
    let mut dacl = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: all output pointers are valid; descriptor owns the returned
    // owner and DACL pointers and remains live until LocalFree below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(path_wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        )
    };
    if status.0 != 0 {
        return Err(format!("inspect {} ACL: {}", path.display(), status.0));
    }
    let result = (|| {
        if owner.is_invalid() || dacl.is_null() || !trusted_sid(owner)? {
            return Err(format!(
                "untrusted owner or missing DACL: {}",
                path.display()
            ));
        }
        if require_protected {
            let mut control = 0;
            let mut revision = 0;
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
                .map_err(|error| format!("inspect journal ACL flags: {error}"))?;
            if control & SE_DACL_PROTECTED.0 == 0 {
                return Err(format!(
                    "journal DACL inherits permissions: {}",
                    path.display()
                ));
            }
        }
        // Only basic allow/deny ACEs are accepted. Unexpected object/callback
        // ACEs fail closed rather than relying on incomplete access evaluation.
        for index in 0..unsafe { (*dacl).AceCount } as u32 {
            let mut raw = std::ptr::null_mut();
            unsafe { GetAce(dacl, index, &mut raw) }
                .map_err(|error| format!("inspect {} ACE: {error}", path.display()))?;
            let ace = raw.cast::<ACCESS_ALLOWED_ACE>();
            let header = unsafe { &(*ace).Header };
            match header.AceType {
                0 => {
                    if (header.AceSize as usize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>() {
                        return Err(format!("invalid ACE on {}", path.display()));
                    }
                    // Inherit-only ACEs do not grant access to this executable
                    // or directory. Journal ACLs are stricter because their
                    // children must also remain protected.
                    if !require_protected && header.AceFlags & INHERIT_ONLY_ACE.0 as u8 != 0 {
                        continue;
                    }
                    let mask = unsafe { (*ace).Mask };
                    if mask & unsafe_rights != 0 {
                        let sid = PSID(unsafe { std::ptr::addr_of_mut!((*ace).SidStart) }.cast());
                        if !trusted_sid(sid)? {
                            return Err(format!(
                                "writable by an untrusted SID: {}",
                                path.display()
                            ));
                        }
                    }
                }
                1 => {} // A deny ACE cannot grant write access.
                _ => return Err(format!("unsupported ACE type on {}", path.display())),
            }
        }
        Ok(())
    })();
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    result
}

fn trusted_sid(sid: PSID) -> Result<bool, String> {
    if sid.is_invalid() || !unsafe { IsValidSid(sid) }.as_bool() {
        return Err("invalid security identifier".into());
    }
    if unsafe { IsWellKnownSid(sid, WinLocalSystemSid) }.as_bool()
        || unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid) }.as_bool()
    {
        return Ok(true);
    }
    let mut raw = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut raw) }
        .map_err(|error| format!("format owner SID: {error}"))?;
    let value = unsafe { raw.to_string() }.map_err(|error| error.to_string());
    let _ = unsafe { LocalFree(Some(HLOCAL(raw.0.cast()))) };
    Ok(value?.eq_ignore_ascii_case(TRUSTED_INSTALLER_SID))
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn wide_text(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
