# AX201 development WinUSB package

This package is development-only and matches exactly
`USB\VID_8087&PID_0026`. It uses the Windows in-box `WinUSB.sys`; there is no
DirectHCI `.sys`, filter, co-installer, or firmware payload.

The INF now applies a device-specific DACL allowing only SYSTEM and elevated
Administrators to open the WinUSB function. Changing the INF invalidates the
previous catalog signature: rebuild, sign, and stage the revised package
manually before expecting this ACL on the Windows host. The effective device
and application-interface ACL still needs read-only verification on Windows.

Build and sign it from an elevated PowerShell on the Windows development host:

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\scripts\windows\build-dev-driver.ps1 -CreateCertificate
```

`-CreateCertificate` is explicit: it creates a self-signed certificate in the
current user's personal store and exports its public certificate beside the
built package. By default the package output goes to the host-local
`$env:LOCALAPPDATA\DirectHCI\driver\winusb-ax201-dev`, not the VMware shared
source tree. `-OutputDirectory` can select another host-local path. The script
does not trust the certificate, change Secure Boot, enable test-signing, stage
the driver, or bind it to a device. A pre-existing certificate can instead be
selected with `-CertificateThumbprint`.

For the self-signed development route, the first run produces the signed CAT
and exported CER but signature-chain verification can fail until the user
explicitly trusts that CER. From an elevated PowerShell, after reviewing the
certificate and only on the test host, the manual trust steps are:

```powershell
$cert = "$env:LOCALAPPDATA\DirectHCI\driver\winusb-ax201-dev\directhci-development-test.cer"
certutil -addstore Root $cert
certutil -addstore TrustedPublisher $cert
```

Then rerun the build script with the printed thumbprint. Microsoft documents
that a self-signed test driver package normally also requires Windows test
mode; enabling it is an explicit user operation followed by a reboot, and
Secure Boot can prevent that policy change. The script only reports current
Secure Boot/test-signing state. It never executes `bcdedit` with a modifying
option. A suitably trusted enterprise or production signing route can avoid
the self-signed test-mode workflow.

After the user has manually configured the required certificate/signing policy,
stage only the INF from an elevated terminal:

```powershell
pnputil /add-driver "$env:LOCALAPPDATA\DirectHCI\driver\winusb-ax201-dev\directhci-ax201-dev.inf"
```

Do not add `/install`. Confirm the AX201 remains on `BTHUSB`, then run the
DirectHCI takeover planner before authorizing a round trip.
