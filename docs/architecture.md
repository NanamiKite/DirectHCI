# Architecture

## Current implementation boundaries

DirectHCI now contains Windows controller discovery, M1 temporary ownership,
M2 Raw HCI, an M3 named-pipe runtime/client SDK, a generic BLE Central library,
and a native Windows Control Panel. The AX201 takeover/restore and Raw HCI path
have been exercised on a Windows host; the current revision's BLE library and
installer/GUI changes still need separate regression acceptance (see
[compatibility.md](compatibility.md)).

```text
ControllerIdentity       Physical-controller matching evidence
ControllerObservation    Current Windows PnP/driver view
OwnershipJournal         Durable transition intent, not the source of truth
RuntimeControllerSession Owns takeover and RawHciSession during one lease window
directhcid               Privileged, single-active-writer runtime
directhci-client          Versioned local IPC client SDK
directhci-ble             Generic BLE Central/GATT consumer library
```

Identity and observation are separate because driver changes and PnP
re-enumeration can replace interface paths or devnodes. An opaque DirectHCI ID
is only an index into recorded identity evidence; it is not sufficient proof
for a privileged operation.

`DEVPKEY_Device_ContainerId` is supporting evidence, not an unconditional
physical-device key. The Windows system Container ID and the all-zero invalid
GUID are normalized away. The v0 controller ID uses, in order, a genuine USB
serial, USB/PCI location, or the current Windows instance ID, always scoped by
VID/PID. Driver, INF, service, status, and interface path never participate.

## Product modes and ownership invariant

The product model distinguishes Dedicated Controller Mode and Takeover Mode.
Only Takeover Mode has current AX201 hardware acceptance.

```text
Dedicated:
DirectHciReady -> DirectHciOwned(lease) -> DirectHciReady

Takeover:
WindowsOwned -> DirectHciOwned(lease) -> WindowsOwned
```

`DirectHciReady` means the dedicated controller remains intentionally bound to
WinUSB but has no active client write lease. It does not mean that Windows
Bluetooth should reclaim it. In takeover mode the available desired state is
`WindowsOwned`.

The current runtime allows one active writer session globally. If ownership
is transitioning, missing, unexpected, or ambiguous, HCI I/O is not
permitted. Dedicated Mode is a documented auxiliary design, not a validated
provisioning or parallel-ownership implementation.

## Runtime controller session

The implemented `RuntimeControllerSession` in `directhci-windows` owns
takeover plus `RawHciSession`. `directhcid` owns that session on behalf of
one named-pipe client; the client SDK never receives WinUSB or driver handles.
The durable M1 journal covers interruption before or after driver rebinding.

Release is a desired-state operation and must be idempotent:

```text
stop new I/O
cancel and join workers
close/release the backend ownership guard
observe again
reconcile to the mode's available state
mark the durable transition complete
```

`Drop` may perform best-effort local cleanup, but correctness also depends on
OS handle teardown, durable transition intent, startup recovery, and the
offline reconciler.

- A takeover-mode client disconnect changes the desired state to
  `WindowsOwned`.
- A service crash is reconciled at next service start or by offline recovery.
- A missing controller remains a pending recovery item; DirectHCI must not
  substitute another device with the same VID/PID.
- Repeated release and recover calls re-observe and converge on the same desired
  state instead of replaying inverse steps.

Dedicated mode is different only in its available state: the device remains
bound to WinUSB, but no client has an active write lease.

## Recovery

The durable record contains controller evidence, backend kind, lease ID,
transition phase, original observation, and recovery metadata. It records
intent and history only.

```text
desired state
    + current controller/backend/PnP observation
    -> minimum safe reconciliation action
    -> new observation
```

An old INF name is not an instruction to reinstall that INF. Driver selection
may have changed after Windows Update or administrator action.

For the temporary-WinUSB backend, the minimal durable phases are
`WindowsOwned`, `AcquirePrepared`, `RebindingToDirectHci`, `DirectHciReady`,
`DirectHciOwned`, `RestoringWindows`, and `RecoveryRequired`. A phase is
persisted before its external side effect. Restart recovery then observes the
exact physical controller and current applicable drivers before deciding the
next action. See [`temporary-rebind.md`](temporary-rebind.md).

## Runtime and client boundary

`directhcid` is now the product ownership boundary. Normal controller queries
and Raw HCI sessions use the versioned local named-pipe protocol through
`directhci-client`. Explicit development takeover commands and offline
recovery remain direct library entry points so recovery does not depend on a
healthy runtime service.

```text
CLI / Control Panel / Client SDK / Consumer
                         |
                  versioned local IPC
                         |
                 DirectHCI Runtime
                         |
          controller session and backend
```

The runtime owns the temporary takeover object, `RawHciSession`, WinUSB
handles, and I/O workers. One client connection may own the single active
writer session. A broken owning pipe, explicit release, console shutdown, or
SCM stop closes Raw HCI before restoring Windows ownership. On startup, the
runtime reconciles an unresolved M1 journal before accepting acquisition.

The v1 pipe is byte-framed and bounded. Control messages use small JSON
payloads; HCI Command/Event and ACL payloads stay binary. Authenticated local
users may query diagnostics, while administrator group membership is required
for acquire and Raw HCI operations. No network listener is created.

The Windows Control Panel is another `directhci-client` consumer. It reads
runtime/controller state through IPC, queries service status through SCM, and
does not own the controller lifecycle. It requests administrator approval
when launched without an elevated token. It refreshes status periodically on
a background worker. Closing the panel requests a service stop and waits for
SCM to report `Stopped`; an owning client disconnect restores Windows
Bluetooth without stopping the service. The daemon validates and persists the
single preferred ControllerId
in `%ProgramData%\DirectHCI\config.json`; this
preference never substitutes for fresh identity validation during takeover.
Changing it while an active session exists is refused. The Control Panel's
Restore Windows action delegates to the runtime's existing offline recovery
path after stopping an active session.

Windows PnP and WinUSB APIs remain outside `directhci-core`. Vendor-specific
code must not enter controller identity or generic USB transport. This
boundary remains independent of the Control Panel and consumer Host Stack.

## Reusable BLE Central boundary

The `directhci-ble` library is the generic Rust BLE Central/GATT consumer
layer. It owns the DirectHCI client session, the `directhci-bt-hci` adapter,
the TrouBLE runner and GATT task, and an active BLE connection. Its public API
returns structured advertisements, discovered GATT attributes, raw
characteristic values, and notification/indication events.

```text
Rust consumer
    |
directhci-ble                 generic BLE Central / GATT API
    |
directhci-bt-hci              bt-hci controller adapter
    |
directhci-client              versioned local IPC SDK
    |
directhcid                    privileged ownership + Raw HCI runtime
    |
Bluetooth controller
```

The `directhci-ble-cli` package builds the `directhci-ble` executable. It is
only a command-line frontend: argument parsing, operation selection, and human
readable formatting remain there. It does not construct a TrouBLE stack or own
Raw HCI directly.

Standard subscription and passive listening remain separate public
operations. Subscription writes a discovered CCCD; passive `listen` and
`listen_all` only observe unsolicited values. A notification stream can
remain active while the same `BleConnection` performs writes, which lets an
application compose listener-ready, write, and receive sequences without a
device-specific protocol in this crate.

`BleConnection::disconnect` and `DirectHciBleCentral::shutdown` are the
normal awaited teardown paths. Dropping either object only requests
best-effort shutdown; it is not the primary recovery mechanism. The worker
owns all TrouBLE borrows and stops its GATT task and connection before
releasing the DirectHCI Raw HCI session.
