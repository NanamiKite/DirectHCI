# Troubleshooting and safe recovery

DirectHCI can temporarily change the driver of a Windows Bluetooth USB controller. If something fails, first identify the **observed** controller state. Do not delete `%ProgramData%\DirectHCI\ownership-journal-v1.json`, force another device's INF, or disable Secure Boot or driver-signing checks to make an error disappear. Keep a non-Bluetooth keyboard or mouse available when working on the controller used by your input devices.

For normal installation, see [installation](installation.md). The [failure model](failure-model.md) explains the internal states. [中文版](troubleshooting.zh-CN.md).

## The Control Panel lists no controller

- **Symptom:** The service may show Running, but **Bluetooth Controller** is empty or unavailable.
- **Check:** Refresh the panel and check its **Diagnostics**. Confirm in Device Manager that the radio has a present, healthy **USB Bluetooth controller devnode**; a Bluetooth-labelled device on another bus is not necessarily a target. If the service is stopped, start it first. For advanced diagnostics, `directhci controllers --direct --json` enumerates without acquiring a controller.
- **Safe next step:** If the only observed node is non-USB (for example `BCMBTBUS\BLUETOOTH\...`), this backend cannot prepare that node. If a healthy USB/BTHUSB controller is present but DirectHCI still lists none, save the panel diagnostics and the direct enumeration result for an issue. Do not bind WinUSB to an arbitrary Device Manager entry. See the [eligibility model](compatibility.md#compatibility-model).

## Prepare Controller is rejected by Windows

- **Symptom:** The panel reports a preparation or `PackageStagingRejectedByWindows` error.
- **Check:** Copy the exact panel diagnostic/error and confirm that Device Manager still shows the original Windows Bluetooth driver. Advanced diagnosis can inspect `%WINDIR%\INF\setupapi.dev.log` around the failed attempt; do not publish the full log without reviewing device identifiers and paths.
- **Safe next step:** Preparation does not take over the controller. Leave Secure Boot, TESTSIGNING and BCD unchanged. Do not use `/install` or manually force an INF. If Windows rejects the locally signed package, report the exact Windows error and signing-policy context; the host may not accept this preparation method. A trusted public one-time certificate may remain after a failed stage. See [driver provisioning](internals/driver-provisioning.md).

## Recovery Required

- **Symptom:** The panel or `directhci status` reports `RecoveryRequired`, possibly after a reboot.
- **Check:** Look at the panel's **Recovery** and **Diagnostics** fields and whether Device Manager shows the same physical controller, its active service/driver and problem code. A retained journal is recovery evidence, not proof that WinUSB is still active.
- **Safe next step:** End any active client, then use **Restore Windows** in the running panel. Recovery re-enumerates the journal's controller; if Windows ownership is already healthy, it can reconcile and clear stale intent. If the service is unavailable, use the [offline recovery procedure](installation.md#recovery) from an elevated terminal. If identity is missing or ambiguous, preserve the journal and diagnostic output; do not select a different device with the same VID/PID or keep retrying takeover.

## The service stopped, but Bluetooth did not return

- **Symptom:** SCM shows DirectHCI stopped, but Windows Bluetooth is off, Device Manager shows an error, or no controller is listed.
- **Check:** Service stop alone does **not** prove `WindowsOwned`. Inspect Device Manager and the panel/CLI diagnostics. An unknown USB device with “Device Descriptor Request Failed” may not expose the original PnP identity, so recovery can report `device_missing`.
- **Safe next step:** Do not start another takeover. Run [offline recovery](installation.md#recovery) as administrator while the service is stopped. If it reports `device_missing`, `RecoveryRequired`, or cannot confirm the original controller, keep the journal and collect the exact result and Device Manager problem code. A hardware power cycle or reboot may change what Windows can enumerate, but is not a guaranteed fix; re-check the state afterward. Do not delete the journal or force a driver onto the unknown USB node.

## System Bluetooth is unusable after abnormal termination

**This is a known limitation that is not yet fixed.**

- **Symptom:** Unexpected power loss, a blue screen, replacement of program files while the DirectHCI service is running, or abnormal process termination is followed by Windows Bluetooth staying off and refusing to turn on. Device Manager may still show a healthy controller, and the service may already be stopped.
- **Check:** Preserve panel **Diagnostics**, the service state, the controller's current driver and Device Manager problem code. BTHUSB with problem code 0, or an absent journal, does not by itself prove that the Windows Bluetooth switch or connections work.
- **Safe next step:** Do not attempt another takeover. If DirectHCI is still running, disconnect clients and stop the service normally; do not force-terminate processes or keep replacing files. Windows **Restart** may be needed to restore system Bluetooth; shutdown and power-on with Fast Startup are not equivalent. After restarting, check the Bluetooth switch and actual device usability. If the problem remains, preserve diagnostics and follow [the offline recovery guidance](installation.md#recovery). Restart is not a guaranteed fix; do not delete the journal or force a driver to hide the failure.

## Sharing a diagnostic report

Record the DirectHCI build, Windows build, controller model/USB ID, the action that failed, the exact error, and the final observed driver/ownership state. Use the panel's **Diagnostics → Copy** where possible. Before sharing a CLI/SetupAPI log publicly, review instance IDs, serial numbers, machine/user paths and certificate identifiers. Keep raw logs private when in doubt. The [Windows hardware test plan](windows-test-plan.md) lists useful observations.
