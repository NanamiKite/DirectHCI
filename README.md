# DirectHCI

DirectHCI is a Windows userspace runtime for taking temporary control of a Bluetooth controller and exposing raw HCI access without permanently replacing the Windows Bluetooth stack.

The normal Windows Bluetooth driver remains in use when DirectHCI is idle. When a DirectHCI client needs the controller, the runtime can temporarily move the selected device to WinUSB, use it from userspace, and restore the Windows driver when the session ends.

The project is intended for applications that need lower-level Bluetooth access than the normal Windows APIs provide. Applications that do not need DirectHCI continue using the Windows Bluetooth stack normally.

## What works

DirectHCI currently provides:

- Bluetooth controller discovery and stable controller identity
- temporary Windows Bluetooth → WinUSB takeover
- automatic restoration of the Windows Bluetooth driver
- durable recovery state for interrupted sessions
- raw HCI command, event, and ACL transport
- a privileged Windows runtime (`directhcid`)
- local Named Pipe IPC
- a Rust client SDK
- a `bt-hci` transport adapter
- optional BLE/GATT support built on TrouBLE
- command-line diagnostics and recovery tools

The main development and hardware validation target is currently an Intel AX201 Bluetooth controller. The runtime itself is not intended to be Intel-specific.

## How it works

A normal DirectHCI session looks roughly like this:

```text
Application
    │
    │ DirectHCI SDK
    ▼
directhcid
    │
    ├─ temporarily takes ownership of the selected controller
    ├─ opens the WinUSB interface
    └─ exposes HCI command/event/ACL transport
    │
    ▼
Bluetooth Controller
```

When the client releases the session or disconnects:

```text
DirectHCI
    ↓
close HCI session
    ↓
restore Windows Bluetooth driver
    ↓
Windows owns the controller again
```

Running `directhcid` by itself does not take over a Bluetooth controller. Ownership changes only when a client explicitly requests a DirectHCI session.

## This is not a replacement Bluetooth stack

DirectHCI is an opt-in compatibility path.

It does not redirect all Bluetooth traffic on the machine, and ordinary Windows Bluetooth applications do not need to know that DirectHCI exists.

```text
Normal application
    ↓
Windows Bluetooth stack

Application using DirectHCI
    ↓
DirectHCI
    ↓
selected controller
```

The core runtime exposes controller ownership and HCI transport. Higher-level BLE support is provided separately through the DirectHCI BLE library and existing Bluetooth host-stack components rather than reimplementing ATT, GATT, or L2CAP from scratch.

## Repository layout

```text
apps/
  directhci/          Command-line tools
  directhcid/         Privileged Windows runtime

crates/
  directhci-core/     Shared models and IPC types
  directhci-windows/  Windows controller, driver and WinUSB support
  directhci-client/   Client SDK
  directhci-bt-hci/   bt-hci controller adapter
  directhci-ble/      Higher-level BLE/GATT API

driver/
  winusb-ax201-dev/   Current development WinUSB package

docs/
  ...                 Design, recovery and hardware-testing notes
```

## CLI

Some useful development and diagnostic commands:

```text
directhci controllers
directhci controllers --json

directhci status

directhci controller show <id>

directhci doctor
directhci doctor --json

directhci recover --offline
```

Development builds also contain lower-level ownership and HCI commands used for hardware validation.

See the documentation under [`docs/`](docs/) for destructive takeover tests and driver provisioning. Do not run takeover commands on a controller you cannot recover.

## Controller ownership

DirectHCI treats controller ownership as exclusive:

```text
Windows owned
    ↕
DirectHCI owned
```

A controller is never intentionally shared between the Windows Bluetooth stack and a DirectHCI writer.

The runtime keeps a recovery journal before changing controller ownership. Recovery always checks the current Windows device state rather than blindly replaying old driver information.

Device interface paths, driver names, and current devnodes are treated as observations rather than permanent controller identities.

## Windows and WinUSB

The current development package uses Microsoft's in-box `WinUSB.sys`; DirectHCI does not ship a custom kernel driver.

The development AX201 package is located under:

```text
driver/winusb-ax201-dev/
```

It is intended for development and hardware validation, not as a generic production driver package.

Driver staging, temporary rebind, restoration and recovery are documented in:

- [`docs/temporary-rebind.md`](docs/temporary-rebind.md)
- [`docs/windows-test-plan.md`](docs/windows-test-plan.md)

## Development

The main target is Windows.

Most portable Rust code can also be built from Linux.

See [`docs/development.md`](docs/development.md).

## Safety

DirectHCI operates below the normal Windows Bluetooth API and can temporarily remove a Bluetooth controller from the Windows Bluetooth stack.

A few rules are therefore deliberate:

- only one writer owns a controller at a time
- controller takeover is explicit
- destructive driver operations are gated
- interrupted ownership changes leave recovery information behind
- recovery prefers current Windows state over stale journal data
- hardware-persistent HCI writes are not part of the normal DirectHCI path

If DirectHCI is experimenting with your only Bluetooth controller, expect Windows Bluetooth devices using that controller to be temporarily unavailable while the session is active.

## Project status

DirectHCI is still under active development.

The controller ownership, Raw HCI runtime, local IPC, Rust client path and BLE integration have been exercised on real Windows hardware. Packaging, installation, controller configuration and hardening of the system-service boundary are still being worked on.

The current focus is turning the validated runtime into a clean Windows system component without changing the already-working HCI path.

## License

The current, provisional project license is GNU GPL v3 only
(`GPL-3.0-only`). See [LICENSE.txt](LICENSE.txt) for the full English text.
This applies to DirectHCI's first-party crates and applications. Third-party
dependencies and external tools keep their own licenses; see
[references.md](docs/references.md). A future release may revisit the license,
but that does not retroactively revoke rights granted for an earlier release.

