# Ownership and recovery

The temporary takeover backend binds one exact USB Bluetooth controller to
WinUSB for a session, then attempts to restore its Windows Bluetooth driver.
The service records each transition in a durable journal so startup or
offline recovery can reconcile an interrupted operation. The journal is
evidence of intent, not a substitute for a fresh PnP observation. Recorded
hardware results are in [compatibility.md](compatibility.md); user-visible
outcomes are summarized in [failure-model.md](failure-model.md).

The Control Panel's **Prepare Controller** action generates and stages a
driver package through the service after certificate trust consent. Setup
only installs the provisioning component. Preparation and takeover are
separate operations; this diagnostic tests the driver roundtrip without HCI:

```text
directhci takeover roundtrip <controller-id> --execute
```

Normal Raw HCI ownership runs inside `directhcid`; the explicit
`takeover hci-info ... --execute` and `recover --offline` commands remain
development/recovery entry points. See [architecture.md](architecture.md).

## Scope and invariant

The detailed trace below used an Intel AX201 Bluetooth USB function
(`USB\VID_8087&PID_0026`) normally using `BTHUSB`. Preparation generates an INF
for one freshly observed Hardware ID. User-confirmed working runs for
`8087:0029` and `8087:0036` are listed in [compatibility](compatibility.md);
their per-stage logs have not yet been archived. The ownership sequence is:

```text
WindowsOwned
  -> acquire intent persisted
  -> device-specific DirectHCI WinUSB binding
  -> DirectHciReady
  -> DirectHciOwned
  -> reconcile desired WindowsOwned
  -> WindowsOwned
```

The package may be hardware-specific without making `directhci-core` or the
WinUSB transport AX201-specific. Rebinding/provisioning ends when the same
physical controller becomes `DirectHciReady`; the existing generic WinUSB
open/readiness layer begins there.

## Read-only planner

`directhci takeover plan <controller-id>` performs a fresh controller
enumeration and requires exactly one current observation for the opaque ID. It
opens that exact PnP instance with SetupAPI, builds its compatible-driver list,
and reports:

- current device/service/INF/provider/version;
- applicable driver packages and SetupAPI rank;
- whether exactly one expected DirectHCI package is applicable;
- the best currently applicable non-DirectHCI recovery candidate;
- package presence in the Driver Store, package rank/default-selection risk,
  privilege, unresolved-journal, and recovery blockers.

The planner sets `changes_applied` to `false` and contains no mutation primitive.
`DI_FLAGSEX_ALLOWEXCLUDEDDRVS` is used while building the read-only candidate
list because PnP packages are commonly hidden from manual selection.

Execution does not trust that earlier snapshot. After atomically reserving the
journal—but before persisting rebind intent or invoking `DiInstallDevice`—it
freshly verifies the still-current Windows package, unique DirectHCI candidate,
unique recovery candidate, and conservative rank gate. Drift cancels without a
driver mutation and clears the acquisition record only after another healthy
Windows-owned observation.

The DirectHCI candidate must have a numerically larger (worse) rank than the
best Windows candidate. This conservative gate reduces the risk that merely
staging the package causes Windows to select it automatically. Rank is not the
whole driver-selection algorithm, so staging followed by re-enumeration and a
reboot observation is still mandatory before takeover.

## Device-specific install primitive

The implemented mutation primitive is `DiInstallDevice` with:

- an `SP_DEVINFO_DATA` resolved from a fresh, exact device instance ID;
- the already staged DirectHCI compatible-driver entry for that devnode;
- `DIIDFLAG_INSTALLNULLDRIVER` absent;
- reboot-required output handled as a failed online acquire, not silently
  ignored.

Microsoft documents this API for installing a pre-staged package on a
specified device, including the case where same-type devices need
device-instance-specific packages. In contrast,
`UpdateDriverForPlugAndPlayDevices` is hardware-ID oriented and is not the
lease primitive.

Package staging remains machine-wide Driver Store state, and hardware-ID
matching remains global eligibility. “Device-specific” describes the binding
operation, not the scope of package installation. DirectHCI must therefore
verify rank and staging behavior before the first bind.

## Preparation boundary

The Control Panel's **Prepare Controller** action selects one exact Hardware
ID from a freshly observed, eligible USB Bluetooth controller and stages a
matching WinUSB package. This is not an acquisition: it does not rebind the
device. The older class-compatible-ID INF is not the current path, and the
multi-Hardware-ID development package is migration-only. Generation,
libwdi signing, trust, Windows staging policy and cleanup limits are in
[driver-provisioning.md](internals/driver-provisioning.md).

The planner still requires an applicable package, safe rank, exact physical
identity, a Windows recovery candidate and a usable journal before
`DiInstallDevice`. After actual takeover, the WinUSB E0/01/01 descriptor and
required Event/ACL pipes must pass runtime readiness checks.

## Durable acquire ordering

```text
Persist recovery intent
    → perform exact-device PnP transition
    → re-enumerate and verify the observed state
    → commit the journal phase
```

Before any external side effect, the runtime reserves the controller by
atomically creating a previously absent `AcquirePrepared` journal. The
no-replace create prevents two concurrent acquisitions from both reserving the
controller. Later phases write a complete temporary file in the same local
filesystem, flush it, then atomically replace the previous record with
write-through semantics on Windows. The planner also creates, flushes, and
removes a harmless temporary probe so an unwritable journal location blocks
execution before device mutation. The record is intent and history; Windows is
always re-observed.

The minimum durable phases are:

