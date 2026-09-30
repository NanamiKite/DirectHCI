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
cargo build --workspace --target x86_64-pc-windows-gnu
```

Controller discovery, `doctor`, and `takeover plan` are non-device-mutating.
The development command `takeover roundtrip <id> --execute` and
`recover --offline` use privileged driver-install APIs and must run from an
elevated terminal. Follow [temporary-rebind.md](temporary-rebind.md) and
[windows-test-plan.md](windows-test-plan.md); never add `/install` to the
manual `pnputil /add-driver` staging command.

## M3 runtime

Run the development console host from an elevated Windows terminal:

```powershell
directhcid run
```

Normal product-path commands connect to `\\.\pipe\DirectHCI\v1`:

```powershell
directhci status
directhci controllers
directhci hci-info <controller-id> --execute
```

`directhcid install-service` registers the current executable as the manual
start `DirectHCI` Windows service. It does not stage the WinUSB package or
modify controller drivers. `controllers --direct`, `takeover ... --execute`,
and `recover --offline` remain explicit development/disaster-recovery paths.
Service installation now refuses non-fixed/remote paths, reparse points, or a
binary/parent with untrusted owner or write access. Console `directhcid run`
continues to work from development directories.

The `%ProgramData%\DirectHCI` ownership journal directory is now accepted only
with a protected SYSTEM/Administrators ACL and trusted owner. An older or
pre-created directory with broader permissions is deliberately rejected; have
an administrator inspect and repair it before takeover or offline recovery.
DirectHCI will not silently adopt an untrusted journal directory.
