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
