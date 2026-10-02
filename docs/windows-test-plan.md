# Windows host test plan

VM compilation is not hardware validation. Results from this plan must be
recorded as Windows host observations with Windows build, driver versions,
backend version, controller identity evidence, and exact result.

## Stage 1: read-only observation

1. Run `directhci controllers --json` without elevation.
2. Confirm the system Bluetooth controller appears exactly once.
3. Compare VID/PID, instance ID, Container ID, location paths, parent, INF,
   service, driver provider/version, status, and problem code with Device
   Manager or `pnputil /enum-devices /connected /deviceids /drivers`.
4. Run the command repeatedly and after a normal reboot. Record which identity
   evidence remains stable.
5. Confirm no device is disabled, restarted, rebound, or removed.

Observed on the current host: the Intel AX201 Bluetooth USB function is
`8087:0026` and uses `BTHUSB`. The system/sentinel Container ID is normalized
away; location topology is the current identity basis.

Its instance is `USB\\VID_8087&PID_0026\\5&310905D1&0&14`. This is the system
controller and is permanently excluded from Dedicated Mode provisioning. It is
the explicit M1 takeover target. The first temporary-rebind safety gate was
completed for the recorded host; reapply it before a new destructive run,
package revision, or different machine.

## Stage 2: read-only UsbDk compatibility gate

Completed host observation:

```text
UsbDkHelper.dll: missing
UsbDk service: missing
enumeration: not attempted
correlation: not evaluated
```

This is an environment result, not a DirectHCI probe error. Do not install
UsbDk merely to change it. The read-only adapter and `doctor` diagnostics remain
available for machines where the runtime is already present.

UsbDk is now an experimental takeover candidate. Its installer registers a
system USB filter and triggers machine-wide USB re-enumeration. A public issue
also reports `WDF_VIOLATION` around `StartRedirect` for a power-managed
Bluetooth USB device on Windows 11, with related Windows 10 reports. Neither
safety nor universal incompatibility has been established.

No UsbDk install, uninstall, filter change, `StartRedirect`, or `StopRedirect`
is authorized on the primary development host by this plan.

## Stage 3: temporary-rebind read-only preflight

Run, without elevation if the host permits read-only driver-list access:

```text
directhci controllers --json
directhci doctor --json
directhci takeover plan <AX201-controller-id> --json
```

The plan must freshly resolve exactly one AX201 devnode, report `BTHUSB` and
the current Intel package, enumerate applicable compatible-driver packages and
ranks, and apply no device changes. Before the development package is staged,
`directhci_package` should be `missing`; journal writability and the built-in
offline recovery entry point are checked independently. This stage never
installs, binds, disables, enables, or restarts a device.

Package creation, staging, transaction ordering, reconciliation, and the first
destructive test gate are specified in
[`temporary-rebind.md`](temporary-rebind.md). A staged package is not sufficient
authorization to bind it.

## Auxiliary: Dedicated WinUSB provisioning

Not yet executed because no separate controller has been supplied. The driver
change is performed only by the user and applies only to a separate,
expendable USB Bluetooth controller—not the built-in AX201. Follow
[`dedicated-controller.md`](dedicated-controller.md).

Preconditions:

1. Keep the system controller Windows-owned and verify local input does not
   depend on the dedicated dongle.
2. Record the target controller's identity, location, current driver/INF, and
   USB class metadata before changing it.
3. Remove other identical dongles where practical and refuse ambiguous
   identity.
4. Use an administrator provisioning step, initially an audited manual Zadig
   selection of WinUSB. Do not install UsbDk or a filter driver.
5. Record whether installation requests reboot or unplug/replug.
6. Re-enumerate and verify the same physical controller now uses WinUSB while
   the AX201 remains on `BTHUSB`.

A production acceptance path will later replace the developer-only Zadig step
with a narrowly matched, signed DirectHCI WinUSB INF/catalog and a
device-specific installer workflow.

## Auxiliary: read-only Dedicated WinUSB readiness

Implemented in the Windows backend; hardware acceptance is pending a separate
WinUSB-provisioned Bluetooth dongle. Run `directhci doctor` (or
`directhci doctor --json`) without changing the AX201 binding. With no such
dongle, the expected result is `Dedicated WinUSB / candidates: 0`.

Completed no-device host baseline:

~~~text
controllers: 1 (Intel AX201, BTHUSB)
Dedicated WinUSB discovery: success
Dedicated WinUSB candidates: 0
AX201 readiness: not_win_usb_bound
~~~

After separately authorized provisioning:

