# Failure model

DirectHCI changes the Windows driver bound to a Bluetooth controller. Its
safety goal is to preserve enough durable evidence to verify Windows
Bluetooth ownership after any interrupted operation. It cannot guarantee
that a missing, electrically failed or firmware-unresponsive controller
will recover without host intervention.

| Failure or interruption | Implemented response | State that may remain |
| --- | --- | --- |
| Prepare fails before Driver Store staging | Do not acquire or bind WinUSB; re-observe original Windows binding | Windows Bluetooth remains selected; generated attempt files may remain |
| Windows rejects package staging | Report `PackageStagingRejectedByWindows` with Windows error; check unchanged binding | Windows Bluetooth remains selected; a trusted *public* one-time certificate may remain |
| Package stages but planner is unsafe | Refuse takeover; report the actual blocker (for example rank, candidate or journal) | Windows Bluetooth stays selected; staged package remains |
| Takeover fails after a PnP side effect | Attempt exact-controller Windows-driver reconciliation; retain journal if restore cannot be verified | `WindowsOwned` if confirmed, otherwise `RecoveryRequired` |
| Client releases or its pipe breaks | Stop Raw HCI, then attempt and verify Windows-driver restore | Service remains running; unresolved restore retains journal |
| Raw HCI/controller I/O fails | End the owner session and enter the same restore path | Failure can still become `RecoveryRequired` |
| Service stop or Control Panel exit | Stop accepting new sessions, request owner-session shutdown and wait up to the runtime deadline | A failed or timed-out restore can retain the journal; SCM stop alone is not proof of `WindowsOwned` |
| Daemon crash or machine power loss | Durable journal survives; startup/offline recovery re-enumerates the exact controller | Missing or ambiguous controller keeps the journal |
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

These are implementation paths, not a blanket hardware guarantee. The
dynamic signing/staging path and later installer/Control Panel changes need
Windows-host acceptance by build and device; see
[compatibility](compatibility.md) and the
[Windows hardware test plan](windows-test-plan.md). For journal phase
ordering, driver selection and exact restore checks, see
[ownership and recovery](ownership-and-recovery.md).
