# Development

DirectHCI uses Rust 2024 and requires Rust 1.88 or later. Runtime and hardware
testing takes place on Windows; Linux can build and test the portable logic.
Keep Cargo output on a local disk when the source is in a VM shared folder.

## Build on Windows

Install the Windows GNU Rust target and put MinGW-w64 tools on `PATH`.
The Control Panel build runs `windres.exe` to embed its icon; `WINDRES` can
specify another resource compiler path.

From PowerShell at the repository root:

```powershell
rustup target add x86_64-pc-windows-gnu
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\DirectHCI\target"
cargo build --locked --release --workspace --target x86_64-pc-windows-gnu
$bin = Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-gnu\release'
```

The build produces `directhci.exe`, `directhcid.exe`,
`directhci-control-panel.exe` and `directhci-ble.exe` in `$bin`.
The BLE executable's Cargo package is named `directhci-ble-cli`.

## Build the installer

The Windows build host also needs a mounted EWDK ISO for the pinned native
libwdi DLL and Inno Setup 6.4 or later. EWDK is a build-host dependency,
not an end-user requirement. The Rust executables still use the Windows GNU
target and MinGW-w64 tools described above.

In `cmd.exe`, initialize the EWDK environment, then run both scripts from
the same window:

```cmd
F:\LaunchBuildEnv.cmd
SetupVSEnv
cd /d "C:\path\to\DirectHCI"
rustup target add x86_64-pc-windows-gnu
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\build-libwdi.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\build-installer.ps1
```

Replace `F:` and the source path with your build host's actual paths.
`build-libwdi.ps1` downloads pinned libwdi 1.5.1, checks its hash, applies
the DirectHCI patches and builds `libwdi.dll` with EWDK/MSBuild.
`build-installer.ps1` checks the DLL manifest, compiles the four Rust
executables unless `-SkipBuild` was explicitly supplied, then invokes Inno
Setup. Do not use `-SkipBuild` after code changes: it reuses existing
executables without verifying that they match the source.

With the default target directory, the Rust executables are under
`%LOCALAPPDATA%\DirectHCI\target\x86_64-pc-windows-gnu\release`, the
native DLL and source archive under `%LOCALAPPDATA%\DirectHCI\native`, and
the installer under `%LOCALAPPDATA%\DirectHCI\installer-output`. An
explicit `CARGO_TARGET_DIR` overrides the Rust output location. Keep build
outputs on a local disk when the source is in a shared folder.

The Inno Setup file currently bundles `libwdi.dll` but not its source
archive. A binary distribution must separately satisfy the upstream
source/notice obligations documented in [references](references.md#distribution-files).

## Run the daemon from a terminal

In an elevated PowerShell, use the `$bin` directory above:

```powershell
& (Join-Path $bin 'directhcid.exe') run
```

Keep that terminal open. In a second PowerShell, set `$bin` to the same build
directory and query the daemon:

```powershell
$bin = "$env:LOCALAPPDATA\DirectHCI\target\x86_64-pc-windows-gnu\release"
& (Join-Path $bin 'directhci.exe') status
& (Join-Path $bin 'directhci.exe') controllers --json
```

Adjust the second path if using a different `CARGO_TARGET_DIR`. The daemon
uses `\\.\pipe\DirectHCI\v1`; stop an installed DirectHCI service before
running the foreground daemon. Ctrl+C closes sessions and attempts to
restore Windows Bluetooth.

Controller discovery and planning commands do not change the device:

```powershell
& (Join-Path $bin 'directhci.exe') controllers --direct --json
& (Join-Path $bin 'directhci.exe') doctor --json
& (Join-Path $bin 'directhci.exe') takeover plan '<controller-id>' --json
```

Replace `<controller-id>` with an ID from the current controller list.
`controllers --direct`, `doctor`, `controller show` and `takeover plan`
can run without the daemon. Normal `controllers` and `status` commands use IPC.

Takeover commands with `--execute` and `recover --offline` change driver
state and require an elevated terminal. Follow the
[hardware test plan](windows-test-plan.md) before running them.

## Service and Control Panel

Use [the installer](installation.md) to test service
installation and the Control Panel. Launch the installed panel from the Start
Menu; it requests elevation and can start or stop the service. Panel behavior,
controller preparation and recovery are described in
[installation.md](installation.md).

For development, `directhcid install-service` registers the executable being
run as a demand-start service. Its path must be on a fixed local drive, with
trusted ownership and protected write access. Shared folders, reparse points
and user-writable service paths are refused. `directhcid uninstall-service`
stops the service, verifies recovery and removes the registration.

Service registration does not prepare a WinUSB package. That is a separate
**Prepare Controller** action in the panel. Installed binaries under
`C:\Program Files\DirectHCI` are updated by installing a new package; a Cargo
build only updates `$bin`.

## Local checks

On Linux, set a local target directory before checking the workspace:

```sh
export CARGO_TARGET_DIR=/tmp/directhci-target
cargo fmt --all --check
cargo check --locked --workspace
cargo test --locked --workspace
```

To check Windows compilation from Linux, install the Windows GNU Rust target
and MinGW-w64 tools, including `x86_64-w64-mingw32-windres`, then run:

```sh
rustup target add x86_64-pc-windows-gnu
cargo check --locked --workspace --target x86_64-pc-windows-gnu
```

These checks cover Rust compilation and portable tests. SetupAPI, WinUSB,
driver changes, service behavior and Bluetooth recovery require the
[Windows host tests](windows-test-plan.md).

## Continuous integration

GitHub Actions runs `cargo fmt --all --check` and
`cargo test --locked --workspace` on Linux, then cross-builds the workspace
for `x86_64-pc-windows-gnu` with MinGW-w64. See
[the CI workflow](../.github/workflows/ci.yml). The Windows job checks that
the executables link; it does not execute them on the Linux runner.

CI deliberately does not create or trust signing certificates, build the
libwdi DLL, package the installer, install a service or driver, prepare a
controller, run HCI/BLE, or claim that Windows Bluetooth restoration works.
Those remain explicit Windows build-host and hardware acceptance steps.

## Runtime state

The daemon stores preferences and the ownership journal in
`%ProgramData%\DirectHCI`. The directory requires a protected
SYSTEM/Administrators ACL and trusted ownership.

Installation and startup can repair a legacy directory if it has a trusted
owner, no reparse point, and no contents beyond an optional config file.
A previously writable `config.json` is quarantined and preferences are
recreated. An existing journal, unexpected files or an untrusted owner blocks
automatic repair. Preserve these files when investigating recovery errors;
see [ownership-and-recovery.md](ownership-and-recovery.md#offline-recovery-boundary).
