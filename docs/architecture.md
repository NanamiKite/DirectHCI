# Architecture

DirectHCI has three main layers: Rust consumers, a local Windows service and
the Windows controller backend.

```text
CLI / Control Panel / Rust application
              |
       directhci-client
              | local named pipe
          directhcid
              |
      directhci-windows
              | SetupAPI + WinUSB
      Bluetooth controller
```

BLE consumers use `directhci-ble` and `directhci-bt-hci` above the client SDK.
The core service transports HCI commands, events and ACL data; TrouBLE supplies
the BLE host stack. Hardware results and pending tests are tracked in
[compatibility.md](compatibility.md).

## Components

| Component | Responsibility |
| --- | --- |
| `directhci-core` | Controller models, ownership states, HCI types and IPC messages |
| `directhci-windows` | PnP enumeration, driver preparation/rebind, journal, recovery and WinUSB I/O |
| `directhcid` | Privileged session owner, named-pipe server and Windows service |
| `directhci-client` | Rust SDK for queries, preferences and HCI sessions |
| `directhci-bt-hci` | `bt-hci` controller adapter |
| `directhci-ble` | BLE Central/GATT API and TrouBLE task lifecycle |
| `apps/` clients | Command parsing, diagnostics and Control Panel UI |

Windows API calls live in `directhci-windows`; shared types in
`directhci-core` have no Windows API dependency. Peripheral UUIDs and device
protocols belong in consumer applications.

## Controller identity

`ControllerIdentity` holds physical matching evidence.
`ControllerObservation` holds the current PnP and driver state. They are
separate because driver changes can replace interface paths and devnodes.

The v0 controller ID uses a genuine USB serial, then USB/PCI location, then
the current Windows instance ID, in that order, scoped by VID/PID. Driver,
INF, service and interface path do not participate. Container ID is supporting
evidence; the Windows system Container ID and zero GUID are discarded.

An opaque ControllerId indexes this evidence. Before a privileged operation,
the backend enumerates again and requires a unique match to the physical
controller. A cached ID or VID/PID alone is insufficient.

## Ownership and sessions

The implemented takeover flow is:

```text
WindowsOwned -> DirectHciOwned(session) -> WindowsOwned
```

The runtime permits one active writer session globally. Its
`RuntimeControllerSession` owns the temporary driver binding and
`RawHciSession`, including WinUSB handles and I/O workers. Clients receive
session IDs and HCI data over IPC.

Acquire first records recovery intent, selects the WinUSB driver for the
freshly identified device, and checks the resulting interface and endpoints.
An unresolved journal or ambiguous device state blocks acquisition.

Release stops new I/O, cancels and joins workers, closes handles, then
re-observes the controller and restores an applicable Windows driver. The
journal is completed only after restoration is verified. An owning pipe
disconnect, explicit release, console shutdown or SCM stop all use this path.

A separate dedicated-controller design leaves a spare dongle bound to WinUSB:

```text
DirectHciReady -> DirectHciOwned(session) -> DirectHciReady
```

The dedicated readiness probe exists, but a dedicated runtime acquisition path
and separate dongle have not been validated. See
[dedicated-controller.md](dedicated-controller.md) for the experimental setup.

## Recovery

The ownership journal records controller evidence, backend, session, transition
phase and the original observation. Each driver-changing phase is persisted
before the operation. After a crash, startup recovery or
`directhci recover --offline` reads that intent and inspects the current device.

Recovery selects a currently applicable Windows driver. An old INF name in
the journal is historical evidence, not an unconditional reinstall target.
A missing controller stays pending; another device with the same VID/PID
cannot substitute for it. Repeated recovery calls re-observe the state before
acting.

The journal phases, selection rules and failure handling are documented in
[temporary-rebind.md](temporary-rebind.md). Offline recovery calls the Windows
backend directly, so it remains available when the service cannot run.

## IPC and permissions

The service listens on `\\.\pipe\DirectHCI\v1`. Messages are byte-framed and
bounded; control payloads use JSON, while HCI and ACL payloads stay binary.
There is no network listener.

Authenticated local users can query diagnostics. Acquisition and raw HCI
operations require administrator membership. A client connection can own the
single active session; breaking that connection triggers release.

The Control Panel uses IPC for controller operations and Windows SCM for
service control. It requests elevation, refreshes state on a background
worker, and waits for the service to stop before closing. Client disconnect
restores Bluetooth but leaves the service running.

The daemon validates and atomically stores the preferred ControllerId in
`%ProgramData%\DirectHCI\config.json`. It refuses preference changes during
an active session. A preference selects the intended controller; acquisition
still performs fresh identity checks.

## BLE lifecycle

`directhci-ble` owns the client session, `directhci-bt-hci` adapter, TrouBLE
runner, GATT task and BLE connection. The CLI in `apps/directhci-ble` calls
this library and formats the results.

`DirectHciBleCentral::connect_device` consumes the central and returns a
`BleConnection`. Standard `subscribe` writes the discovered CCCD; passive
`listen` and `listen_all` receive unsolicited values. Notification streams
can remain open while the connection performs writes.

`BleConnection::disconnect` and `DirectHciBleCentral::shutdown` wait for
teardown. The worker stops GATT and the BLE connection before releasing the
HCI session. `Drop` requests best-effort cleanup; service recovery handles
interrupted ownership changes. Usage is in the
[BLE README](../crates/directhci-ble/README.md).
