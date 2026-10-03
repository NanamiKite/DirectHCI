# CLI

The installed `directhci.exe` is a diagnostic and recovery client. Normal
`status` and `controllers` requests use the local `directhcid` service. The
Control Panel is the normal path for service control, preferred-controller
selection and **Prepare Controller**.

In PowerShell, assign the path before using `&`:

```powershell
$cli = Join-Path $env:ProgramFiles 'DirectHCI\directhci.exe'
& $cli status --json
& $cli controllers --json
```

## Read-only diagnostics

| Command | Source / purpose |
| --- | --- |
| `status [--json]` | Runtime, active session, recovery state and controller states over IPC |
| `controllers [--json]` | Runtime controller list over IPC |
| `controllers --direct [--json]` | Fresh local USB Bluetooth enumeration without the service |
| `controller show <id> [--json]` | Local observation of one controller |
| `doctor [--json]` | Local diagnostic probes |
| `takeover plan <id> [--json]` | Local, read-only takeover preflight with blockers and driver candidates |

`<id>` is a ControllerId returned by current enumeration, not a display name
or VID/PID. `controllers --direct` currently lists eligible USB Bluetooth
candidates; it does not enumerate every Windows Bluetooth radio bus type.
A missing package in `takeover plan` can be addressed through **Prepare
Controller** in the Control Panel, subject to Windows signing policy.

## Commands that may change controller state

| Command | Use |
| --- | --- |
| `hci-info <id> --execute [--json]` | Acquire through the runtime, read basic HCI information, release and restore |
| `takeover roundtrip <id> --execute [--json]` | Direct development diagnostic: WinUSB readiness and restore without HCI I/O |
| `takeover hci-info <id> --execute [--json]` | Direct development HCI diagnostic without the service |
| `recover --offline [--json]` | Local recovery when the runtime is stopped or unavailable |

Run state-changing commands elevated and only after the
[Windows hardware test precautions](windows-test-plan.md). Never treat an
old printed ControllerId, driver INF or rank as a substitute for a fresh
planner result. If recovery cannot confirm Windows ownership, keep the
journal and follow [installation recovery](installation.md#recovery).

The separate `directhci-ble.exe` commands are documented in the
[BLE library README](../crates/directhci-ble/README.md#cli). Run
`directhci.exe --help` for the exact syntax of this build.
