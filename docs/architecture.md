# Architecture

## Phase 1 boundary

Phase 1 establishes Windows Bluetooth Controller identity, current observation,
exclusive ownership transitions, release, recovery, and diagnostics. Raw HCI,
vendor initialization, IPC, and Bluetooth Host protocols remain outside this
phase.

```text
ControllerIdentity       Physical-controller matching evidence
ControllerObservation    Current Windows PnP/driver view
OwnershipBackend         One concrete ownership transition mechanism
ControllerLease          Lifetime root of DirectHCI-owned access
RecoveryRecord           Durable transition intent, never the source of truth
Reconciler               Observe current state and approach desired state
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

DirectHCI formally supports both Dedicated Controller Mode and Takeover Mode.

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

At most one active writer can exist. If ownership is transitioning, missing,
unexpected, or ambiguous, HCI I/O is not permitted. Mode is durable enrollment
policy, not something inferred from a transient interface path.

## Controller lease

The M1 ownership journal and transition executor reserve one controller across
the temporary rebind. `ControllerLease` will become the runtime root once HCI
sessions are introduced; a future transport and all of its workers must borrow
or be owned by that lease and stop before the backend ownership guard is
released.

The privileged controller manager creates leases. A client receives a lease
identifier or capability through IPC later; it does not construct the lease or
own the underlying OS handle directly.

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

The development CLI currently calls the Windows backend directly, including
the explicitly destructive `takeover roundtrip --execute` command. That is a
temporary implementation shape, not the product ownership boundary.

```text
CLI / Client SDK / Consumer / optional Control Panel
                         |
                  versioned local IPC
                         |
                 DirectHCI Runtime
                         |
          controller session and backend
```

The future runtime owns controller handles, leases, and I/O workers. A GUI or
CLI process exiting must not implicitly decide controller ownership. A future
Control Panel only needs to query controllers, enrolled mode, backend,
ownership/status, health, active consumer, and recovery state.

Windows PnP and WinUSB APIs remain outside `directhci-core`. Vendor-specific
code must not enter controller identity or generic USB transport. This
direction does not authorize implementing service, IPC, or GUI in the current
milestone.
