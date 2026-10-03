# Architecture

DirectHCI separates privileged Windows controller ownership from the Raw HCI
data path and from any Bluetooth host or device-specific application protocol.

```text
Rust consumer / diagnostic CLI / Control Panel
                    │
             directhci-client
                    │ versioned local Named Pipe
                    ▼
              directhcid
                ├─ control plane: discovery, preference, preparation,
                │                 ownership, recovery, IPC authorization
                └─ data plane: Raw HCI Command / Event / ACL
                    │
             directhci-windows
                    │ SetupAPI + WinUSB
                    ▼
             USB Bluetooth controller

Optional BLE consumer path:
application → directhci-ble → TrouBLE → directhci-bt-hci
            → directhci-client → directhcid
```

## Control plane

`directhcid` is the privileged controller owner. It lists freshly observed
USB Bluetooth candidates, saves the preferred ControllerId, prepares
device-specific WinUSB packages, and controls temporary acquisition and
Windows restoration. The Control Panel uses the client SDK for runtime
operations and Windows SCM for service state; it does not rebind devices
itself.

`directhci-windows` handles PnP enumeration, exact-devnode driver selection,
protected journal storage and recovery. A ControllerId identifies a physical
controller using multiple topology/identity observations, not just a VID/PID
or display name. Before a privileged operation, the backend re-enumerates
and requires a unique current match. The runtime currently permits one
active Raw HCI writer session globally.

Prepare and acquire are different transitions. Preparation stages a driver
candidate and leaves Windows Bluetooth active. Acquire checks identity,
candidate/rank, recovery and journal conditions before temporarily binding
WinUSB. The journal records intent before driver-changing side effects.
Release, owning-client disconnect and service stop enter the restore path;
the journal is cleared only after Windows ownership is freshly confirmed.
See [driver provisioning](internals/driver-provisioning.md),
[ownership and recovery](ownership-and-recovery.md) and
[failure model](failure-model.md).

## Data plane

The runtime owns `RawHciSession`, its WinUSB handle, receive workers and
pending commands for the duration of the client session. HCI Command
responses are correlated server-side; unsolicited Events and ACL packets
remain available to the owning client. Shutdown stops new I/O, cancels and
joins workers, closes WinUSB handles, then performs Windows-driver
reconciliation.

The local Named Pipe uses a bounded, versioned protocol. Diagnostic queries
are available to authenticated local users; acquisition and Raw HCI require
administrator membership. There is no TCP or HTTP listener. See the
[SDK guide](sdk.md) for the consumer boundary.

`directhci-bt-hci` adapts the client SDK to `bt-hci`.
`directhci-ble` uses TrouBLE for BLE Central, ATT and GATT operations.
BLE is one optional consumer of Raw HCI, not the reason for or the limit of
the controller runtime.

## Boundaries that must remain intact

- Control Panel and CLI do not own WinUSB handles or bypass the service for
  normal sessions. Offline recovery is an explicitly separate tool.
- The Raw HCI runtime does not implement vendor-specific GATT commands,
  peripheral protocols or application state.
- The BLE library does not own Windows driver installation, journal state
  or recovery decisions.
- An exact Hardware ID allows a *preparation attempt*; only the later USB
  topology, HCI and restore observations establish usable hardware behavior.
