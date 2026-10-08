# Development

DirectHCI uses Rust 2024 and requires Rust 1.87 or later. Runtime and hardware
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

Use [the installer](installation.md#build-the-installer) to test service
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

## Runtime state

The daemon stores preferences and the ownership journal in
`%ProgramData%\DirectHCI`. The directory requires a protected
SYSTEM/Administrators ACL and trusted ownership.

Installation and startup can repair a legacy directory if it has a trusted
owner, no reparse point, and no contents beyond an optional config file.
A previously writable `config.json` is quarantined and preferences are
recreated. An existing journal, unexpected files or an untrusted owner blocks
automatic repair. Preserve these files when investigating recovery errors;
see [temporary-rebind.md](temporary-rebind.md#offline-recovery-boundary).
