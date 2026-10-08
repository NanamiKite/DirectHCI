# Hardware compatibility

Development testing has used one Intel AX201 Bluetooth USB function
(`8087:0026`) on Windows 11. Other controller, firmware and Windows
combinations have not been validated.

## Recorded AX201 results

| Area | Recorded observation |
| --- | --- |
| Driver takeover and restore | `BTHUSB/oem69.inf` → `WinUSB/oem183.inf` → `BTHUSB/oem69.inf` |
| WinUSB readiness | Interface E0/01/01; event interrupt IN `0x81`, ACL bulk IN `0x82`, ACL bulk OUT `0x02` |
| HCI | Reset status `0x00`; version `0x0b`, revision `0x375b`, manufacturer `0x0002` |
| BLE | LE connection, ATT MTU exchange (131), GATT discovery and passive notifications |
| Session teardown | Clean disconnect and final `WindowsOwned` state |

The INF names and endpoint addresses are measurements from that host. The
runtime selects drivers from fresh observations and discovers endpoints from
descriptors.

These records predate later BLE and packaging changes. Exact build revisions
and complete host logs are not included here; they should accompany future
test reports using [the test plan](windows-test-plan.md).

## Known issues and pending tests

- Host reports after the BLE CLI was extracted into `directhci-ble` showed
  GATT timeouts and disconnect regressions. Connect, inspect, passive listen
  and final Windows restoration need another acceptance run.
- Recent installer, service lifecycle, Control Panel and UAC changes have no
  new Windows host acceptance report.
- Dynamic WinUSB package preparation needs host verification of local
  signing acceptance, driver rank, takeover and restore.
- A separate dedicated WinUSB dongle has not been tested.

## Other backend observations

With the AX201 still bound to `BTHUSB`, the dedicated WinUSB probe found zero
candidates and reported `not_win_usb_bound` for that controller.

The recorded host had neither `UsbDkHelper.dll` nor the UsbDk service, so
UsbDk enumeration, correlation and redirect were not tested. Temporary WinUSB
rebind is the implemented takeover backend. The earlier comparison is in
[ownership-survey.md](ownership-survey.md).
