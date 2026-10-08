# Temporary device-specific WinUSB rebind

The temporary takeover backend binds one controller to WinUSB for a session,
then restores its Windows Bluetooth driver. The service records each
transition in a durable journal so startup or offline recovery can finish an
interrupted restore. Recorded hardware results are in
[compatibility.md](compatibility.md).

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

The tested controller was an Intel AX201 Bluetooth USB function
(`USB\VID_8087&PID_0026`) normally using `BTHUSB`. Preparation generates an INF
for one freshly observed Hardware ID. Other controllers still need firmware,
HCI and restore testing. The ownership sequence is:

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

## Device-specific WinUSB preparation

The former `USB\Class_E0&SubClass_01&Prot_01` class-compatible-ID INF
failed Windows Inf2Cat B2.6.4.9 and is not used. The static multi-Hardware-ID
development package is migration-only. The runtime instead selects one exact
Hardware ID from a fresh USB Bluetooth controller observation, then, after
administrator consent, generates a single-model INF. libwdi generates and
self-signs its CAT with a one-time certificate. The privileged service stages
it with SetupAPI only; it never calls `/install` or rebinds during preparation.

The package uses the in-box `WinUSB.sys`, preserves DirectHCI's application
interface GUID and protected device security descriptor, and includes no
custom kernel driver. The private key must be destroyed after signing; the
public Root/TrustedPublisher certificate can remain while the package exists.
Windows may still refuse staging under its current code-integrity policy;
`PackageStagingRejectedByWindows` preserves that real SetupAPI error and
leaves the original Bluetooth binding unchanged.

Preparation is not takeover. The planner still requires an applicable
candidate, safe rank, exact physical identity, Windows recovery driver and
writable ownership journal before `DiInstallDevice`. After explicit takeover,
WinUSB E0/01/01 and required endpoint readiness remain mandatory. The legacy
`build-dev-driver.ps1` flow is retained only for development diagnostics.

End-user machines do not need EWDK, MSBuild, Inf2Cat, SignTool, or
developer PowerShell scripts. The build host prepares the pinned libwdi DLL once; the
installer bundles it. The service may add one explicitly approved local
certificate to Root and TrustedPublisher during each package preparation.
That trust does not guarantee Windows will accept the package under Secure
Boot. DirectHCI never changes Secure Boot, TESTSIGNING, or BCD, and a Windows
rejection is reported without initiating takeover.

## Durable acquire ordering

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
pre-acquire controller permissions, DirectHCI package identity, timestamps,
and owner/session metadata. Historical
INF and interface paths are diagnostic evidence only.

### Controller permission restoration

The WinUSB INF restricts its device to SYSTEM/Administrators. Returning to
`BTHUSB` does **not** prove that Windows radio clients regained access: a
missing `SecuritySDS` device property does not prove a healthy live radio DACL.

Before either runtime or CLI takeover, DirectHCI now reads and durably saves:

- the exact device instance ID;
- the device's `DEVPKEY_Device_SecuritySDS` override, including confirmed absence;
- owner/group/DACL from the exact active BTHPORT radio handle.

Read errors are not treated as absence. Known SYSTEM/Administrators-only
DirectHCI permissions cannot be adopted as a normal Windows baseline. All
radio handles used for capture close before the rebind. Immediately before
rebind, permission drift is checked alongside driver/identity/rank drift.

After the selected Windows driver has returned and the DirectHCI interface
is inactive, recovery restores the original override (or removes only the
known DirectHCI override when it was originally absent), then restores the
saved live radio DACL. It does not grant a hardcoded user, service, LocalService
or Everyone access, change live owner/group/SACL, or modify a setup-class ACL.
Unexpected permission changes are reported as conflicts, not overwritten.
Readback compares ACE content/order and protection, tolerating AI bookkeeping
and generic-right mapping rather than comparing entire SDDL strings.

Driver ownership and radio-interface readiness are separate observations.
Before permission replay, recovery waits for one present BTHPORT interface
on the exact same device instance and reads its actual permissions. After a
driver restore, this shares the existing 20-second driver/radio readiness
budget, polling every 250 ms and proceeding as soon as readiness is observed.
Only a successful interface enumeration returning zero interfaces is retried;
multiple interfaces, API errors, identity/binding changes and unsafe ownership
fail closed with a stage-specific error. This wait does not write permissions,
install a driver or restart a device. Interface publication alone is never
enough to clear the journal: permission replay and final verification still
have to succeed.

