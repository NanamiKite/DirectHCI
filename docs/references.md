# Reference implementations and dependencies

No third-party implementation source has been copied into DirectHCI. Minimal
public ABI declarations that are required for FFI are recorded explicitly
below.

DirectHCI's first-party code currently uses
[GPL-3.0-only](../LICENSE.txt). Dependencies and external tools retain the
licenses listed below.

The temporary-rebind implementation follows Microsoft's documented
`DiInstallDevice` `NeedReboot` contract and uses
[`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)
with replace/write-through flags for the Windows journal commit. The legacy
development driver script uses the official
[`Inf2Cat`](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/inf2cat)
and [driver test-signing](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing-driver-packages)
tool flow; the current on-demand preparation path uses pinned libwdi instead.
These are API/tool contracts, not copied implementation code.

The former class-compatible-ID INF (`USB\Class_E0&SubClass_01&Prot_01`)
failed Inf2Cat B2.6.4.9 on the Windows build host, so DirectHCI no
longer builds or stages it. Microsoft's
[standard USB identifier rules](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/standard-usb-identifiers)
and [INF Models-section rules](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/inf-models-section)
inform the replacement: one exact USB PnP Hardware ID in each dynamically
generated package, using the WinUSB install section. The
[WinUSB custom-INF guidance](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation)
supplies the in-box `WinUSB.sys` Include/Needs structure and `USBDevice`
setup class. A sourced Hardware ID is not proof of DirectHCI readiness;
firmware, descriptor and restore behavior still require host verification.
No Microsoft source code was copied.

| Source | Exact reference | Intended use | License boundary / derived code |
| --- | --- | --- | --- |
| Microsoft Windows API documentation and `windows-rs` | [Container IDs](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/container-ids), [WinUSB installation](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/automatic-installation-of-winusb), [custom WinUSB package](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation), [WinUSB initialization](https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_initialize), [driver store](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/driver-store), [driver selection](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/overview-of-the-driver-selection-process), [`DiInstallDevice`](https://learn.microsoft.com/en-us/windows/win32/api/newdev/nf-newdev-diinstalldevice), [instance-specific installation guidance](https://learn.microsoft.com/en-us/previous-versions/windows/drivers/install/functions-that-simplify-driver-installation), [`SetupDiBuildDriverInfoList`](https://learn.microsoft.com/en-us/windows/win32/api/setupapi/nf-setupapi-setupdibuilddriverinfolist), [`SetupDiGetDriverInstallParams`](https://learn.microsoft.com/en-us/windows/win32/api/setupapi/nf-setupapi-setupdigetdriverinstallparamsw), [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw), [PnPUtil](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/pnputil-command-syntax), [InfVerif](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/running-infverif-from-the-command-line), [Inf2Cat](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/inf2cat), [SignTool](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/signtool), and [test-signing packages](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing-driver-packages); `windows` 0.62.x API bindings | Direct dependency for SetupAPI planning, exact-devnode `DiInstallDevice` binding/recovery, atomic journal replacement, and direct WinUSB readiness | Documentation/API use; `windows-rs` is MIT OR Apache-2.0; no copied implementation |
| UsbDk v1.00-22 | [`UsbDkData.h`](https://github.com/daynix/UsbDk/blob/v1.00-22/UsbDk/UsbDkData.h), [`UsbDkHelper.h`](https://github.com/daynix/UsbDk/blob/v1.00-22/UsbDkHelper/UsbDkHelper.h), [identity provenance](https://github.com/daynix/UsbDk/blob/v1.00-22/UsbDk/DeviceAccess.cpp), [installation/re-enumeration notes](https://github.com/daynix/UsbDk/wiki/Troubleshooting-UsbDk-installation), and [Bluetooth power-policy WDF_VIOLATION report #115](https://github.com/daynix/UsbDk/issues/115) | Retained read-only probe and experimental backend research only; current host has neither helper DLL nor service | Apache-2.0; minimal ABI declarations are represented in Rust with attribution. No implementation copied. Public risk evidence prevents treating redirect as the default or production-safe backend; it does not establish universal incompatibility |
| libusb / rusb | [Windows backend source](https://github.com/libusb/libusb/tree/master/libusb/os), [Windows guidance and restrictions](https://github.com/libusb/libusb/wiki/Windows), [libusb license](https://github.com/libusb/libusb/blob/master/COPYING), and [rusb](https://github.com/a1ien/rusb) | Mature possible optional transport; reference for async transfer/cancellation and documented WinUSB limitations. Direct WinUSB remains the default | libusb LGPL-2.1-or-later; rusb MIT; no code copied; not a current core dependency |
| nusb | [nusb Windows backend documentation](https://github.com/kevinmehall/nusb/blob/main/src/lib.rs) | Dedicated WinUSB transport candidate | MIT OR Apache-2.0; no code copied; does not provide runtime capture |
| libwdi / Zadig | [libwdi README](https://github.com/pbatard/libwdi), [API usage](https://github.com/pbatard/libwdi/wiki/Usage), [Zadig guide](https://github.com/pbatard/libwdi/wiki/Zadig), and [library license notice](https://github.com/pbatard/libwdi/blob/master/libwdi/libwdi.c) | Pinned libwdi 1.5.1 is bundled for exact-HWID INF/CAT generation and one-time self-signing; DirectHCI uses SetupAPI only for stage-only installation. Zadig remains external. | libwdi is LGPL-3.0-or-later. The build produces a source archive; the current installer includes only the DLL. See the distribution note below. |
| BTstack Windows WinUSB/Intel ports | [`hci_transport_h2_winusb.c`](https://github.com/bluekitchen/btstack/blob/master/platform/windows/hci_transport_h2_winusb.c) and [license](https://github.com/bluekitchen/btstack/blob/master/LICENSE) | Reference direct WinUSB open, descriptor-driven pipe discovery, control/interrupt/bulk mapping, overlapped I/O, abort, and handle teardown | Non-commercial default license with separate commercial licensing; reference only, no code copied or derived |
| Dolphin Bluetooth passthrough | [`LibUSBBluetoothAdapter.cpp`](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/Core/IOS/USB/Bluetooth/LibUSBBluetoothAdapter.cpp) and [`BTReal.cpp`](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/Core/IOS/USB/Bluetooth/BTReal.cpp) | Reference libusb async input resubmission, command-credit flow, cancellation/draining, interface release, and Bluetooth USB mapping | GPL-2.0-or-later; reference only. No code, vendor initialization, or consumer-specific behavior copied |
| Linux `btusb` / `btintel` | [`btusb.c`](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btusb.c), [`btintel.c`](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btintel.c), [`btintel.h`](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btintel.h) | Reference known Intel state transitions and firmware behavior | GPL-2.0; no code copied or translated |
| BlueZ | [HCI userspace-channel documentation](https://github.com/bluez/bluez/wiki/HCI) | Reference exclusive userspace HCI semantics | Mixed GPL/LGPL; Linux-specific; no code copied |
| linux-firmware Intel files | [Intel firmware entries and license references](https://kernel.googlesource.com/pub/scm/linux/kernel/git/firmware/linux-firmware.git/+/refs/tags/20250211/WHENCE) | Possible external firmware source only | Separate Intel binary firmware terms; do not bundle by default |
| `bt-hci` 0.10.1 | [`Controller`, `ControllerCmdSync`, and `ControllerCmdAsync`](https://docs.rs/bt-hci/0.10.1/bt_hci/controller/index.html) | Direct dependency defining the typed HCI controller boundary used by `directhci-bt-hci` | MIT OR Apache-2.0; DirectHCI independently adapts its own IPC SDK and does not copy a transport implementation |
| TrouBLE (`trouble-host`) 0.8.0 | [repository and current examples](https://github.com/embassy-rs/trouble/tree/trouble-host-v0.8.0/examples), [crate source](https://docs.rs/crate/trouble-host/0.8.0/source/) | Direct dependency behind the reusable `directhci-ble` library for GAP Central, L2CAP, ATT, GATT discovery/read/write, and notification/indication handling; `apps/directhci-ble` is only its CLI frontend | MIT OR Apache-2.0; used as an upstream dependency, not forked or copied into DirectHCI |

## Control Panel and installer references

- [Native Windows GUI](https://github.com/gabdube/native-windows-gui/blob/master/readme.md)
  1.0.13 is a direct MIT-licensed dependency of
  `apps/directhci-control-panel`; DirectHCI uses its native controls rather
  than copying GUI framework code.
- [Inno Setup](https://github.com/jrsoftware/issrc/blob/HEAD/license.txt)
  is an external installer compiler, not a runtime dependency. The project
  owns `installer/directhci.iss` and does not copy Inno Setup implementation
  code. Its license is the Inno Setup License.
- [Inno Setup `PrivilegesRequired`](https://jrsoftware.org/ishelp/topic_setup_privilegesrequired.htm)
  controls Setup's admin install mode.
  [Microsoft's UAC guidance](https://learn.microsoft.com/en-us/windows/win32/secbp/running-with-administrator-privileges)
  explains that a later GUI process has its own execution level. The Control
  Panel currently checks the effective token and uses the documented
  [`ShellExecuteW` `runas` verb](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecutew)
  when elevation is needed. Acceptance status is tracked in
  [compatibility.md](compatibility.md).

- [`bt-hci-usb` 0.2.0](https://github.com/embassy-rs/bt-hci/tree/main/bt-hci-usb)
  is an upstream USB HCI transport in the same ecosystem. Its README requires
  WinUSB binding on Windows. DirectHCI does not depend on it; the project
  implements driver rebind, recovery and service IPC around its own transport.

## Distribution files

`build-libwdi.ps1` produces the DLL and pinned source archive in
`%LOCALAPPDATA%\DirectHCI\native`. The build script in this repository records
the patches applied to that source. `build-installer.ps1` requires the archive
and build manifest, but `installer/directhci.iss` currently installs only the
DLL. Include the corresponding source, build script and required
notices when preparing binary distributions; see [LICENSE.txt](../LICENSE.txt)
and the dependency license links above.

## Confirmed compatibility notes

- `bt-hci` 0.10.1 declares `link_control::Disconnect` (opcode `0x0406`)
  as `SyncCmd<Return = ()>`. The Bluetooth HCI procedure and the observed
  Intel AX201 behavior acknowledge this command with Command Status, followed
  later by the unsolicited Disconnection Complete event. The
  `directhci-bt-hci` adapter therefore accepts a successful Command Status only
  for opcode `0x0406`; all other `SyncCmd` implementations still require
  Command Complete. This compatibility branch should be removed when the
  upstream command model represents Disconnect as an asynchronous command.

The Dedicated WinUSB readiness implementation uses only Microsoft-documented
API contracts through `windows-rs`: `SetupDiOpenDevRegKey` reads the
provisioned `DeviceInterfaceGUID` / `DeviceInterfaceGUIDs` value;
`SetupDiEnumDeviceInterfaces` and `SetupDiGetDeviceInterfaceDetailW` resolve
the application path for the same PnP instance; `CreateFileW` uses
`FILE_FLAG_OVERLAPPED`; and `WinUsb_Initialize`,
`WinUsb_QueryInterfaceSettings`, `WinUsb_QueryPipe`, and `WinUsb_Free`
perform the open/inspect/close probe. No reference implementation code was used.

## Dependency classification for Dedicated WinUSB


| Component | DirectHCI decision |
| --- | --- |
| Microsoft WinUSB API through `windows-rs` | Direct dependency; preferred transport implementation |
| libusb | Technically usable and mature; reserve as an optional fallback, not the default core dependency |
| libwdi | Pinned native provisioning dependency in the privileged Windows service; never used for takeover, rebind, or Raw HCI |
| Zadig | External developer provisioning tool, not a dependency |
| BTstack | Behavioral reference only under its default license |
| Dolphin | Behavioral reference only because its implementation is GPL-2.0-or-later |

When implementation work uses a reference, add the exact repository revision,
file/function, observed behavior, and corresponding independent DirectHCI code
before merging that implementation.
