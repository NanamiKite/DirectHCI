# Failure model

DirectHCI changes the Windows driver bound to a Bluetooth controller. Its
safety goal is to preserve enough durable evidence to verify Windows
Bluetooth ownership after any interrupted operation. It cannot guarantee
that a missing, electrically failed or firmware-unresponsive controller
will recover without host intervention.

| Failure or interruption | Implemented response | State that may remain |
| --- | --- | --- |
| Prepare fails before Driver Store staging | Do not acquire or bind WinUSB; re-observe original Windows binding | Windows Bluetooth remains selected; generated attempt files may remain |
| Windows rejects package staging | Report `PackageStagingRejectedByWindows` with Windows error; check unchanged binding; remove this attempt's public trust when no package remains | Windows Bluetooth remains selected; generated evidence is retained; cleanup failure is explicit |
| Package rank is unsafe or unknown | Reject signed source INF before staging; post-stage planner must also be safe | On post-stage failure, remove only this attempt's newly staged unused package, without force; retain package/trust if removal is not confirmed |
| Takeover fails after a PnP side effect | Attempt exact-controller Windows-driver reconciliation; retain journal if restore cannot be verified | `WindowsOwned` if confirmed, otherwise `RecoveryRequired` |
| Client releases or its pipe breaks | Stop Raw HCI, then attempt and verify Windows-driver restore | Service remains running; unresolved restore retains journal |
| Raw HCI/controller I/O fails | End the owner session and enter the same restore path | Failure can still become `RecoveryRequired` |
| Short RX queue burst | Pause adapter pumping without consuming/dropping the next packet; daemon retries outbound writes within a bounded deadline | Event/ACL enqueue order is preserved; command and shutdown handling continue |
| Sustained RX non-consumption | SDK and Raw HCI each enforce 4096 packets / 16 MiB; daemon outbound retry expires after two seconds | Explicit session failure and restoration rather than unlimited memory growth; no guarantee against all overload |
| Service stop or Control Panel exit | Stop accepting new sessions, request owner-session shutdown and wait up to the runtime deadline | A failed or timed-out restore can retain the journal; SCM stop alone is not proof of `WindowsOwned` |
| Normal OS shutdown/restart | SCM sends `PRESHUTDOWN`; configured 90-second SCM budget covers the runtime's 45-second stop deadline | Failure/timeout is reported to SCM and Application event log; journal retained |
| Daemon crash, blue screen or machine power loss | Durable journal survives; installer-registered SYSTEM boot task runs recovery-only even though the main service is manual-start; daemon startup and offline recovery also reconcile | Missing, ambiguous or firmware-unresponsive controller keeps the journal; recovery is not guaranteed |
| PnP reports reboot required | Do not reboot automatically; retain journal and re-check real PnP state on later startup/recovery | Clear only when a later boot and healthy Windows-owned state are confirmed; otherwise `RecoveryRequired` |
| Controller missing or changed identity | Do not substitute another device with the same VID/PID | Journal retained for diagnosis |
| Untrusted ProgramData path | Refuse operations that rely on the journal | No new takeover; fix storage trust before retrying |
| Upgrade or uninstall with unconfirmed recovery | Abort removal of service/binaries and retain journal | Installation retained so recovery remains possible |

A journal is an intent and evidence record, not proof that WinUSB is still
active. Recovery must freshly identify the same physical controller and
observe its present service, driver, problem code and DirectHCI interface.
When the required Windows-owned state is already confirmed, recovery can
clear stale intent without reinstalling an already-active driver. Conversely,
deleting a journal merely hides evidence and is not a recovery procedure.

New journals record the kernel boot identifier and owner process creation
FILETIME. Recovery compares boots **before** inspecting the old PID; within
one boot it compares process creation time, not PID alone. A full **Restart**
is distinct from shutdown/power-on with Fast Startup. Legacy journals lack
these fields: they remain readable, but uncertain owner identity stays blocked.

New takeover journals also capture the original device-instance security
property before changing the driver. Recovery restores that property before
restarting the Windows driver and verifies it before clearing the journal.
An old journal cannot establish an unknown original DACL; a remaining known
DirectHCI-only override requires explicit repair rather than guessed access.
Healthy PnP/driver ownership still does not prove Windows radio/UI availability.

The boot task, foreground diagnostics, controller preparation and live leases
share a protected, share-denied mutation-lock file. A crash releases its kernel
handle, not the journal. The boot task never starts the interactive service or
acquires a controller. Uninstall removes this task only after confirmed recovery
and service deletion. These changes take effect after upgrading/reinstalling
the service registration, not merely rebuilding an executable.

These are implementation paths, not a blanket hardware guarantee. The
dynamic signing/staging path and later installer/Control Panel changes need
Windows-host acceptance by build and device; see
[compatibility](compatibility.md) and the
[Windows hardware test plan](windows-test-plan.md). For journal phase
ordering, driver selection and exact restore checks, see
[ownership and recovery](ownership-and-recovery.md).
