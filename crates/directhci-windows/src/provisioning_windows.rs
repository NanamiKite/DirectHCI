//! Privileged, stage-only device-specific WinUSB preparation.
//! libwdi creates/signs the catalog; SetupAPI stages but never installs it.

use std::ffi::{CString, c_char};
use std::fs;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SP_COPY_STYLE, SPOST_PATH, SetupCopyOEMInfW,
};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LoadLibraryExW,
};
use windows::Win32::System::Rpc::UuidCreate;
use windows::core::{GUID, PCSTR, PCWSTR};

use crate::provisioning::{device_specific_package_blueprint, eligible_device_specific_package};
use crate::rebind::{DirectHciPackageReadiness, same_physical_controller};
use crate::{ControllerObservation, JournalStore, plan_takeover};

const OUTPUT_INF: &str = "directhci-winusb.inf";
const OUTPUT_CAT: &str = "directhci-winusb.cat";

#[derive(Clone, Debug, Serialize)]
pub struct ControllerPreparationStatus {
    pub hardware_id: String,
    pub ready: bool,
    pub takeover_safe: bool,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreparedController {
    pub status: ControllerPreparationStatus,
    pub already_prepared: bool,
    pub staged_inf: Option<String>,
}

#[repr(C)]
struct WdiDeviceInfo {
    next: *mut WdiDeviceInfo,
    vid: u16,
    pid: u16,
    is_composite: i32,
    mi: u8,
    desc: *mut c_char,
    driver: *mut c_char,
    device_id: *mut c_char,
    hardware_id: *mut c_char,
    compatible_id: *mut c_char,
    upper_filter: *mut c_char,
    driver_version: u64,
}

#[repr(C)]
struct WdiPrepareOptions {
    driver_type: i32,
    vendor_name: *mut c_char,
    device_guid: *mut c_char,
    disable_cat: i32,
    disable_signing: i32,
    cert_subject: *mut c_char,
    use_wcid_driver: i32,
    external_inf: i32,
}

type WdiPrepare = unsafe extern "system" fn(
    *mut WdiDeviceInfo,
    *const c_char,
    *const c_char,
    *mut WdiPrepareOptions,
) -> i32;

struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

pub fn controller_preparation_status(
    controller_id: &str,
) -> Result<ControllerPreparationStatus, String> {
    let plan = plan_takeover(controller_id);
    let controller = plan
        .rebind
        .controller
        .as_ref()
        .ok_or_else(|| format!("controller cannot be resolved: {:?}", plan.blockers))?;
    let package = eligible_device_specific_package(controller)?;
    let hardware_id = package
        .supported_hardware_ids
        .into_iter()
        .next()
        .ok_or("missing exact Hardware ID")?;
    Ok(ControllerPreparationStatus {
        hardware_id,
        ready: matches!(
            plan.rebind.directhci_package,
            DirectHciPackageReadiness::Ready { .. }
        ),
        takeover_safe: plan.safe_to_execute,
        blockers: plan.blockers.iter().cloned().collect(),
    })
}

pub fn prepare_controller(controller_id: &str) -> Result<PreparedController, String> {
    let before = unique_controller(controller_id)?;
    let package = eligible_device_specific_package(&before)?;
    let hardware_id = package
        .supported_hardware_ids
        .first()
        .ok_or("missing exact Hardware ID")?;
    let blueprint = device_specific_package_blueprint(hardware_id)?;
    let previous = controller_preparation_status(controller_id)?;
    if previous.ready {
        return Ok(PreparedController {
            status: previous,
            already_prepared: true,
            staged_inf: None,
        });
    }
    let journal = JournalStore::program_data().map_err(|error| error.to_string())?;
    if !matches!(journal.load(), Ok(crate::JournalLoad::Missing)) {
        return Err("RecoveryRequired: unresolved or unreadable ownership journal".into());
    }
    let (vid, pid, mi) = parse_usb_id(&blueprint.hardware_id)?;
    let root = JournalStore::program_data()
        .map_err(|error| error.to_string())?
        .path()
        .parent()
        .ok_or("journal path has no parent")?
        .to_owned();
    crate::security::ensure_secure_journal_directory(&root)?;
    let drivers = secure_child(&root, "drivers")?;
    let generated = secure_child(&drivers, "generated")?;
    let device_dir = secure_child(&generated, &blueprint.package_key)?;
    let attempt = format!("package-{}", new_uuid()?);
    let output = secure_child(&device_dir, &attempt)?;
    let template_dir = secure_child(&output, "template")?;
    let source = template_dir.join(OUTPUT_INF);
    fs::write(&source, blueprint.inf.as_bytes())
        .map_err(|error| format!("PackageGenerationFailed: write INF template: {error}"))?;
    crate::security::validate_secure_data_file(&source)?;

    // The DLL is loaded only from the protected service executable directory.
    // The GUI obtains explicit consent before issuing this privileged request.
    let executable =
        std::env::current_exe().map_err(|error| format!("ProvisioningUnavailable: {error}"))?;
    crate::security::validate_service_executable(&executable)?;
    let dll = executable
        .parent()
        .ok_or("service executable has no parent")?
        .join("libwdi.dll");
    crate::security::validate_secure_data_file(&dll)
        .map_err(|error| format!("ProvisioningUnavailable: untrusted libwdi.dll: {error}"))?;
    let dll_wide = wide(&dll);
    let module = Library(
        unsafe {
            LoadLibraryExW(
                PCWSTR(dll_wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map_err(|error| format!("ProvisioningUnavailable: load libwdi.dll: {error}"))?,
    );
    let proc = unsafe { GetProcAddress(module.0, PCSTR(b"wdi_prepare_driver\0".as_ptr())) }
        .ok_or("ProvisioningUnavailable: libwdi.dll lacks wdi_prepare_driver")?;
    let prepare: WdiPrepare = unsafe { std::mem::transmute(proc) };
    let path = c_path(&output)?;
    let input = c_path(&source)?;
    let description =
        CString::new("DirectHCI WinUSB Bluetooth Controller (Device-Specific)").unwrap();
    let current_driver = CString::new("BTHUSB").unwrap();
    let exact_id =
        CString::new(blueprint.hardware_id.as_str()).map_err(|_| "invalid Hardware ID")?;
    let vendor = CString::new("DirectHCI Project").unwrap();
    let guid = CString::new(crate::DIRECTHCI_WINUSB_INTERFACE_GUID).unwrap();
    let subject_text = format!("CN=DirectHCI-{}", new_uuid()?);
    let subject = CString::new(subject_text.as_str()).unwrap();
    // Record the certificate identity durably before libwdi creates trust.
    // A crash can then be diagnosed without guessing which certificate belongs
    // to this package. This file is not the M1 ownership journal.
    let metadata = serde_json::json!({
        "phase": "preparing",
        "hardware_id": &blueprint.hardware_id,
        "original_instance_id": &before.identity.instance_id,
        "certificate_subject": &subject_text,
        "package_version": env!("CARGO_PKG_VERSION"),
    });
    let mut metadata_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("metadata.json"))
        .map_err(|error| format!("create provisioning metadata: {error}"))?;
    metadata_file
        .write_all(&serde_json::to_vec_pretty(&metadata).map_err(|error| error.to_string())?)
        .and_then(|_| metadata_file.sync_all())
        .map_err(|error| format!("persist provisioning metadata: {error}"))?;
    drop(metadata_file);
    let mut device = WdiDeviceInfo {
        next: std::ptr::null_mut(),
        vid,
        pid,
        is_composite: i32::from(mi.is_some()),
        mi: mi.unwrap_or(0),
        desc: description.as_ptr() as *mut c_char,
        driver: current_driver.as_ptr() as *mut c_char,
        device_id: std::ptr::null_mut(),
        hardware_id: exact_id.as_ptr() as *mut c_char,
        compatible_id: std::ptr::null_mut(),
        upper_filter: std::ptr::null_mut(),
        driver_version: 0,
    };
    let mut options = WdiPrepareOptions {
        driver_type: 0, // WDI_WINUSB
        vendor_name: vendor.as_ptr() as *mut c_char,
        device_guid: guid.as_ptr() as *mut c_char,
        disable_cat: 0,
        disable_signing: 0,
        cert_subject: subject.as_ptr() as *mut c_char,
        use_wcid_driver: 0,
        external_inf: 1,
    };
    validate_unchanged(&before, controller_id)?;
    // SAFETY: all C strings and both repr(C) structures outlive this synchronous
    // call; the DLL stays loaded until it returns. No wdi_install_driver call.
    let code = unsafe { prepare(&mut device, path.as_ptr(), input.as_ptr(), &mut options) };
    if code != 0 {
        return Err(format!(
            "PackageSigningFailed: libwdi wdi_prepare_driver returned {code}; inspect directhcid diagnostics"
        ));
    }
    drop(module);
    let inf = output.join(OUTPUT_INF);
    let cat = output.join(OUTPUT_CAT);
    for file in [&inf, &cat] {
        let metadata = fs::metadata(file)
            .map_err(|error| format!("PackageVerificationFailed: {}: {error}", file.display()))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(format!(
                "PackageVerificationFailed: missing or empty {}",
                file.display()
            ));
        }
        crate::security::validate_secure_data_file(file)?;
    }
    validate_unchanged(&before, controller_id)?;
    let inf_wide = wide(&inf);
    // SetupCopyOEMInfW stages this signed package only. It does not bind the
    // controller; DiInstallDevice remains behind the existing takeover gates.
    if let Err(error) = unsafe {
        SetupCopyOEMInfW(
            PCWSTR(inf_wide.as_ptr()),
            PCWSTR::null(),
            SPOST_PATH,
            SP_COPY_STYLE(0),
            None,
            None,
            None,
        )
    } {
        let binding = validate_unchanged(&before, controller_id);
        return Err(format!(
            "PackageStagingRejectedByWindows: {error}; binding: {}; one-time public certificate {subject_text} remains trusted because safe reference-aware cleanup is not yet available",
            binding.map_or_else(|error| error, |()| "BTHUSB unchanged".into()),
        ));
    }
    validate_unchanged(&before, controller_id)?;
    let status = controller_preparation_status(controller_id)?;
    if !status.ready {
        return Err(format!(
            "PackageVerificationFailed: Driver Store accepted the package but fresh compatible-driver enumeration has no unique DirectHCI candidate: {:?}",
            status.blockers
        ));
    }
    let staged = serde_json::json!({
        "phase": "staged",
        "hardware_id": &blueprint.hardware_id,
        "certificate_subject": &subject_text,
        "staged_inf": inf.to_string_lossy(),
    });
    let mut staged_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("staged.json"))
        .map_err(|error| format!("create staged package record: {error}"))?;
    staged_file
        .write_all(&serde_json::to_vec_pretty(&staged).map_err(|error| error.to_string())?)
        .and_then(|_| staged_file.sync_all())
        .map_err(|error| format!("persist staged package record: {error}"))?;
    Ok(PreparedController {
        status,
        already_prepared: false,
        staged_inf: Some(inf.display().to_string()),
    })
}

fn unique_controller(id: &str) -> Result<ControllerObservation, String> {
    let matches: Vec<_> = crate::enumerate_controllers()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|item| item.id.as_str().eq_ignore_ascii_case(id))
        .collect();
    match matches.as_slice() {
        [item] => Ok(item.clone()),
        [] => Err("ControllerNotEligible: controller not found".into()),
        _ => Err("ControllerNotEligible: controller identity is ambiguous".into()),
    }
}

