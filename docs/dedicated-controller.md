# Dedicated Controller onboarding

This procedure is a development-only acceptance path for a separate,
expendable USB Bluetooth controller. It does not provision the controller
automatically, and it is not the future DirectHCI product installer.

## Exclusion from Dedicated provisioning: system AX201

Never use this Dedicated/Zadig procedure to provision or permanently rebind
the current system controller. Its separately documented M1 temporary
takeover/recovery flow is different:

~~~text
Intel AX201 Bluetooth
VID:      8087
PID:      0026
Instance: USB\\VID_8087&PID_0026\\5&310905D1&0&14
Service:  BTHUSB
~~~

It is the Windows system Bluetooth controller. Dedicated Controller Mode
requires another physical USB dongle. If the external target cannot be
distinguished from the AX201 or another attached device, stop.

## Before provisioning

Connect the intended external dongle and keep the AX201 Windows-owned. Run the
read-only commands from a normal, non-elevated terminal:

~~~powershell
directhci controllers --json
directhci doctor --json
~~~

Save both complete outputs. For the external dongle, confirm and record:

- DirectHCI ControllerId;
- VID/PID;
- full Device Instance ID;
- genuine USB serial, if available;
- location paths and parent instance;
- current service;
- current INF;
- driver provider and version.

The snapshot proves which physical controller the user intends to modify. It is
not an instruction to restore a cached INF later. Device interface paths are
observations and must not be used after provisioning.

VID/PID alone is insufficient. If two identical dongles are attached, use a
genuine serial or the instance ID plus topology/location. If that still leaves
ambiguity, stop. For a development provisioning session, disconnecting every
other external dongle of the same model is the safest practical choice; this
does not change DirectHCI's runtime identity rules.

## Manual Zadig provisioning

The following driver-changing action is performed by the user, not DirectHCI:

1. Verify again that the selected device is the external dongle and not the
   AX201, an input device, a USB hub, or a composite parent.
2. If Zadig does not show the dongle because it already has a driver, the user
   may enable **Options → List All Devices**.
3. Compare the displayed USB ID with the saved snapshot. When serial/topology
   evidence is weak, leave only the target external dongle connected.
4. Select **WinUSB**, not libusbK, libusb-win32, or a filter-driver operation.
5. Only after the target is unambiguous may the user choose the Zadig
   install/replace action.
6. Record any reboot or unplug/replug request and the exact Zadig result.

Zadig is an external manual development tool. It is not a DirectHCI runtime
dependency and this procedure does not authorize automated libwdi, INF,
SetupAPI, or pnputil driver installation.

## After provisioning

Discard all old interface paths and run fresh read-only observation:

~~~powershell
directhci controllers --json
directhci doctor --json
~~~

Confirm that:

- the AX201 still has its original instance and BTHUSB service;
- the external dongle still matches the saved physical identity evidence;
- the external dongle now reports service WinUSB;
- a provisioned application interface GUID is present and maps uniquely to the
  same PnP instance;
- CreateFile with FILE_FLAG_OVERLAPPED and WinUsb_Initialize succeed;
- the default interface is E0/01/01;
- exactly one interrupt IN, one bulk IN, and one bulk OUT pipe are usable;
- the final readiness status is Ready.

GUID_DEVINTERFACE_USB_DEVICE is never a fallback application interface. If the
service is WinUSB but no DeviceInterfaceGUID or DeviceInterfaceGUIDs
registration is present, NoApplicationInterface is the correct result and the
provisioning configuration must be investigated.

## Stop conditions and evidence

Stop without changing DirectHCI code if any of these occurs:

- the physical identity cannot be correlated uniquely;
- more than one application interface path remains plausible;
- open or WinUSB initialization fails;
- the default interface is not E0/01/01;
- a required pipe is missing or duplicated.

Save both JSON outputs and record all registered application interfaces, the
complete default interface descriptor, every reported pipe descriptor, and
whether Windows exposes a composite device. An unexpected interface may require
an associated interface, alternate setting, or different provisioning choice,
but that conclusion must come from the real device evidence rather than a
relaxed readiness rule.

## Restoring or retiring the test dongle

Restoration is a user-controlled Device Manager or vendor-driver operation.
First record the current observation, then select the original/native driver
for that external dongle or remove the dedicated WinUSB package as appropriate
for that device and reconnect it. Re-observe the actual result afterward.

Do not mechanically replay the pre-provisioning INF snapshot, and never apply
this procedure to the AX201. Windows driver selection or Windows Update may
have changed since the snapshot was taken.

## Dedicated hardware acceptance

Dedicated-mode Raw HCI still requires a real external controller with
`Ready` descriptor topology. This is **not** a gate on M2 generally:
Raw HCI and BLE operation have already been exercised through the AX201
temporary-takeover mode. Dedicated hardware must pass its own identity,
interface, pipe, and lifecycle acceptance before its results are claimed.
