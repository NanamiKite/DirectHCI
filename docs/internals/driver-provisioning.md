# Device-specific WinUSB driver provisioning

This document describes the privileged implementation. For ordinary use,
see [installation](../installation.md#prepare-a-controller). Preparation is
separate from controller acquisition and never invokes the takeover install
primitive.

## Eligibility and identity

The runtime freshly enumerates a present, problem-free USB Bluetooth
controller. Its PnP instance must begin with `USB\`, its active service must
be `BTHUSB` or `IBTUSB`, and Bluetooth class evidence must be present. It
chooses an unambiguous, exact `USB\VID_xxxx&PID_yyyy` or composite
`USB\VID_xxxx&PID_yyyy&MI_zz` Hardware ID observed on that controller. A
device name, guessed model, class-wide compatible ID or arbitrary supplied
USB instance ID is not sufficient.

The selected controller's identity and Windows binding are checked again
before and after package generation and staging. The runtime refuses
preparation while an ownership journal is unresolved.

## Package generation and signing

For an unprepared exact Hardware ID, the runtime creates a protected
subdirectory below `%ProgramData%\DirectHCI\drivers\generated`. Its package
key derives from the normalized Hardware ID, while each new attempt receives
a unique subdirectory. The single-model INF template is in
[driver/winusb-device-specific](../../driver/winusb-device-specific/).
It uses Microsoft's in-box `WinUSB.sys` and the DirectHCI application
interface GUID; it is not a custom Bluetooth kernel driver.

The installed `libwdi.dll` prepares the INF/CAT and signs the catalog with
a one-time local certificate after explicit Control Panel consent. The public
certificate is added to the required local trust stores; the private key is
destroyed after signing. `metadata.json` records the preparation attempt
before trust is changed. The runtime loads the DLL only from its protected
service executable directory.

A previously applicable DirectHCI package is reused when the fresh takeover
planner reports it Ready; the runtime does not unconditionally create and
sign a new package for each connection.

## Driver Store staging is not takeover

After confirming nonempty INF and CAT files, the runtime inspects the signed
source INF against the actual controller with `DI_ENUMSINGLEINF`. Its rank
must be known and, even with the best possible signature score after catalog
registration, strictly worse than the freshly selected Windows recovery
candidate, so preparation cannot intentionally introduce a default-winning
WinUSB candidate. Unknown ranks and ambiguous candidates are refused before
staging. The signature/feature/identifier fields follow
[Windows driver ranking](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/how-windows-ranks-driver-packages).
The runtime then calls `SetupCopyOEMInfW` with
`SP_COPY_NOOVERWRITE` to stage the package. It does **not** call libwdi's driver
installation function, use `pnputil /install` or bind WinUSB to the device.
It then checks that the same physical controller still has its original
Windows Bluetooth binding and that a fresh planner sees a unique applicable
DirectHCI candidate with `takeover_safe` set. A failure after staging attempts
to remove only this attempt's newly published INF, without force; an existing
package is never removed as rollback. Only a later client acquisition can pass through the
separate journal, driver-rank, recovery-candidate and exact-devnode
`DiInstallDevice` gates.

Windows can reject a locally signed catalog under its current code-integrity
policy even if catalog generation succeeds. DirectHCI reports
`PackageStagingRejectedByWindows` with the Windows error; it does not change
Secure Boot, TESTSIGNING or BCD, and must not proceed to takeover after a
rejected stage. Dynamic preparation is therefore an attempt, not a promise
that every USB Bluetooth controller or Windows installation will accept it.

The old multi-Hardware-ID development package is migration/diagnostic
material only. The current installer bundles the provisioning DLL, not a
pre-generated driver package, and does not stage a package during Setup.

## State and cleanup limits

Generated INF, CAT and metadata files are kept under the protected
ProgramData tree. The M1 ownership journal is separate: release restores
the Windows driver but does not delete the staged package. The current
uninstaller leaves staged packages, public certificates and ProgramData
state in place. Failed preparation removes only its own UUID-named public
certificate after confirming that no newly staged package is left. If Windows
refuses safe package removal, both the package and its public trust are retained
and the cleanup error is reported separately. Cleanup of arbitrary historical
or reused package references is not part of this per-attempt rollback. Do not
describe uninstall as removing these objects or remove a public certificate
while a staged package may still refer to it.

The release build pins libwdi 1.5.1 and builds the DLL on the development
host. End-user machines do not need EWDK or WDK tools. Binary redistribution
must account for libwdi's source and license obligations; see
[references](../references.md#distribution-files).