fn validate_unchanged(before: &ControllerObservation, id: &str) -> Result<(), String> {
    let after = unique_controller(id)?;
    if !same_physical_controller(&before.identity, &after.identity)
        || !after.status.present
        || after.status.problem_code.is_some_and(|code| code != 0)
        || !after.service.as_deref().is_some_and(|service| {
            service.eq_ignore_ascii_case("BTHUSB") || service.eq_ignore_ascii_case("IBTUSB")
        })
        || !after.driver.inf_path.eq(&before.driver.inf_path)
    {
        return Err(
            "controller identity or Windows Bluetooth binding changed during preparation".into(),
        );
    }
    Ok(())
}

fn secure_child(parent: &Path, name: &str) -> Result<PathBuf, String> {
    crate::security::reject_reparse(parent)?;
    let child = parent.join(name);
    match fs::create_dir(&child) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create protected provisioning directory: {error}")),
    }
    crate::security::validate_secure_data_file(&child)?;
    Ok(child)
}

fn parse_usb_id(id: &str) -> Result<(u16, u16, Option<u8>), String> {
    let parts: Vec<_> = id
        .strip_prefix("USB\\VID_")
        .ok_or("invalid USB Hardware ID")?
        .split('&')
        .collect();
    let vid = u16::from_str_radix(parts[0], 16).map_err(|_| "invalid USB VID")?;
    let pid = u16::from_str_radix(
        parts
            .get(1)
            .and_then(|value| value.strip_prefix("PID_"))
            .ok_or("invalid USB PID")?,
        16,
    )
    .map_err(|_| "invalid USB PID")?;
    let mi = parts
        .get(2)
        .map(|value| {
            u8::from_str_radix(value.strip_prefix("MI_").ok_or("invalid USB MI")?, 16)
                .map_err(|_| "invalid USB MI")
        })
        .transpose()?;
    Ok((vid, pid, mi))
}

fn new_uuid() -> Result<String, String> {
    let mut guid = GUID::zeroed();
    let status = unsafe { UuidCreate(&mut guid) };
    if status.0 != 0 && status.0 != 1824 {
        return Err(format!("generate package UUID: RPC status {}", status.0));
    }
    Ok(format!("{guid:?}")
        .trim_matches(['{', '}'])
        .to_ascii_lowercase())
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "path contains NUL".into())
}
