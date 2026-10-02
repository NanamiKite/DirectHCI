# Windows installation

The installer targets Windows x64 and installs four executables, the libwdi
provisioning DLL, a Start Menu shortcut and the `DirectHCI` Windows service.
The service runs as LocalSystem and starts on demand. Setup leaves Bluetooth
controller drivers unchanged.

The latest installer and Control Panel changes still need Windows host
acceptance; tracked results are in [compatibility.md](compatibility.md).

## Install and start

If building from source, first [build the installer](#build-the-installer).

1. Close any foreground `directhcid run` process so it releases the named pipe.
2. Run `DirectHCI-Setup-0.1.0-alpha.1.exe` with administrator approval.
   Programs are installed under `C:\Program Files\DirectHCI`.
3. Open **DirectHCI Control Panel** from the Start Menu. The panel requests
   elevation if needed; declining closes it.
4. Click **Start Service**, then select the controller to use.

The daemon saves the preferred controller in
`%ProgramData%\DirectHCI\config.json`. With one controller and no saved
preference, it selects that controller automatically. With several, select
one in the panel. A missing saved controller requires a new selection;
preferences cannot change during an active session.

## Prepare a controller

When the selected controller shows **Not prepared**, click **Prepare
Controller**. The confirmation describes the certificate trust change:

1. The service reads the controller's current USB Hardware ID and generates
   a matching WinUSB INF.
2. libwdi creates a one-time certificate, adds its public certificate to
   LocalMachine Root and TrustedPublisher, signs the catalog, and destroys
   the private key.
3. The service stages the package in the Driver Store, checks the resulting
   driver candidate and verifies that the Windows Bluetooth driver is still
   bound.

The package uses Microsoft's in-box `WinUSB.sys`. Preparation requires no
WDK or signing tools on the user machine. A newly attached controller can be
prepared without reinstalling DirectHCI.

Windows may reject the locally signed package. In that case, preparation
reports the SetupAPI error and leaves the Bluetooth binding unchanged.
DirectHCI does not change Secure Boot, TESTSIGNING or BCD. The public
certificate may remain trusted after a failed preparation because automatic
certificate cleanup is not yet implemented with package-reference checks.

Preparation only makes the driver available. The service switches to WinUSB
when a client acquires a session, after checking identity, driver rank and
recovery readiness. See [temporary-rebind.md](temporary-rebind.md) for the
checks and recovery sequence.

## Use the service

With the service running, status queries work from a normal PowerShell:

```powershell
& "$env:ProgramFiles\DirectHCI\directhci.exe" status
& "$env:ProgramFiles\DirectHCI\directhci.exe" controllers --json
& "$env:ProgramFiles\DirectHCI\directhci.exe" doctor --json
```

Clients that acquire a controller or send raw HCI need administrator rights.
For BLE commands and library use, see [directhci-ble](../crates/directhci-ble/README.md).

The panel refreshes status about every two seconds. Its lifecycle controls
behave as follows:

| Action | Result |
| --- | --- |
| Client releases or disconnects | HCI closes and Windows Bluetooth is restored; the service stays running |
| **Stop Service** | Stops the service and keeps the panel open |
| Close the panel or choose **Exit Control Panel** in the tray | Stops the service, waits for `Stopped`, then exits |
| Minimize the panel | Hides it in the tray; the service keeps running |

Stopping with an active session requires confirmation. If stopping fails,
the panel stays open and shows the error. Click the tray icon to reopen a
minimized panel.

Rust clients can opt into service startup with
`DirectHciClient::connect_or_start(...)`. It requires Windows `SERVICE_START`
permission, normally held by administrators with this installation.
`DirectHciClient::connect(...)` connects to an already running service.

## Recovery

For recovery while the runtime is available, use **Restore Windows** in the
panel. If the runtime cannot recover normally, stop it and run offline recovery
from an elevated PowerShell:

```powershell
& "$env:ProgramFiles\DirectHCI\directhci.exe" recover --offline --json
```

Offline recovery checks the saved journal against a fresh device and driver
observation. It refuses to run while the journal's owning process is active.
If the controller is missing, identity is ambiguous, or the journal path is
unsafe, keep the reported error and `%ProgramData%\DirectHCI` contents for
diagnosis. Deleting the journal removes information needed for recovery.

## Upgrade or uninstall

Run a new installer to upgrade, or use **Installed apps → DirectHCI →
Uninstall** to remove the application. Both first stop the service and check
Windows Bluetooth recovery. If recovery cannot be confirmed, the operation
stops and retains the files, service and journal; resolve the error and retry.

Uninstall leaves staged WinUSB packages, trusted public certificates and
`%ProgramData%\DirectHCI` state in place. These need separate maintenance after
controller recovery is confirmed.

## Build the installer

The Windows build host needs:

- Rust 1.87 or later with the `x86_64-pc-windows-gnu` target;
- MinGW-w64 tools on `PATH`, including `windres.exe` for the panel's icon;
- a mounted EWDK ISO for the native libwdi build;
- Inno Setup 6.4 or later.

In `cmd.exe`, initialize EWDK and run both build scripts from the same window:

```cmd
F:\LaunchBuildEnv.cmd
SetupVSEnv
cd /d "C:\path\to\DirectHCI"
rustup target add x86_64-pc-windows-gnu
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\build-libwdi.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\build-installer.ps1
```

Replace `F:` and the repository path with local values. `build-libwdi.ps1`
downloads pinned libwdi 1.5.1, verifies the archive hash, applies the project's
INF and private-key cleanup patches, then builds the DLL with EWDK MSBuild.
`build-installer.ps1` checks the native build manifest, builds the four Rust
executables, and runs Inno Setup.

Default output locations:

| Artifact | Directory under `%LOCALAPPDATA%\DirectHCI` |
| --- | --- |
| libwdi DLL, source archive and build manifest | `native` |
| Rust executables | `target\x86_64-pc-windows-gnu\release` |
| `DirectHCI-Setup-0.1.0-alpha.1.exe` | `installer-output` |

An existing `CARGO_TARGET_DIR` overrides the Rust output directory. Source
can live in a shared folder; keep build outputs on a local disk.

After changing source, build without `-SkipBuild`. That flag reuses existing
executables without checking whether they match the source. The development
installer filename stays the same, so run the file from the output directory.

The build requires a libwdi source archive, but the current Inno Setup file
only installs its DLL. Binary distribution also needs the corresponding
source and notices described in [references.md](references.md).
