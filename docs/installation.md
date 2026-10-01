# Windows binary installation (development package)

This installer packages the Rust executables and registers `directhcid` as a
LocalSystem, demand-start Windows service. It installs a Control Panel shortcut
and an Add/Remove Programs uninstall entry. It does **not** install, stage,
remove, or select any Bluetooth driver package, change the AX201 binding,
change boot signing policy, or run a takeover. On the already validated AX201
machine, the previously staged, signed development WinUSB package remains a
separate prerequisite for takeover.

The installer includes the project's current GPLv3 license text. Before
redistributing a binary build, make the corresponding source for that exact
build available under the license terms; third-party components retain their
own notices and licenses. See [LICENSE.txt](../LICENSE.txt) and
[references.md](references.md).

## Build on the Windows host

Install Inno Setup 6.4 or newer (compiler `ISCC.exe`) separately. From the repository in
PowerShell, run:

```powershell
.\scripts\windows\build-installer.ps1
```

The script builds `directhci`, `directhcid`, `directhci-control-panel`, and
`directhci-ble-cli` for `x86_64-pc-windows-gnu`. Cargo output and the installer
are placed under `%LOCALAPPDATA%\DirectHCI`, not the VMware shared folder.
After a source change, run the script **without** `-SkipBuild`: that flag only
checks that four executables exist and may package stale binaries. The installer
output filename remains `DirectHCI-Setup-0.1.0-alpha.1.exe` during this
development version, so run the newly generated file from the output directory,
not an older copy in Downloads.

Run the resulting `DirectHCI-Setup-0.1.0-alpha.1.exe` from
`%LOCALAPPDATA%\DirectHCI\installer-output`. Setup requires administrator
approval, installs under `C:\Program Files\DirectHCI`, registers the service,
and adds a Start Menu Control Panel shortcut. If Setup is already running with
an elevated token, Windows need not display another UAC prompt; absence of
a second prompt is not evidence of a non-administrative install. It does not
start takeover or
test the Bluetooth controller. Close any manually running `directhcid run`
console before setup so it does not retain the named pipe. The service is
demand-start: use the Control
Panel's **Start Service** button or the opt-in Rust SDK
`DirectHciClient::connect_or_start(...)` to start it. The normal
`DirectHciClient::connect(...)` call intentionally retains its original
no-autostart behavior.

Service installation initializes a protected `%ProgramData%\DirectHCI`
directory. Upgrade and uninstall preflight can automatically repair a
legacy directory with a trusted owner and only an optional config file, but
never one containing an ownership journal. A formerly writable config is
quarantined rather than trusted. This does not change the Bluetooth driver.

Only callers with Windows `SERVICE_START` permission can demand-start the
service. Default Windows service security generally reserves this to
Administrators; this installer does not grant low-privilege users new service
or Raw HCI rights. An existing consumer binary will not automatically use the
new SDK method: its integration must explicitly call it (and be rebuilt).
After an owning client disconnects, the service closes Raw HCI and restores
Windows Bluetooth, but remains running for later clients. Client disconnect
and an empty IPC connection list do not stop the service. It stops on the
Control Panel's **Stop Service** action, normal Control Panel close, or another
explicit Windows SCM stop request. RecoveryRequired keeps the M1 journal for
offline recovery. Console `directhcid run` remains long-running until its
own explicit shutdown.

## Uninstall and upgrade

Use Windows **Installed apps → DirectHCI → Uninstall**. The uninstaller asks
`directhcid uninstall-service` to stop the service, wait for it to exit, and
run the existing offline Windows Bluetooth recovery before deleting the service
or binaries. If recovery is not confirmed, uninstallation stops and retains
the files and journal. Resolve the reported condition and retry; do not delete
the service executable while the controller is WinUSB-bound.

Setup uses the same preflight before replacing an existing installation. It
skips service removal on a first install when neither a DirectHCI service nor
an ownership journal exists. If a preflight fails, Setup shows the runtime's
specific error instead of only an exit code; rebuild the installer after
changing its `.iss` source. It
does not remove the separately staged development WinUSB driver package, the
development signing certificate, or `%ProgramData%\DirectHCI` state. Those
are deliberately separate from the runtime installer so an unresolved journal
is not silently erased. The installer currently has no flow for provisioning a
fresh, unconfigured PC; driver package enrollment is still a manual,
privileged procedure documented under `driver/winusb-ax201-dev/`.

The Control Panel and SDK work only through the service. The Control Panel
requests UAC administrator approval when launched without an elevated token;
installer elevation does not carry over to later Start Menu launches. If
approval is declined, the panel does not open. An already elevated launch
does not request elevation again. The UAC request is implemented in the GUI
executable, so an installed **old binary** will not gain it merely because
the installer source changed: rebuild, install the new package, and launch
the installed executable from the Start Menu. Ordinary DirectHCI clients do
not become administrators. Closing the Control Panel requests an SCM service
stop and waits for `Stopped` before the window exits; the explicit
**Stop Service** button remains available. An active session requires
confirmation before either action disconnects the client and restores Windows
Bluetooth. If stopping fails, the window remains open and shows the error.
Service, controller, active-client, and recovery status refresh automatically
about every two seconds while the panel is open; IPC queries stay on its
background worker. An owning client's disconnect restores Windows Bluetooth
without stopping the service.
