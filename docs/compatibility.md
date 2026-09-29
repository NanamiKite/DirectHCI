# Compatibility

This document distinguishes architectural possibility from tested support.

| Controller | VID/PID | Windows | Ownership backend | Enumeration | Acquire/release | Recovery | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Intel Wi-Fi 6 AX201 Bluetooth function | 8087:0026 | Windows 11 | Temporary device-specific WinUSB rebind | Observed | Executable code; hardware round-trip not yet run | Executable offline recovery; hardware not yet exercised | BTHUSB baseline confirmed; development package build/sign/stage and destructive validation pending |

No controller is currently claimed as takeover-validated. M1 is implemented at
code level, but support remains unvalidated until a real
`BTHUSB -> WinUSB -> BTHUSB` round trip succeeds on the host.

The observed AX201 Windows-owned state uses service `BTHUSB`, Intel INF
`oem69.inf`, provider `Intel Corporation`, and driver version `23.90.0.8`.
These values are an observation from one host, not identity fields or an
ownership recovery recipe.

No dedicated WinUSB controller has been selected, provisioned, or validated
yet. The readiness/open implementation exists and can report candidates,
application-interface registration, descriptors, and pipes, but no hardware
result is claimed until a separate provisioned dongle is tested.

The current Windows host readiness baseline is one controller (the AX201),
Dedicated WinUSB discovery successful, zero candidates, and AX201 status
`not_win_usb_bound`. This validates the negative path only; it is not
Dedicated hardware acceptance.

The current host's read-only UsbDk result is: helper DLL missing, service
missing, enumeration not attempted, and correlation not evaluated. No UsbDk
installation or redirect test has been performed.
