# DirectHCI

[中文](docs/README.zh-CN.md)

DirectHCI gives Windows applications raw HCI access to a USB Bluetooth
controller. A local service temporarily binds the selected controller to
WinUSB, handles HCI commands, events and ACL data, and restores the Windows
Bluetooth driver when the client releases the session or disconnects.

The repository includes a Windows service, a control panel, command-line tools,
and Rust libraries for raw HCI and BLE Central/GATT through TrouBLE.

**Status: `0.1.0-alpha.1`.** Development testing has used an Intel AX201 on
Windows 11. Driver takeover, raw HCI and Windows restoration have worked on
that host. BLE lifecycle regressions and recent installer/control-panel
changes still need hardware retesting. See [compatibility](docs/compatibility.md)
for the recorded results and open items.

While a DirectHCI session is active, Windows Bluetooth devices using that
controller are unavailable. The runtime allows one active HCI session at a
time. Keep a wired keyboard or mouse available when testing a controller used
by your input devices.

## Getting started

The installer targets Windows x64. To build it from source, follow
[the installation guide](docs/installation.md#build-the-installer); the build
requires Rust, MinGW-w64, EWDK and Inno Setup.

1. Run the installer and open **DirectHCI Control Panel** from the Start Menu.
   Both require administrator approval.
2. Click **Start Service** and select a controller.
3. If it shows **Not prepared**, click **Prepare Controller**. This creates and
   stages a WinUSB package after asking for local certificate trust. Windows
   may reject the package under its signing policy; see
   [controller preparation](docs/installation.md#prepare-a-controller).
4. Use the CLI or a Rust client to open a session. Starting the service and
   preparing a controller leave Windows Bluetooth in control; acquisition
   happens when a client requests a session.

With the service running, these PowerShell commands query its status and list
controllers:

```powershell
& "$env:ProgramFiles\DirectHCI\directhci.exe" status
& "$env:ProgramFiles\DirectHCI\directhci.exe" controllers
```

For command syntax, run `directhci.exe --help`. BLE operations are available
through `directhci-ble.exe` and the [BLE library](crates/directhci-ble/README.md).
Acquiring a controller requires an elevated client.

Closing the Control Panel stops the service; minimizing it keeps the service
running in the background. If a session ends abnormally and Bluetooth is not
restored, follow [recovery](docs/installation.md#recovery).

## Rust libraries

| Crate | Purpose |
| --- | --- |
| [`directhci-client`](crates/directhci-client) | Local service client: controller queries, acquisition and raw HCI sessions |
| [`directhci-bt-hci`](crates/directhci-bt-hci) | `bt-hci` controller adapter for a DirectHCI session |
| [`directhci-ble`](crates/directhci-ble) | Async BLE scanning, connections, GATT read/write and notifications |

The crates are unpublished; use path dependencies from a local checkout.
The client talks to `directhcid` over a local named pipe. The service owns the
WinUSB handles and driver changes, and records recovery state before takeover.
See [architecture](docs/architecture.md) for the session and recovery model.

## Building

Rust 1.87 or later is required. On Windows, install the
`x86_64-pc-windows-gnu` target and put the MinGW-w64 tools, including
`windres.exe`, on `PATH`. From the repository root:

```powershell
rustup target add x86_64-pc-windows-gnu
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\DirectHCI\target"
cargo build --locked --release --workspace --target x86_64-pc-windows-gnu
```

Executables are written to
`%LOCALAPPDATA%\DirectHCI\target\x86_64-pc-windows-gnu\release`.
Building them does not install the service. See
[development](docs/development.md) for running the daemon from a terminal,
Linux checks and installer prerequisites.

## Repository

| Path | Contents |
| --- | --- |
| `apps/` | Service, Control Panel, diagnostic CLI and BLE CLI |
| `crates/` | Client libraries, shared types and Windows backend |
| `driver/` | Device-specific WinUSB INF template and legacy development package |
| `installer/`, `scripts/windows/` | Windows packaging and build scripts |
| `docs/` | Setup, architecture, recovery and hardware test notes |

## Documentation

- [Installation, preparation and recovery](docs/installation.md)
- [Development](docs/development.md)
- [Hardware compatibility and known issues](docs/compatibility.md)
- [Architecture](docs/architecture.md)
- [Temporary driver rebind and recovery](docs/temporary-rebind.md)
- [Windows hardware test plan](docs/windows-test-plan.md)
- [Dependencies and references](docs/references.md)

## License

DirectHCI's first-party crates and applications currently use
[GPL-3.0-only](LICENSE.txt). Third-party licenses are listed in
[references](docs/references.md).
