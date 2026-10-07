# Install and use DirectHCI on Windows

[简体中文](installation.zh-CN.md) · [Troubleshooting and safe recovery](troubleshooting.md)

DirectHCI targets Windows x64. The installer installs the runtime service,
Control Panel, command-line tools and the provisioning component. It does
**not** switch any Bluetooth controller while installing. End users do not
need EWDK, MSBuild or driver-signing tools.

The newer dynamic preparation and installer paths still need full Windows
host acceptance; see [compatibility](compatibility.md).

## Install and start

1. Close any foreground `directhcid run` process before installing so the
   installed service can own the named pipe.
2. Run `DirectHCI-Setup-<version>.exe` for the release you downloaded and approve administrator elevation.
   Programs are installed under `C:\Program Files\DirectHCI`.
3. Open **DirectHCI Control Panel** from the Start Menu. It requests elevation
   if needed; declining closes the panel.
4. Click **Start Service**. Select the USB Bluetooth controller you intend to
   use in **Preferred Controller**.

The service saves the chosen ControllerId in
`%ProgramData%\DirectHCI\config.json`. If exactly one controller is found and
no preference exists, it can select that controller automatically. If several
exist, choose one. If a saved controller is no longer present, the panel does
not silently select another. Preferences cannot change during an active
session.

If Device Manager shows Bluetooth but the panel lists no controller, follow
[the no-controller troubleshooting steps](troubleshooting.md#the-control-panel-lists-no-controller): this backend currently
enumerates eligible USB Bluetooth devnodes, not every Windows Bluetooth
radio or bus type.

## Prepare a controller

If the selected controller shows **Not prepared**, click **Prepare Controller**
and review the local certificate-trust confirmation. The service reads that
controller's actual PnP Hardware ID, generates a matching device-specific
WinUSB package, signs it with a one-time local certificate and asks Windows
to stage it in the Driver Store. A newly attached eligible USB controller can
be prepared without reinstalling DirectHCI.

**Prepare is not takeover.** It does not request a DirectHCI session or
replace the controller's active Windows Bluetooth driver. After successful
staging, the runtime checks that the Windows binding remains active and
reruns its safety planner. A later client acquisition is a separate operation.

Windows may reject a locally signed package under its code-integrity policy.
The preparation error is reported without initiating takeover. DirectHCI
does not change Secure Boot, TESTSIGNING or BCD. Follow
[the safe preparation-failure steps](troubleshooting.md#prepare-controller-is-rejected-by-windows); see
[driver provisioning internals](internals/driver-provisioning.md) for INF,
catalog, certificate and Driver Store details.

## Use the service

The Control Panel shows runtime status, selected controller, active client
and recovery state. The service is the controller owner; closing a client
session should restore Windows Bluetooth while leaving the service running.

| Action | Expected result |
| --- | --- |
| Client releases or disconnects | HCI session closes; runtime attempts and verifies Windows Bluetooth restore; service stays running |
| **Stop Service** | Stops the service and keeps the panel open |
| Close the panel or choose **Exit Control Panel** in the tray | Stops the service, waits for `Stopped`, then exits |
| Minimize the panel | Hides it in the tray; service keeps running |

Stopping with an active session requires confirmation. If stopping fails,
the panel stays open and shows the error. Click the tray icon to reopen it.
**A stopped service does not prove Bluetooth has recovered.** Check the actual
controller and Windows Bluetooth state.

The main service remains manual-start. Installation also registers
**DirectHCI Boot Recovery**, a SYSTEM task that runs `directhcid boot-recovery`
at boot and exits after checking any retained journal. It does not start the
interactive runtime or take over a controller. Failed recovery is recorded in
the Windows Application event log under `DirectHCI`. Upgrade/reinstall is
needed to register this task and the service's preshutdown timeout.

CLI status queries can use a normal PowerShell:

```powershell
$cli = Join-Path $env:ProgramFiles 'DirectHCI\directhci.exe'
& $cli status
& $cli controllers
```

Acquiring a controller or sending Raw HCI requires administrator membership
with the current pipe policy. For other commands see [CLI](cli.md), and for
Rust consumers see [SDK](sdk.md). `DirectHciClient::connect_or_start(...)` can
request service startup when the caller has Windows `SERVICE_START` permission;
`connect(...)` requires an already running service.

## Recovery after abnormal termination

**Known limitation: recovery of system Bluetooth after abnormal termination
is not yet fixed.** Unexpected power loss, a blue screen, replacement of
program files while the DirectHCI service is running, or abnormal process
termination can interrupt orderly restoration. Windows Bluetooth may then
remain off and refuse to turn on, even when Device Manager shows a healthy
controller.

Windows **Restart** may be needed to restore Bluetooth; shutdown and power-on
with Fast Startup are not equivalent. Restart is not a guaranteed fix. A
stopped service, an active BTHUSB driver or an absent journal alone does not
prove Bluetooth is usable. Before upgrading, disconnect clients, stop the
service and exit the applications normally; do not replace running files or
force-terminate processes. See [safe next steps after abnormal termination](troubleshooting.md#system-bluetooth-is-unusable-after-abnormal-termination).

## Recovery

If recovery is required while the runtime is available, use **Restore
Windows** in the panel. If the service cannot recover normally, stop it and
run offline recovery in an elevated PowerShell:

```powershell
$cli = Join-Path $env:ProgramFiles 'DirectHCI\directhci.exe'
& $cli recover --offline --json
```

Recovery re-enumerates the saved physical controller and the current driver
state; a stale journal is not by itself proof that the driver is still wrong.
It can clear a journal only after the required Windows-owned state is freshly
confirmed. Missing or ambiguous devices, unsafe journal storage and
unconfirmed restoration remain blocked. **Do not delete the journal to make
an error disappear**; it contains recovery evidence. Start with
[troubleshooting and safe recovery](troubleshooting.md); see
[ownership and recovery](ownership-and-recovery.md) and the
[failure model](failure-model.md).

If recovery requests a reboot, choose Windows **Restart**, not shutdown and
power-on with Fast Startup. Recovery compares the kernel boot identifier before
checking an old owner PID. New journals include the original device security
baseline; an older journal may require explicit repair if that baseline is
unknown. Do not widen permissions or delete evidence to force a successful result.

## Upgrade or uninstall

Run a new installer to upgrade, or use **Installed apps → DirectHCI →
Uninstall**. Before upgrading, disconnect active clients, stop the DirectHCI
service, confirm Windows Bluetooth recovery, and exit the Control Panel
(including its tray icon) and DirectHCI CLI tools. Setup refuses to open its
wizard if a DirectHCI process or a non-stopped service is detected, and rechecks
before recovery preflight and file replacement. A failed status query also
blocks installation. Setup does not terminate processes or stop a running
service for an upgrade. Keep DirectHCI closed throughout installation; these
checks are checkpoints, not a system-wide lock against starting another process.

Upgrade preflight still checks offline recovery before removing the stopped
service registration; normal uninstall retains its stop/recover path. If
recovery cannot be confirmed, removal is refused and recovery evidence is
retained. A stopped service or absent journal alone is not proof that the
Windows Bluetooth switch works; verify it before upgrading.

The current uninstaller does not remove previously staged WinUSB packages,
trusted public certificates or `%ProgramData%\DirectHCI` state. Their safe
cleanup requires separate, reference-aware maintenance; do not remove a
certificate before determining which packages use it.

## Build the installer

Developer build prerequisites, EWDK/libwdi steps, Rust compilation, Inno
Setup packaging and output directories are in
[development.md](development.md#build-the-installer). Building executables
alone does not install the service.
