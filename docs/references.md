# Reference implementations and dependencies

No third-party implementation source has been copied into DirectHCI. Minimal
public ABI declarations that are required for FFI are recorded explicitly
below.

The M1 implementation additionally follows Microsoft's documented
`DiInstallDevice` `NeedReboot` contract, uses
[`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)
with replace/write-through flags for the Windows journal commit, and uses the
official [`Inf2Cat`](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/inf2cat)
and [driver test-signing](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing-driver-packages)
tool flow. These are API/tool contracts, not copied implementation code.

| Source | Exact reference | Intended use | License boundary / derived code |
| --- | --- | --- | --- |
| Microsoft Windows API documentation and `windows-rs` | [Container IDs](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/container-ids), [WinUSB installation](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/automatic-installation-of-winusb), [custom WinUSB package](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation), [WinUSB initialization](https://learn.microsoft.com/en-us/windows/win32/api/winusb/nf-winusb-winusb_initialize), [driver store](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/driver-store), [driver selection](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/overview-of-the-driver-selection-process), [`DiInstallDevice`](https://learn.microsoft.com/en-us/windows/win32/api/newdev/nf-newdev-diinstalldevice), [instance-specific installation guidance](https://learn.microsoft.com/en-us/previous-versions/windows/drivers/install/functions-that-simplify-driver-installation), [`SetupDiBuildDriverInfoList`](https://learn.microsoft.com/en-us/windows/win32/api/setupapi/nf-setupapi-setupdibuilddriverinfolist), [`SetupDiGetDriverInstallParams`](https://learn.microsoft.com/en-us/windows/win32/api/setupapi/nf-setupapi-setupdigetdriverinstallparamsw), [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw), [PnPUtil](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/pnputil-command-syntax), [InfVerif](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/running-infverif-from-the-command-line), [Inf2Cat](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/inf2cat), [SignTool](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/signtool), and [test-signing packages](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing-driver-packages); `windows` 0.62.x API bindings | Direct dependency for SetupAPI planning, exact-devnode `DiInstallDevice` binding/recovery, atomic journal replacement, and direct WinUSB readiness | Documentation/API use; `windows-rs` is MIT OR Apache-2.0; no copied implementation |
| UsbDk v1.00-22 | [`UsbDkData.h`](https://github.com/daynix/UsbDk/blob/v1.00-22/UsbDk/UsbDkData.h), [`UsbDkHelper.h`](https://github.com/daynix/UsbDk/blob/v1.00-22/UsbDkHelper/UsbDkHelper.h), [identity provenance](https://github.com/daynix/UsbDk/blob/v1.00-22/UsbDk/DeviceAccess.cpp), [installation/re-enumeration notes](https://github.com/daynix/UsbDk/wiki/Troubleshooting-UsbDk-installation), and [Bluetooth power-policy WDF_VIOLATION report #115](https://github.com/daynix/UsbDk/issues/115) | Retained read-only probe and experimental backend research only; current host has neither helper DLL nor service | Apache-2.0; minimal ABI declarations are represented in Rust with attribution. No implementation copied. Public risk evidence prevents treating redirect as the default or production-safe backend; it does not establish universal incompatibility |
| libusb / rusb | [Windows backend source](https://github.com/libusb/libusb/tree/master/libusb/os), [Windows guidance and restrictions](https://github.com/libusb/libusb/wiki/Windows), [libusb license](https://github.com/libusb/libusb/blob/master/COPYING), and [rusb](https://github.com/a1ien/rusb) | Mature possible optional transport; reference for async transfer/cancellation and documented WinUSB limitations. Direct WinUSB remains the default | libusb LGPL-2.1-or-later; rusb MIT; no code copied; not a current core dependency |
| nusb | [nusb Windows backend documentation](https://github.com/kevinmehall/nusb/blob/main/src/lib.rs) | Dedicated WinUSB transport candidate | MIT OR Apache-2.0; no code copied; does not provide runtime capture |
| libwdi / Zadig | [libwdi README](https://github.com/pbatard/libwdi), [API usage](https://github.com/pbatard/libwdi/wiki/Usage), [Zadig guide](https://github.com/pbatard/libwdi/wiki/Zadig), and [library license notice](https://github.com/pbatard/libwdi/blob/master/libwdi/libwdi.c) | Zadig is the recommended external developer provisioning tool; libwdi is only a possible later installer adapter | libwdi library is LGPL-3.0-or-later; Zadig is GPL-3.0. Neither is embedded or copied. Production provisioning should prefer DirectHCI's own signed INF/package flow |
| BTstack Windows WinUSB/Intel ports | [`hci_transport_h2_winusb.c`](https://github.com/bluekitchen/btstack/blob/master/platform/windows/hci_transport_h2_winusb.c) and [license](https://github.com/bluekitchen/btstack/blob/master/LICENSE) | Reference direct WinUSB open, descriptor-driven pipe discovery, control/interrupt/bulk mapping, overlapped I/O, abort, and handle teardown | Non-commercial default license with separate commercial licensing; reference only, no code copied or derived |
| Dolphin Bluetooth passthrough | [`LibUSBBluetoothAdapter.cpp`](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/Core/IOS/USB/Bluetooth/LibUSBBluetoothAdapter.cpp) and [`BTReal.cpp`](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/Core/IOS/USB/Bluetooth/BTReal.cpp) | Reference libusb async input resubmission, command-credit flow, cancellation/draining, interface release, and Bluetooth USB mapping | GPL-2.0-or-later; reference only. No code, vendor initialization, or consumer-specific behavior copied |
| Linux `btusb` / `btintel` | [`btusb.c`](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btusb.c), [`btintel.c`](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btintel.c), [`btintel.h`](https://github.com/torvalds/linux/blob/master/drivers/bluetooth/btintel.h) | Reference known Intel state transitions and firmware behavior | GPL-2.0; no code copied or translated |
| BlueZ | [HCI userspace-channel documentation](https://github.com/bluez/bluez/wiki/HCI) | Reference exclusive userspace HCI semantics | Mixed GPL/LGPL; Linux-specific; no code copied |
| linux-firmware Intel files | [Intel firmware entries and license references](https://kernel.googlesource.com/pub/scm/linux/kernel/git/firmware/linux-firmware.git/+/refs/tags/20250211/WHENCE) | Possible external firmware source only | Separate Intel binary firmware terms; do not bundle by default |
| `bt-hci` 0.10.1 | [`Controller`, `ControllerCmdSync`, and `ControllerCmdAsync`](https://docs.rs/bt-hci/0.10.1/bt_hci/controller/index.html) | Direct dependency defining the typed HCI controller boundary used by `directhci-bt-hci` | MIT OR Apache-2.0; DirectHCI independently adapts its own IPC SDK and does not copy a transport implementation |
| TrouBLE (`trouble-host`) 0.8.0 | [repository and current examples](https://github.com/embassy-rs/trouble/tree/trouble-host-v0.8.0/examples), [crate source](https://docs.rs/crate/trouble-host/0.8.0/source/) | Direct dependency for GAP Central, L2CAP, ATT, GATT discovery/read/write, and notification/indication handling in the generic `directhci-ble` consumer | MIT OR Apache-2.0; used as an upstream dependency, not forked or copied into DirectHCI |

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
| libwdi | Possible future provisioning adapter; keep outside runtime core pending installer/license design |
| Zadig | External developer provisioning tool, not a dependency |
| BTstack | Behavioral reference only under its default license |
| Dolphin | Behavioral reference only because its implementation is GPL-2.0-or-later |

When implementation work uses a reference, add the exact repository revision,
file/function, observed behavior, and corresponding independent DirectHCI code
before merging that implementation.
