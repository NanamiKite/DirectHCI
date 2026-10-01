# Compatibility

This page separates recorded Windows-host observations from current-build
acceptance. A successful test on one AX201/Windows installation is not a
blanket compatibility claim for every firmware, driver, or Windows build.

| Controller | VID/PID | Host | Ownership backend | Recorded evidence |
| --- | --- | --- | --- | --- |
| Intel AX201 Bluetooth USB function | 8087:0026 | Windows 11 | Temporary, device-specific WinUSB rebind | Real BTHUSB → DirectHCI WinUSB → BTHUSB round trip; WinUSB readiness; HCI Reset, version query, LE connection, ATT MTU exchange, GATT discovery, passive notification reception, and Windows restore were observed during development. |

The AX201 round trip was observed with Intel `BTHUSB` / `oem69.inf` →
DirectHCI `WinUSB` / `oem183.inf` → `BTHUSB` / `oem69.inf`.
The WinUSB interface was E0/01/01 with event interrupt IN `0x81`, ACL
bulk IN `0x82`, and ACL bulk OUT `0x02`. These endpoint IDs are observed
values, **not** generic transport constants.

A later host run reported HCI Reset status `0x00`, HCI version `0x0b`,
revision `0x375b`, manufacturer `0x0002`, and a final
`WindowsOwned` state. The driver names/versions and instance path are
historical observations, not stable controller identity or a restore recipe.

**Current-build caveat:** a subsequent extraction of the BLE CLI logic into
`crates/directhci-ble` introduced GATT timeout/disconnect regressions in
reported host runs. Earlier GATT/notification successes establish that the
underlying path has worked, but do **not** certify that the latest BLE library
build has passed the same acceptance again. Revalidate connect, inspect,
listen, and final Windows restore after that lifecycle regression is fixed.

No separate Dedicated WinUSB dongle has been provisioned or validated. The
readiness probe's negative-path result was one AX201 on `BTHUSB`, zero
Dedicated candidates, and `not_win_usb_bound` for the AX201.

On the recorded host, `UsbDkHelper.dll` and the UsbDk service were missing.
UsbDk enumeration/correlation and redirect were not attempted. UsbDk is not
the current AX201 takeover backend.

The installer, service lifecycle, Control Panel, and GUI self-elevation code
exist, but the most recent packaging/UAC changes have **not** been confirmed
by a new Windows-host acceptance report. Do not infer host validation from
Cargo checks or the presence of source code.
