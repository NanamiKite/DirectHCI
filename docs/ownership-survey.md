# Ownership backend survey

> **Historical research — not the current installation or controller-preparation guide.**
> This survey preserves earlier options and proposals. In particular, its
> “universal INF” proposal below was superseded by dynamic, exact-Hardware-ID
> device-specific preparation. For current behavior use
> [installation](installation.md), [compatibility](compatibility.md) and
> [driver provisioning](internals/driver-provisioning.md).

Historical design notes, reviewed 2026-10-01. The comparison led to the
temporary WinUSB rebind backend. Implementation proposals below describe the
options considered at that stage; current behavior is in
[architecture.md](architecture.md), and test results are in
[compatibility.md](compatibility.md).

## Product modes and non-goals

The design considered two ownership modes with shared identity, diagnostics
and a single writer, but different idle states.

Dedicated Controller Mode uses a separate USB controller intentionally
provisioned for WinUSB:

```text
system controller -> WindowsOwned
dedicated controller -> DirectHciReady -> DirectHciOwned(lease) -> DirectHciReady
```

The dedicated controller remains WinUSB-bound when no consumer is active. A
runtime or client crash must close/cancel userspace I/O, but does not require
restoring `BTHUSB`.

Takeover Mode operates on a controller that Windows normally owns:

```text
Windows-owned -> temporary exclusive use -> Windows-owned
```

This is a historical conceptual diagram, not a list of legal Rust ownership
phase transitions. The implemented phases and failure paths are described in
[ownership and recovery](ownership-and-recovery.md).

It additionally requires driver/PnP transition safety, durable recovery, and
offline reconciliation. There is no shared-write state in either mode.

The route priority is:

1. Temporary device-specific WinUSB rebind for the current AX201 takeover.
2. Dedicated WinUSB as a future auxiliary development mode.
3. UsbDk runtime capture as an experimental takeover candidate.
4. A custom driver only if the preceding routes are proven insufficient.

At the time of the survey Raw HCI and service/IPC were future milestones.
They are now implemented; Intel vendor initialization and a custom driver
remain outside the current implementation. Package trust and driver staging
remain explicit user operations. The AX201 driver roundtrip has been observed
on the Windows host.

Current host observation: the system controller is an Intel AX201 USB
Bluetooth function at `8087:0026`, using `BTHUSB`. Those values describe the
current Windows observation and do not create a product-specific branch.

## A. Dedicated WinUSB controller — future auxiliary mode

### Established provisioning facts

- Microsoft documentation: `Winusb.sys` and `Winusb.dll` are in-box. Hardware
  that reports the `WINUSB` Microsoft OS compatible ID can use the in-box INF
  on Windows 8 and later. DirectHCI cannot add such descriptors to commodity
  dongle firmware.
- Microsoft documentation: a device without those descriptors needs a driver
  package whose custom INF matches the device, selects `Winusb.sys`, and
  registers a device-interface GUID. The INF/catalog still requires an
  accepted signing and Driver Store installation path even though Microsoft
  supplies the function-driver binary.
- Upstream behavior: Zadig/libwdi can select a USB device, generate a package,
  and install WinUSB with UAC elevation. This is useful for explicit developer
  provisioning, not proof of an appropriate production installer policy.

### Historical provisioning recommendation (superseded)

The following was a design-stage proposal, not a current end-user procedure.
For developer bring-up, use Zadig manually and only on an explicitly selected,
separate USB dongle. Record the controller identity and current driver first,
disconnect other identical dongles where practical, select WinUSB, and
re-observe the device after installation. This is an administrator operation;
a reboot or unplug/replug must be treated as possible rather than promised
away.

At the time of this survey, the proposed distributable approach was a small
DirectHCI-specific multi-Hardware-ID WinUSB INF/catalog package with:

- the narrowest applicable device hardware IDs;
- a DirectHCI-owned device-interface GUID;
- a distinct provider/package identity;
- an accepted production signature; and
- a separate privileged provisioning flow that reports reboot requirements.

The runtime must recognize a dedicated controller from a fresh observation:
stable controller identity plus the expected WinUSB service/package/interface,
not merely a remembered path or VID/PID. Ordinary INF model matching is based
on hardware/compatible IDs, so identical devices cannot safely be
distinguished by VID/PID alone. Enrollment must refuse ambiguity and must not
silently turn a system controller into a dedicated one.