| Phase | Meaning after restart | Recovery action |
| --- | --- | --- |
| `WindowsOwned` | No active acquisition | Confirm or diagnose drift |
| `AcquirePrepared` | Intent persisted; bind may not have started | Observe; clear only if still safely Windows-owned, otherwise reconcile |
| `RebindingToDirectHci` | Side effect may have happened | Observe exact physical controller; never assume either driver |
| `DirectHciReady` | DirectHCI package observed and WinUSB readiness passed | With no live service owner, restore Windows |
| `DirectHciOwned` | A runtime session was active | Stop/close any surviving local I/O, then restore Windows |
| `RestoringWindows` | Restore side effect may have happened | Re-observe and continue convergence |
| `RecoveryRequired` | Automatic progress was unsafe or failed | Preserve evidence; retry only after a fresh unambiguous observation |

Journal the next dangerous phase *before* issuing its side effect. After an API
returns, never infer success from the return value alone: wait for PnP,
re-enumerate, re-correlate identity, validate service/package/interface, then
persist the observed stable phase. A kill at every boundary therefore leaves a
record that causes observation and reconciliation rather than inverse replay.

The journal schema stores lease ID, schema version, backend, phase, desired
state, controller identity evidence, historical pre-acquire observation,
DirectHCI package identity, timestamps, and owner/session metadata. Historical
INF and interface paths are diagnostic evidence only.

## Restore and reconciliation

The desired takeover release state is `WindowsOwned`, not “install the old
`oem69.inf`”. Recovery freshly enumerates compatible drivers for the exact
physical controller, excludes the DirectHCI package, and identifies current
valid Windows candidates. It selects the still-applicable pre-acquire candidate
first, otherwise the unique best-ranked non-DirectHCI candidate, and explicitly
binds it to the exact devnode with `DiInstallDevice`. Fresh observation must
then match that selected package and a working Windows Bluetooth service.

There is no single documented API meaning “select the best driver except this
package”. SetupAPI rank is observable, but DirectHCI must make the exclusion
and ambiguity policy explicit. If no unique/current valid Windows candidate is
available, automatic recovery stops in `RecoveryRequired`; it does not install
the historical INF blindly.

Windows Update or an administrator may change the preferred Intel package.
That is expected drift. Recovery first selects the exact pre-acquire package
only when it is still present as one unique, currently applicable candidate.
If it has disappeared, recovery selects the unique best-ranked fresh
non-DirectHCI candidate. Missing ranks or a best-rank tie are ambiguous and
leave the journal in `RecoveryRequired`; no historical INF is installed by
name outside the fresh candidate list.

`DiUninstallDriver` is not a normal release primitive because removing a Driver
Store package is broader than rebinding one devnode. DirectHCI package cleanup
belongs to uninstall/maintenance after all managed controllers are reconciled.
Likewise, persistent disable is forbidden. If a future path needs a transient
restart, `CM_Disable_DevNode(..., 0)` must be distinguished from
`CM_DISABLE_PERSIST`, and the persistent flag must not be used in normal
ownership flow.

## Offline recovery boundary

The formal offline entry point is `directhci recover --offline`. Its
library path depends only on:

- journal parsing and atomic update;
- controller identity enumeration and exact correlation;
- compatible-driver observation;
- device-specific driver reconcile primitives;
- post-action PnP observation and structured diagnostics.

It must not load Raw HCI, Intel initialization, IPC, GUI, or consumer code. It
is idempotent: repeated runs observe and converge toward `WindowsOwned`. A
missing device keeps the journal pending. An ambiguous identity or driver
choice produces `RecoveryRequired` without modifying any candidate. It also
refuses to run while the journal's owning process is still active.

## Hardware test prerequisites

Before testing on a new host, changed package or driver/security configuration:

1. prepare and stage the device-specific package;
2. check that the read-only planner finds one DirectHCI candidate and one valid
   Windows recovery candidate for the exact controller;
3. verify the DirectHCI candidate is lower preference, and staging plus reboot
   does not auto-bind it;
4. check that the journal directory is writable and no unresolved journal exists;
5. make the offline recovery command available;
6. arrange non-Bluetooth input, administrator access, recovery media and a
   reboot path;
7. save `controllers --direct --json`, `doctor --json`, the candidate list,
   Windows build and package identities.

The implemented controlled acceptance sequence is: preflight and journal;
device-specific bind; wait/re-observe; validate exact identity, WinUSB service,
DirectHCI application interface and existing WinUSB readiness; perform **no HCI
I/O**; mark restore intent; explicitly bind the freshly selected Windows
candidate; wait/re-observe `BTHUSB`/working Windows Bluetooth; clear the journal
only after verified convergence. Separate kill-point tests follow only after
the basic round trip succeeds.

## Implemented failure behavior

- Before the first journal write: refuse without changing the driver.
- After `AcquirePrepared` is durable but before rebind: offline recovery uses
  fresh observation and clears or reconciles the record.
- `DiInstallDevice` error after invocation: immediately attempt explicit
  Windows-driver restore and preserve both errors if recovery fails.
- DirectHCI-side re-enumeration or readiness failure: immediately attempt
  restore.
- `NeedReboot=TRUE`: never reboot automatically; persist `RecoveryRequired` and
  retain the journal. On the next startup or offline recovery, freshly locate
  the same physical controller. If a later boot is confirmed, the original
  Windows driver is active and healthy, and the DirectHCI interface is absent,
  reconcile to `WindowsOwned` and clear the journal without reinstalling the
  already-active driver. Before a confirmed later boot, explicitly reselect
  the Windows candidate; uncertain observations retain the journal.
- Missing/ambiguous recovery candidate, restore failure, or final verification
  failure: retain the journal and report `directhci recover --offline`.
- Verified `BTHUSB`, healthy device status, and absent DirectHCI application
  interface: persist `WindowsOwned`, then clear the journal. A crash before
  removal leaves a satisfied journal that offline recovery can clear.
