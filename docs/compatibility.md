# Compatibility model and hardware validation

## Compatibility model

DirectHCI does **not** maintain a static Bluetooth-controller VID/PID
allowlist. The privileged runtime discovers a present USB Bluetooth controller,
reads its actual PnP Hardware IDs, and can prepare a WinUSB package for one
exact `USB\VID_xxxx&PID_yyyy` (or `&MI_zz`) Hardware ID. A new eligible
Hardware ID does not require a new DirectHCI software release.

**The hardware records below are validation evidence, not an allowlist.**
Not yet verified does not mean unsupported; an observed VID/PID does not mean
the device is known to work. A generated package only makes a guarded takeover
*attempt* possible.

The current preparation path requires a fresh, healthy `USB\` devnode with
Windows Bluetooth ownership (`BTHUSB` or `IBTUSB`), Bluetooth class evidence
and one unambiguous exact USB Hardware ID. For example, a radio whose only
observed devnode is `BCMBTBUS\BLUETOOTH\...` is not a USB WinUSB target for
this backend merely because Device Manager labels it Bluetooth. If the machine
has a separate USB controller devnode, examine that node independently.

Preparation can be refused by Windows signing policy. Even after successful
Driver Store staging, takeover still requires a uniquely identified controller,
one applicable DirectHCI candidate, a safe driver rank, an applicable Windows
restore candidate and a usable recovery journal. The runtime then validates
WinUSB interface E0/01/01, one Interrupt IN Event pipe, one Bulk IN ACL pipe
and one Bulk OUT ACL pipe. Raw HCI and BLE operation require further device
behavior; Hardware ID matching cannot establish that behavior.

## Working hardware reports

These are user-reported real-hardware runs, **not** a static support list. The
record IDs below are documentation identifiers, not original log IDs or dates.
A reported working run does not establish every individual milestone where
per-stage evidence has not been retained.

| Record ID | USB ID | Common Bluetooth-family label | Overall report |
| --- | --- | --- | --- |
| `DHCI-HW-001` | `8087:0026` | AX201 Bluetooth | Working; detailed earlier takeover/HCI/BLE/restore observations below |
| `DHCI-HW-002` | `8087:0029` | AX200 Bluetooth | Working; per-stage records not yet archived |
| `DHCI-HW-003` | `8087:0036` | BE200 / Gale Peak-family Bluetooth | Working; per-stage records not yet archived |

### Recorded environment

| Record ID | DirectHCI build / revision | Windows version / build | Controller and firmware | Driver/package information |
| --- | --- | --- | --- | --- |
| `DHCI-HW-001` | Exact build/revision unknown; predates dynamic Prepare | Windows 11; exact build unknown | AX201 Bluetooth (`8087:0026`); firmware unknown | Recorded `BTHUSB/oem69.inf → WinUSB/oem183.inf → BTHUSB/oem69.inf` |
| `DHCI-HW-002` | Unknown | Unknown | AX200 Bluetooth (`8087:0029`); firmware unknown | Not archived |
| `DHCI-HW-003` | Unknown | Unknown | BE200 / Gale Peak-family Bluetooth (`8087:0036`); firmware unknown | Not archived |

### Stage evidence

| Record ID | Prepare / package | Raw HCI | BLE | Windows restore |
| --- | --- | --- | --- | --- |
| `DHCI-HW-001` | Legacy development package accepted; current dynamic Prepare not established by this run | HCI Reset and Local Version observed | Connection/ATT, GATT discovery and passive notification observed in recorded runs | Windows driver restored and observed |
| `DHCI-HW-002` | Stage-specific result not archived | Stage-specific result not archived | Stage-specific result not archived | Stage-specific result not archived |
| `DHCI-HW-003` | Stage-specific result not archived | Stage-specific result not archived | Stage-specific result not archived | Stage-specific result not archived |

The labels follow the [USB ID Repository's Intel entries](https://usb-ids.gowdy.us/read/UD/8087);
its [`0036` entry](https://usb-ids.gowdy.us/read/UD/8087/0036) notes
BE200 and related variants. A Bluetooth USB ID does not by itself prove the
exact Wi-Fi module SKU installed in a particular computer. These reports
describe observed working hardware, **not** a VID/PID allowlist or a guarantee
for every host, firmware and Windows build.

## Detailed Windows 11 / Intel AX201 evidence

The recorded controller was an Intel AX201 USB Bluetooth function
(`USB\VID_8087&PID_0026`). The detailed observations below predate the newer dynamic
package-preparation path and do not certify an arbitrary current build.

| Capability | Recorded status | Evidence / limit |
| --- | --- | --- |
| Legacy development-package staging | Verified on recorded host | Old package was already accepted by that host; **not** evidence that current local-signing Prepare succeeds |
| Temporary takeover and Windows restore | Verified on recorded host | `BTHUSB/oem69.inf → WinUSB/oem183.inf → BTHUSB/oem69.inf` |
| WinUSB transport topology | Verified on recorded host | E0/01/01; Event Interrupt IN `0x81`; ACL Bulk IN `0x82`; ACL Bulk OUT `0x02` |
| Raw HCI commands | Verified on recorded host | Reset status `0x00`; HCI version `0x0b`, revision `0x375b`, manufacturer `0x0002` |
| BLE connection and ATT | Verified in recorded runs | LE connection and ATT MTU exchange (131) |
| GATT and unsolicited values | Verified in earlier recorded runs | Service/characteristic discovery and passive notification reception; later BLE library lifecycle changes still need regression acceptance |
| Dynamic device-specific Prepare | Detailed acceptance record pending | The three IDs have user-reported working runs, but this document does not yet contain per-host CAT, staging, rank and binding logs |
| Current installer, panel and service lifecycle | Detailed acceptance record pending | User-reported working hardware does not identify the exact build and every service/uninstall failure path |

The INF names and endpoint addresses are measurements from that host, not
fixed values in the generic transport. The runtime discovers endpoints from
descriptors and selects drivers from fresh observations.

The AX200/BE200 reports and later BLE/packaging changes do not yet have
complete, revision-pinned acceptance logs here. For a future record, capture
the date, Windows build, DirectHCI revision, controller model/USB ID, firmware
when known, package/driver versions, each operation's result and final
`WindowsOwned` state using the [Windows hardware test plan](windows-test-plan.md).
Write **unknown** for unavailable metadata instead of inferring it from a USB ID
or another run. Review instance IDs, serials and local paths before publishing
raw diagnostics.

## Other paths

The separate dedicated-WinUSB-controller path has not completed hardware
acceptance. A read-only probe on the AX201 while it remained bound to
`BTHUSB` found no already-WinUSB-bound candidate. UsbDk was not installed
on that recorded host and is not the implemented takeover backend. Historical
comparison belongs in [ownership-survey.md](ownership-survey.md).