An ended client session is not proof of recovery. The Control Panel explicitly
labels an empty session with incomplete recovery and shows the failure reason.
Its existing **Restore Windows** action can retry journal recovery inside the
running service after the session has ended; stopping/starting the service is
not required. Status polling and new acquire requests do not silently retry
privileged recovery. A persistent readiness failure keeps takeover blocked.

An absent saved override is restored by clearing the underlying `SPDRP_SECURITY`
property with `SetupDiSetDeviceRegistryPropertyW` and a NULL buffer/zero size,
then reading `DEVPKEY_Device_SecuritySDS` back to confirm absence. This follows
the [documented property-clearing contract](https://learn.microsoft.com/en-us/windows/win32/api/setupapi/nf-setupapi-setupdisetdeviceregistrypropertyw);
it does not install an empty descriptor or a null DACL. A reported Windows host
rejected deletion through the string property (`SecuritySDS` plus
`DEVPROP_TYPE_EMPTY`) with `ERROR_INVALID_DATA`, aborting recovery before live
radio permissions were restored. A saved nonempty descriptor still uses the
typed SDDL setter. Neither deletion errors nor readback errors are ignored.

Matching ACLs alone do not undo a Windows client initialization that happened
while the radio was still restricted. When either permission needs replay,
recovery first persists `windows_stack_restart_pending`, restores and verifies
the baseline, closes every radio handle, then requests **one device restart**
of that same, already Windows-bound controller. It uses
[`DIF_PROPERTYCHANGE` / `DICS_PROPCHANGE`](https://learn.microsoft.com/en-us/windows/win32/api/setupapi/ns-setupapi-sp_propchange_params),
not `DiInstallDevice`, a driver-package reinstall, or a machine reboot.
Permissions are never relaxed on an active WinUSB binding. No class ACL,
service configuration, pairing keys, or unrelated device is changed.

After the request, read-only polling is bounded to 20 seconds and checks the
same controller and Windows package, an inactive DirectHCI interface, no
pending restart, and the saved permissions on the freshly opened radio.
Polling does not repeat the restart or add access rights. A veto, native API
error, system-restart requirement, or failed verification keeps the journal
and reports the error. The native SetupAPI call itself has no enforced timeout.
If the process exits between ACL replay and restart verification, the persisted
intent prevents recovery from clearing the journal on ACL equality alone.
An explicit subsequent recovery can retry the unfinished step. If permissions
already match and no such intent exists, there is no additional device restart.

Normal release, failed-acquire rollback, startup/offline reconciliation and
journal cleanup require the saved permissions to match. A permission error
keeps the journal even when the driver binding has recovered. This does not
prove pairing, firmware or end-to-end Windows Bluetooth functionality. Remote
GATT service devnodes are not a fixed-count recovery prerequisite: they depend
on the peripherals and cannot be required to number exactly two.

New journals use schema **3** at the existing `ownership-journal-v1.json`
path so older runtimes cannot overlook the lease; they reject the newer schema.
This is an internal journal format change, not an application version change.
The new runtime can read schema 2 baselines; its unfinished/already-committed
restore records require the missing post-permission restart verification.
It can also read legacy schema 1, but cannot reconstruct permissions
that it never recorded. Such recovery reports `ControllerSecurityBaselineMissing`
and retains the journal instead of guessing a factory ACL or granting service
access. Installing this code does not automatically repair existing damaged
permissions without a trustworthy pre-acquire baseline. These permission
changes still require Windows hardware acceptance; no automated tests were run
for this change at the user's request.

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
Likewise, persistent disable is forbidden. The post-permission restart uses
`DICS_PROPCHANGE`, not a persistent disable/enable sequence. It does not bypass
Windows `NeedReboot` or `DN_NEED_RESTART` results.

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
  retain the journal. If offline recovery observes BTHUSB, it checks pending
  restart/interface state rather than reinstalling the INF again.
- Missing/ambiguous recovery candidate, restore failure, or final verification
  failure: retain the journal and report `directhci recover --offline`.
- Verified `BTHUSB`, healthy device status, and absent DirectHCI application
  interface are necessary but insufficient: also restore/verify the saved
  permissions and finish any pending post-permission device restart before
  persisting `WindowsOwned` and clearing the journal. A crash before removal
  leaves a satisfied journal that offline recovery can revalidate and clear.