1. Freshly enumerate the controller; do not reuse its old interface path.
2. Confirm controller identity remains correlated across the driver change.
3. Confirm the current service is `WinUSB`. Read `DeviceInterfaceGUID` or
   `DeviceInterfaceGUIDs` from that devnode's hardware key, enumerate those
   interface classes, and require an exact PnP instance-ID match. Do not use
   `GUID_DEVINTERFACE_USB_DEVICE` as an application interface.
4. Open its expected device interface with overlapped I/O.
5. Call `WinUsb_Initialize`, query interface descriptors and pipes, and require
   exactly one interrupt IN, one bulk IN, and one bulk OUT for the initial HCI
   transport.
6. Close all handles without sending a control, bulk, or interrupt transfer.
7. Verify the AX201 and Windows Bluetooth remain unaffected.

Multiple matching application paths are reported as ambiguous and are not
opened. A missing or invalid registration is a readiness failure, not a reason
to guess another interface GUID.

## Future auxiliary: Dedicated Raw HCI acceptance

Dedicated-mode HCI hardware acceptance is still future work because no
separate WinUSB-bound dongle was tested. M2 command/event/ACL transport has
instead been exercised through the temporary AX201 takeover. SCO and vendor
firmware initialization are separate capabilities, not implied by basic
HCI success.

## Stage 4: first takeover round-trip

The code path includes durable journal persistence, offline recovery,
device-specific `DiInstallDevice` selection, WinUSB readiness reuse, and
explicit Windows-driver restore. This gate was completed on the recorded host:
`BTHUSB/oem69.inf` → `WinUSB/oem183.inf` → readiness `Ready` →
`BTHUSB/oem69.inf`. Dedicated-controller validation was not required.
A changed package or different host must pass its own gate again.

The M1-only roundtrip binds only the exact AX201 devnode with
`DiInstallDevice`, re-observes and runs the existing WinUSB readiness check,
sends no HCI traffic, then explicitly binds a freshly selected non-DirectHCI
Windows candidate and verifies Windows Bluetooth. The
historical `oem69.inf` is evidence, not the desired state or an unconditional
restore instruction.

Build/stage and execute from an elevated PowerShell only after reviewing the
package and ensuring non-Bluetooth input and recovery access:

```powershell
.\scripts\windows\build-dev-driver.ps1 -CreateCertificate
# Manually establish the required certificate trust/signing policy, then rerun
# with -CertificateThumbprint if signature verification initially fails.
pnputil /add-driver "$env:LOCALAPPDATA\DirectHCI\driver\winusb-supported-devices\directhci-winusb-dev.inf"
.\directhci.exe controllers --json
.\directhci.exe takeover plan <AX201-ID> --json
.\directhci.exe recover --offline --json
.\directhci.exe takeover roundtrip <AX201-ID> --execute --json
```

The pre-roundtrip offline recovery command is a harmless no-op when no journal
exists. Never add `/install` to the staging command.

UsbDk may be reconsidered only in an isolated lab: a sacrificial or readily
recoverable Windows system, non-Bluetooth input, recovery media, crash
dumps/kernel debugging, explicitly recorded UsbDk/Windows/HVCI configuration,
and a noncritical controller. VM results are useful diagnostics but do not
prove physical USB-stack safety.

## Recorded M2/M3/BLE results and regression gate

Earlier AX201 host acceptance observed HCI Reset status `0x00`, Local
Version `0x0b` / revision `0x375b`, LE connect, ACL TX/RX through ATT MTU
exchange (MTU 131), GATT discovery, unsolicited notification reception,
clean disconnect, and `WindowsOwned` restore. These are recorded hardware
observations, not a guarantee for a later build.

After the BLE CLI implementation was extracted into `crates/directhci-ble`,
host reports showed GATT timeout and then disconnect regressions. Before
claiming the **current** BLE library build accepted, repeat only the
smallest sequence: connect, inspect, passive listen, then check
`directhci status` and `directhci controllers` for no active session and
`WindowsOwned`. Do not infer that Cargo compilation resolves a lifecycle
regression.

## Installer and Control Panel acceptance (pending)

Build a fresh installer without `-SkipBuild`; verify the installed binary
rather than a release-tree or Downloads copy. From a non-elevated Start Menu
launch, confirm Control Panel UAC approval is requested, SCM status is
displayed, service Start/Stop works, preferred controller persists, and the
panel refreshes service/controller/session status without a manual Refresh.
Confirm a normal client disconnect restores Windows Bluetooth while the
service stays Running, and a second client connection works without restarting
it. Confirm explicit Stop Service leaves the panel open; restart the service,
then close the panel and verify it waits for SCM Stopped before exiting.
Then separately check upgrade and safe uninstall. These packaging/UAC
checks have not yet been reported as passing on the Windows host.
