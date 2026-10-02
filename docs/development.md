# Development workflow

Source code may live in a VMware shared folder, but build artifacts should be
placed on the VM or Windows host local disk.

## Linux VM

Choose a VM-local directory and export it before running Cargo:

```sh
export CARGO_TARGET_DIR=/tmp/directhci-target
cargo fmt --all --check
cargo check --workspace
cargo check --workspace --target x86_64-pc-windows-gnu
```

The repository does not hard-code a target directory because VM and host paths
are environment-specific.

Linux checks validate portable logic and Windows GNU compilation. They do not
validate SetupAPI behavior, driver state, UsbDk, WinUSB, or AX201 ownership.

## Windows host

Copy or build source from the shared folder, but use a host-local target
directory, for example in PowerShell:

```powershell
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\DirectHCI\target"
cargo build --locked --release --target x86_64-pc-windows-gnu -p directhci -p directhcid -p directhci-control-panel -p directhci-ble-cli
$bin = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-gnu\release'
Get-Item (Join-Path $bin 'directhci.exe'), (Join-Path $bin 'directhcid.exe'), (Join-Path $bin 'directhci-control-panel.exe'), (Join-Path $bin 'directhci-ble.exe')
```

These four executables are in `$bin`, not in the source tree and not in
`C:\Program Files\DirectHCI`. Keep the same `CARGO_TARGET_DIR` for build and
run. The installer copies them to Program Files only after a separate installer
build and installation. If the build fails, the executable may not exist.
Without this environment
variable, Cargo instead uses the repository's `target` directory.

Controller discovery, `doctor`, and `takeover plan` are non-device-mutating.
The development command `takeover roundtrip <id> --execute` and
`recover --offline` use privileged driver-install APIs and must run from an
elevated terminal. Follow [temporary-rebind.md](temporary-rebind.md) and
[windows-test-plan.md](windows-test-plan.md); never add `/install` to the
manual `pnputil /add-driver` staging command.

## M3 runtime

Run the development console host from an elevated Windows terminal:

```powershell
& (Join-Path $bin 'directhcid.exe') run
```

Normal product-path commands connect to `\\.\pipe\DirectHCI\v1`:

```powershell
& (Join-Path $bin 'directhci.exe') status
& (Join-Path $bin 'directhci.exe') controllers
```

`directhcid install-service` registers the current executable as the manual
start `DirectHCI` Windows service. That command does not stage the WinUSB
package or modify controller drivers; the separate installer offers optional
Driver Store staging. `controllers --direct`, `takeover ... --execute`,
and `recover --offline` remain explicit development/disaster-recovery paths.
Service installation now refuses non-fixed/remote paths, reparse points, or a
binary/parent with untrusted owner or write access. Console `directhcid run`
continues to work from development directories.

The `%ProgramData%\DirectHCI` ownership journal directory is now accepted only
with a protected SYSTEM/Administrators ACL and trusted owner. Service
installation/startup safely creates it when missing. A legacy directory with
an Administrators/SYSTEM owner, no reparse point, and no ownership journal is
automatically tightened; a formerly user-writable `config.json` is
quarantined and the daemon recreates preferences. An existing journal,
untrusted owner, reparse point, or unexpected directory content still blocks
automatic repair. DirectHCI never adopts an untrusted ownership journal.

## Control Panel

For normal Windows installation and uninstallation, use the installer build
and safety procedure in [installation.md](installation.md). The commands below
are for development builds only.

Once the `DirectHCI` service is installed, everyday use is simply opening
`directhci-control-panel.exe` and, if needed, clicking **Start Service**. No
terminal is needed for status, controller selection, recovery, or diagnostics.
The panel checks its effective token on startup and requests UAC elevation
when necessary. If approval is declined, it does not open. This does not
elevate the separate CLI, SDK, or BLE consumer. The panel does not install the
service; that is a one-time development/setup action, not something to repeat
every launch.

For developers building from source, use the `$bin` path from the Windows host
build above. If building only the panel:

```powershell
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\DirectHCI\target"
cargo build --release --target x86_64-pc-windows-gnu -p directhci-control-panel
$bin = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-gnu\release'
& (Join-Path $bin 'directhci-control-panel.exe')
```

Launch the resulting executable directly; it does not install or start a
service implicitly. It opens even when the `DirectHCI` service is stopped or
missing. Start/Stop Service use Windows SCM, while runtime diagnostics, preferred
controller changes, and Restore Windows use the existing local named pipe.

Ordinary local users can query diagnostics through the SDK/CLI while the
service is available. The Control Panel requests elevation for its SCM and
preference/recovery operations. Status refreshes automatically about every
two seconds on a background worker. Closing the panel requests an SCM stop,
waits for `Stopped`, then exits; the **Stop Service** button does the same
without closing the panel. Stopping an active session requires confirmation
and runs the existing Windows Bluetooth restore path. The daemon
owns `%ProgramData%\DirectHCI\config.json` and writes it atomically; the GUI
does not edit that file. If exactly one controller exists at daemon startup and
no preference has been saved, the daemon saves it automatically. With multiple
controllers, the user must choose one; a missing saved controller is not silently
replaced.
