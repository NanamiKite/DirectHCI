# DirectHCI legacy development WinUSB package

This multi-Hardware-ID package is retained for development diagnostics and
migration compatibility only. The installer no longer bundles or stages it;
new controllers use the Control Panel's dynamic device-specific Prepare flow.

This directory is the single source for the legacy development package:

- `supported-hardware-ids.txt` lists explicit USB PnP Hardware IDs.
- `directhci-winusb-dev.inf.in` contains the shared WinUSB installation
  sections, DirectHCI application interface GUID and security descriptor.
- `build-dev-driver.ps1` generates **one** INF and CAT in
  `%LOCALAPPDATA%\DirectHCI\driver\winusb-supported-devices`.

The list includes the DirectHCI-verified AX201 `8087:0026`, the observed
`8087:0029`, and selected explicit IDs from Linux `btusb`. The latter
are **not** DirectHCI hardware-verified. Listing an ID only permits a
guarded takeover attempt. The planner must still find a safe Windows restore
candidate and lower-priority DirectHCI driver candidate; after switching,
the WinUSB interface/pipe probe must pass. Hardware-specific firmware or
composite interface behavior may still make a listed device unusable.

The previous USB Bluetooth class-compatible-ID INF failed Inf2Cat B2.6.4.9
on the Windows build host. It is not a supported package route. A copy
already staged on a host is **not** removed by this source change; review
Driver Store candidates separately before a new hardware test. There is no
custom DirectHCI kernel driver; this INF binds the in-box `WinUSB.sys`.

On a Windows development host with WDK/SDK signing tools:

```powershell
.\scripts\windows\build-dev-driver.ps1 -CreateCertificate
```

The script generates the INF, runs InfVerif and Inf2Cat, signs the catalog,
and verifies the signature. It does **not** trust the certificate, change
test-signing/Secure Boot settings, stage the package or rebind a device.
Development certificate trust and signing policy are manual host decisions.

When the host accepts the signed package, staging is an explicit separate
step, without `/install`:

```powershell
pnputil /add-driver "$env:LOCALAPPDATA\DirectHCI\driver\winusb-supported-devices\directhci-winusb-dev.inf"
```

Staging is not takeover of an already-bound device. It makes the package a
candidate for listed IDs, including future arrivals; review that system-wide
selection risk. Re-run `directhci takeover plan <controller-id>` and verify
the original Windows Bluetooth service remains bound before any explicit
takeover. This legacy package is not bundled by the current installer.
New controllers use the Control Panel's exact-HWID Prepare action instead;
this manual command remains for development diagnostics only.
