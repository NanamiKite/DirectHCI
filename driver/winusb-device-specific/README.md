# Device-specific WinUSB provisioning

This INF template is used by the privileged runtime to prepare exactly one
freshly observed USB Bluetooth PnP Hardware ID. The installer bundles the
libwdi provisioning component, not a pre-generated driver package. An
already-staged legacy multi-Hardware-ID package remains migration-only.

The template is intentionally limited to one `USB\VID_xxxx&PID_yyyy` (or
`&MI_zz`) model. It uses the Windows in-box WinUSB driver and preserves the
DirectHCI application interface GUID and its security descriptor. The
read-only planner derives the target ID from a freshly enumerated, present
USB Bluetooth controller. The Control Panel's **Prepare Controller** command
asks the privileged runtime to generate this INF, let libwdi generate and
self-sign its catalog with a one-time certificate, stage with SetupAPI, then
freshly verify the candidate and unchanged BTHUSB binding. The user must
explicitly approve local Root/TrustedPublisher trust. The private key is
destroyed after signing. Public certificate cleanup after a failed stage is
not yet reference-aware, so that certificate may remain trusted.

Windows 11 with Secure Boot may reject a locally self-signed package despite
successful catalog signing. This is a runtime `PackageStagingRejectedByWindows`
outcome, not permission to change Secure Boot, TESTSIGNING, or BCD. A Windows
host still needs to verify staging, candidate rank, guarded takeover, WinUSB
topology/HCI readiness, and confirmed Windows restore.