The initial survey deferred libwdi in favor of manual provisioning. The
implemented preparation flow now bundles pinned libwdi for package generation
and signing; see [installation.md](installation.md#prepare-a-controller).
Zadig remains an external developer tool.

Microsoft OS descriptors are the best experience when the device manufacturer
already supplies them. They are not a provisioning mechanism DirectHCI can
retrofit onto an ordinary Bluetooth-class dongle.

### Raw HCI transport evidence

The Bluetooth USB HCI mapping used by both surveyed implementations is:

| HCI traffic | USB transfer |
| --- | --- |
| Command TX | class control OUT on endpoint zero, device recipient |
| Event RX | interrupt IN |
| ACL TX | bulk OUT |
| ACL RX | bulk IN |

BTstack's Windows implementation directly uses SetupAPI and WinUSB. It opens
the device for overlapped I/O, initializes interface 0, discovers event/ACL
pipes by descriptor type and direction, continuously resubmits event/ACL reads,
uses asynchronous control/bulk writes, aborts data pipes during shutdown, and
then frees interface and device handles. Optional SCO support acquires another
interface and alternate setting. Its non-commercial default license makes it a
behavioral reference only unless a separate commercial license is obtained.

Dolphin uses libusb rather than direct WinUSB. Its current adapter validates
Bluetooth class metadata or explicit device selection, claims interface 0,
submits persistent asynchronous interrupt and bulk reads, queues command/ACL
writes, tracks HCI command credits, cancels transfers, drains completion
callbacks, releases the interface, and closes the handle. It also contains
consumer- and vendor-specific behavior that does not belong in DirectHCI.
Dolphin is GPL-2.0-or-later and is reference-only for the present project.

DirectHCI should discover endpoints from descriptors rather than depend on the
commonly seen `0x81`/`0x82`/`0x02` addresses. SCO/isochronous transport is not
required for the first command/event/ACL milestone.

### Direct WinUSB versus libusb

Direct WinUSB is the default implementation direction. It uses the in-box API,
keeps file/interface/OVERLAPPED ownership visible to the future runtime lease,
avoids a C runtime dependency, and is sufficient for the small Bluetooth HCI
transfer set. The cost is that DirectHCI must correctly implement descriptor
validation, asynchronous completion, cancellation, removal, and shutdown.

libusb is mature and can reduce transfer plumbing, supports WinUSB, and is
LGPL-2.1-or-later. It remains a possible optional adapter if direct WinUSB
proves disproportionately complex or if a later cross-platform transport makes
the abstraction valuable. It is not selected as a default core dependency now.

### Relevant WinUSB limits

- Microsoft requires the device file to be opened for overlapped I/O before
  `WinUsb_Initialize`; associated interfaces and alternate settings need
  explicit handling.
- WinUSB supports asynchronous control, bulk, and interrupt operations, but
  every outstanding operation needs independent lifetime/cancellation state.
  Shutdown must stop new submissions, cancel or abort data pipes, drain
  completions, then free WinUSB and file handles.
- `WinUsb_ResetPipe` clears a pipe stall/data toggle; it is not a physical USB
  device reset. The libusb Windows backend documents that WinUSB cannot issue a
  true USB device reset and cannot select a configuration other than the first.
- Transfer timeout is zero by default. Removal, suspend/resume, and stale
  interface paths must be surfaced as lifecycle events followed by fresh
  enumeration, not hidden as generic I/O errors.
- WinUSB is not intended for multiple concurrent applications. That limitation
  aligns with DirectHCI's single active writer but still requires enforcement
  by the runtime.
- WinUSB power policy is configurable. The first transport must keep power
  policy conservative and record suspend/resume behavior before enabling
  selective-suspend optimizations.

### Minimum DirectHCI implementation

The first transport can remain inside `directhci-windows`; no new crate is
needed yet. The minimum implementation units are:

1. read-only discovery of a WinUSB-bound Bluetooth controller and its expected
   device-interface path, correlated to `ControllerIdentity`;
2. RAII ownership of the device file and WinUSB interface handles;
3. interface/endpoint descriptor validation and endpoint-role selection;
4. asynchronous command, event, ACL-in, and ACL-out operations;
5. cancellation, completion draining, surprise-removal, and deterministic
   shutdown;
6. structured errors and trace hooks without a Host Stack dependency.

Backend/runtime logic remains below the CLI. The implemented `directhcid`
service owns the session and workers; CLI, SDK, consumer applications, and
the Control Panel are clients. The Control Panel does not own the controller
and currently requires the service to be stopped before its window closes.

## B. Temporary device-specific WinUSB rebind — selected backend

The selected takeover path chooses a staged driver for
one freshly identified devnode, re-observes after re-enumeration, and reconciles
back to a freshly applicable Windows driver. It does not use a hardware-ID-wide
force update or blindly reinstall a cached INF. Durable journal storage and
offline recovery are available before the first main-controller experiment.

Microsoft documentation establishes that driver installation is privileged,
driver packages are ranked, and device-install APIs can report a reboot
requirement. A package observed today is not automatically the package Windows
should use after a future update.

## C. UsbDk runtime capture — experimental

### Established facts and risk evidence

- Upstream API contract: `UsbDk_StartRedirect` is intended to detach a USB
  device for exclusive access and return it when the redirect handle closes.
- Upstream installation documentation: UsbDk installs a system USB filter,
  updates the USB device stack, and triggers re-enumeration of every USB device
  on the machine. Installation is not local to one selected controller.
- Upstream issue evidence: UsbDk issue #115 reports UsbDk 1.0.22 on Windows 11
  producing `WDF_VIOLATION` when opening a power-managed Bluetooth USB device.
  Follow-up reports mention Windows 10 and failure occurring at
  `UsbDk_StartRedirect`. Maintainer discussion attributes the affected path to
  a Dx-to-D0 transition interacting with some WDF devices. This is a public
  report, not a claim that every controller or Windows build fails.
- Upstream libusb guidance discourages UsbDk for general use because of reported
  stability problems and recommends WinUSB where persistent binding is
  acceptable.
- UsbDk v1.00-22 is from 2020. Its general compatibility statements do not prove
  safe takeover on the current Windows 11 build, signing policy, or HVCI
  configuration.

Current host observation: `UsbDkHelper.dll` and the `UsbDk` service are absent,
so enumeration and correlation were not attempted. DirectHCI will not install
UsbDk automatically.

The Apache-2.0 read-only adapter and `doctor` diagnostics remain supported code.
UsbDk retains research value for an experimental backend, community hardware
validation, and isolated lab testing. It is neither declared unsupported nor
treated as safe, and is not the recommended production/default backend.

No `StartRedirect` experiment should run on the primary AX201 development host
by default. Reconsider it only on a sacrificial or readily recoverable Windows
machine with non-Bluetooth input, recovery media, kernel crash dumps/debugging,
an explicit UsbDk version and Windows/security configuration, and a noncritical
controller. A VM can exercise some installation/API paths but does not prove
physical Windows USB-stack safety.

## D. Custom driver — last resort

Do not begin KMDF, UMDF, or filter-driver work unless both direct WinUSB
provisioning and the independently evaluated takeover routes fail concrete
requirements. No such evidence currently exists.

## Route comparison

| Property | Dedicated WinUSB | Temporary WinUSB rebind | UsbDk capture |
| --- | --- | --- | --- |
| Formal mode | Dedicated | Takeover | Experimental takeover |
| Meets AX201 takeover cycle | No | Yes, observed on the recorded host | Intended to; unsafe/unknown here |
| Runtime driver mutation | None after provisioning | Yes | Redirect through installed filter |
| Desired available state | `DirectHciReady` | `WindowsOwned` | `WindowsOwned` |
| Crash recovery | Close/cancel I/O; keep WinUSB binding | Persistent binding requires reconciler | Handle return contract plus lab validation |
| Role | Future auxiliary | Implemented backend | Experimental |

## Route decision and sequencing

Phase 1 proceeds through **temporary, device-specific WinUSB rebind of the
current AX201**. Dedicated Controller Mode remains a useful future auxiliary
path, but it does not replace the required Windows-owned takeover/release
cycle. Both eventually feed the same runtime-facing WinUSB transport boundary.

The read-only WinUSB readiness/open probe is implemented inside
`directhci-windows`. It discovers the registered application interface,
requires exact instance-ID correlation, opens with overlapped I/O, and
validates descriptor/pipe topology. The AX201 subsequently passed this gate
**after the temporary takeover**, including E0/01/01 and event/ACL pipes.
A separate Dedicated WinUSB dongle still has not been validated.

The read-only planner, durable intent, offline recovery, and verified
Windows-driver reconcile path were built before the first AX201 rebind. The
takeover round trip and Raw HCI are now implemented. See
[`ownership-and-recovery.md`](ownership-and-recovery.md).

The existing `directhci doctor` UsbDk gate remains read-only. The current host
result is helper missing, service missing, enumeration not attempted, and
correlation not evaluated. That is an environment fact, not a probe failure,
and does not authorize installing UsbDk.
