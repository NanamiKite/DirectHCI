# DirectHCI

[中文](docs/README.zh-CN.md)

DirectHCI is a Windows userspace Bluetooth controller ownership and Raw HCI
runtime for USB Bluetooth controllers. Its local service temporarily binds a
selected controller to Microsoft's WinUSB driver, carries HCI commands, events
and ACL data, and restores the Windows Bluetooth driver when the session ends.

DirectHCI does **not** use a static Bluetooth VID/PID allowlist. **Prepare
Controller** generates an exact-Hardware-ID WinUSB package from a freshly
observed, eligible USB Bluetooth controller. Preparation stages a driver
candidate; it does **not** take the controller away from Windows. Windows
signing policy and the later takeover safety checks can still reject it.

```text
Windows Bluetooth (BTHUSB / IBTUSB)
    │ Prepare Controller: stage an exact-Hardware-ID WinUSB candidate
    │ Acquire: verify identity, driver rank and recovery path
    ▼
DirectHCI owns controller (WinUSB)
    │ HCI Command / Event / ACL
    │ Release, client disconnect or service stop
    ▼
Windows Bluetooth restored
```

The repository includes a Windows service, Control Panel, diagnostic CLI, and
Rust libraries for Raw HCI and BLE Central/GATT through TrouBLE. DirectHCI is
not a vendor-specific device protocol or a replacement BLE host stack.

## Key features

- Guarded, temporary USB Bluetooth ownership with a durable recovery journal.
- Raw HCI Command, Event and ACL access through a local Windows service and SDK.
- On-demand, exact-Hardware-ID WinUSB preparation without a static VID/PID list.
- A native Control Panel, diagnostic CLI and optional BLE Central/GATT library.

## Installation and quick start

The installer targets Windows x64. Installing DirectHCI does not switch any
Bluetooth controller.

1. Install DirectHCI and open **DirectHCI Control Panel** from the Start Menu.
   Installation and the panel request administrator approval.
2. Click **Start Service** and select the USB Bluetooth controller to use.
3. If it shows **Not prepared**, click **Prepare Controller** and review the
   local certificate-trust prompt. Windows may reject a locally signed package;
   the panel reports that failure without switching the controller.
4. Start a DirectHCI client. Only client acquisition initiates the temporary
   takeover. Release, disconnect or service stop triggers restoration.

The Control Panel shows service state, selected controller, active client and
recovery state. Closing it stops the service; minimizing it keeps the service
running in the tray. See [installation](docs/installation.md) for normal use
and recovery.

While DirectHCI owns a controller, Windows Bluetooth devices using that
controller are unavailable. Keep a non-Bluetooth keyboard or mouse available
when experimenting with the controller that serves your input devices. The
runtime allows one active Raw HCI session at a time.

## Current status

Version: `0.1.0-alpha.1`. Real-hardware users report normal DirectHCI
operation with Intel AX201 (`8087:0026`), AX200 (`8087:0029`) and
BE200/Gale Peak-family Bluetooth (`8087:0036`). These are common family
labels, not proof of an exact module SKU from the USB ID alone. Detailed
takeover, Raw HCI, BLE and restore observations are recorded for the AX201;
the other reports still need archived
build/host and per-stage logs. Windows acceptance of a locally signed package
remains host-policy-dependent. See the
[compatibility and validation record](docs/compatibility.md).

## Architecture and development

```text
Raw HCI consumer / CLI / Control Panel
                  ↓
           directhci-client
                  ↓ local named pipe
               directhcid
                  ├─ ownership and recovery
                  └─ Raw HCI over WinUSB
```

`directhcid` owns driver transitions, WinUSB handles and the recovery journal.
The BLE host is an optional consumer of Raw HCI, not part of the privileged
runtime. See [architecture](docs/architecture.md), the
[SDK guide](docs/sdk.md), and [development](docs/development.md). The Rust
crates are currently unpublished; use path dependencies from a local checkout.

## Documentation

- [Installation and recovery](docs/installation.md) · [Troubleshooting and safe recovery](docs/troubleshooting.md)
- [Compatibility model and hardware validation](docs/compatibility.md)
- [Architecture](docs/architecture.md)
- [Ownership and recovery internals](docs/ownership-and-recovery.md)
- [Failure model](docs/failure-model.md)
- [CLI](docs/cli.md) · [Rust SDK](docs/sdk.md) · [Development](docs/development.md)
- [Driver provisioning internals](docs/internals/driver-provisioning.md)
- [Windows hardware test plan](docs/windows-test-plan.md)
- [Dependencies and references](docs/references.md)

## License

DirectHCI's first-party crates and applications currently use
[GPL-3.0-only](LICENSE.txt). Third-party licenses are listed in
[references](docs/references.md).
