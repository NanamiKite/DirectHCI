# Windows host tests

Run hardware tests on a Windows host with administrator access and input
devices that do not depend on the controller under test. Keep offline recovery
available before changing a driver. Existing results are in
[compatibility.md](compatibility.md).

For each run, save the source commit, executable paths, Windows build,
controller identity, driver versions, package identity and complete command
output. Record failures as well as the final controller state.

The PowerShell examples use the installed CLI:

```powershell
$cli = "$env:ProgramFiles\DirectHCI\directhci.exe"
```

For a development build, set `$cli` to that build's executable. Replace quoted
`<controller-id>` placeholders with an ID from the current controller list.

## 1. Observe the controller

These commands can run without the service and do not change the device:

```powershell
& $cli controllers --direct --json
& $cli doctor --json
& $cli takeover plan '<controller-id>' --json
```

Confirm that the target appears once. Compare VID/PID, instance ID, Container
ID, location, parent, service, INF, driver version and problem code with
Device Manager. Repeat discovery after a normal reboot and record which
identity evidence remains stable.

The takeover plan should identify the exact devnode, current Windows driver,
compatible driver candidates and their ranks, and any recovery blockers.
Before preparation, the DirectHCI package may be reported as missing.

## 2. Prepare the WinUSB package

Use **Prepare Controller** in the installed panel, following
[installation.md](installation.md#prepare-a-controller). Save the preparation
result and repeat the read-only plan.

Check that:

- one expected DirectHCI package and a valid Windows restore candidate are
  available for the target;
- the DirectHCI candidate has a numerically worse rank than the Windows
  candidate;
- the controller remains bound to the Windows Bluetooth driver after staging
  and after a reboot;
- the journal directory is usable and no unresolved ownership journal remains;
- `directhci recover --offline` is available from an elevated terminal.

If Windows rejects staging, save the exact error and verify that the binding
is unchanged. Leave boot and signing policy unchanged during this test.
The [legacy development package](../driver/winusb-supported-devices/README.md)
is a separate diagnostic path; it is not required for normal preparation.

## 3. Test driver takeover and restore

Stop the service before running direct takeover diagnostics. From an elevated
PowerShell, with non-Bluetooth input and a recovery/reboot path available:

```powershell
& $cli recover --offline --json
& $cli takeover plan '<controller-id>' --json
& $cli takeover roundtrip '<controller-id>' --execute --json
& $cli controllers --direct --json
```

The roundtrip should bind the exact device to DirectHCI WinUSB, pass readiness,
then restore a freshly selected Windows driver. It sends no HCI traffic.
Verify the target identity, WinUSB application interface, E0/01/01 descriptor,
event/ACL pipes, and final working Windows Bluetooth state.

A different host, package or driver/security configuration needs its own
roundtrip. If it fails, preserve the journal and use the
[recovery procedure](installation.md#recovery) before proceeding. Interruption
and kill-point tests come after a successful basic roundtrip; the expected
outcomes are listed in
[temporary-rebind.md](temporary-rebind.md#implemented-failure-behavior).

## 4. Test HCI and BLE sessions

Start the service, then run from an elevated PowerShell:

```powershell
& $cli hci-info '<controller-id>' --execute --json
& $cli status --json
& $cli controllers --json
```

Check HCI Reset, Local Version Information, session release and final
`WindowsOwned` state.

For the BLE regression test, use a known test peripheral and the
[BLE CLI](../crates/directhci-ble/README.md#cli). Run `connect`, `inspect`, and
`listen` or `listen-all` as separate sessions, saving their output. Use a
peripheral known to send unsolicited values for passive listening; use
`subscribe` if it requires a CCCD write. After each command, check for no
active HCI session and a restored Windows Bluetooth controller. Repeat after
Ctrl+C to exercise interrupted cleanup.

GATT timeout and disconnect regressions remain pending acceptance. A successful
scan alone does not cover these operations.

## 5. Test the installer and Control Panel

Build without `-SkipBuild` and install the resulting package. Test the installed
executables, recording their paths and hashes.

1. Launch the panel from a non-elevated Start Menu session. Verify the UAC
   prompt, successful launch on approval and exit on rejection.
2. Check service Start/Stop, controller selection and preference persistence.
   Verify status refreshes without clicking Refresh.
3. Prepare a controller and verify the certificate consent and preparation
   result. Confirm preparation leaves Windows Bluetooth bound.
4. Connect and release a client. Verify Bluetooth is restored while the service
   stays running, then connect a second client without restarting the service.
5. Stop the service during an active session. Verify confirmation, cleanup and
   restoration; the panel should remain open.
6. Restart the service and close the panel. Verify it waits for SCM `Stopped`
   before exiting. Minimize separately: the tray icon should restore the
   panel, and minimizing should leave the service running.
7. Test upgrade and uninstall. An unresolved recovery failure should retain
   the service, binaries and journal instead of deleting them.

## Optional: a dedicated WinUSB dongle

Use a separate, expendable USB controller and follow
[dedicated-controller.md](dedicated-controller.md). Keep the system controller
Windows-owned. After manual provisioning, run `doctor --json` and check fresh
identity correlation, a unique registered application interface, overlapped
open, WinUSB initialization, E0/01/01 and exactly one interrupt IN, bulk IN
and bulk OUT pipe. Close all handles after the probe.

A separate dedicated dongle has not completed hardware acceptance. Its
readiness and HCI results should be recorded separately from temporary takeover.

## Optional: UsbDk research

The `doctor` UsbDk probe is read-only. A missing helper DLL or service is an
expected environment result and does not require installing UsbDk.

UsbDk redirect testing is outside this plan. It requires an isolated,
recoverable Windows machine, non-Bluetooth input, recovery media and crash
capture. The primary AX201 development host is excluded from UsbDk installation
and redirect experiments. Background and upstream references are in
[ownership-survey.md](ownership-survey.md#c-usbdk-runtime-capture--experimental).
