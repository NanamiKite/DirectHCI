# DirectHCI

DirectHCI is a Windows userspace Bluetooth Controller ownership and Raw HCI
runtime. The current development phase is limited to controller discovery,
identity, observation, ownership, release, recovery, and diagnostics.

DirectHCI is not a Bluetooth Host Stack and does not contain consumer-specific
Bluetooth protocols or application logic.

## Current status

The implemented M1 code includes read-only Windows controller discovery,
diagnostics, a temporary-rebind planner, an explicitly destructive AX201
WinUSB round trip, durable ownership journaling, and offline Windows-driver
recovery. No HCI command or data transfer is implemented.

DirectHCI formally supports two product modes:

- Dedicated Controller Mode keeps a separate controller intentionally bound to
  WinUSB; `DirectHciReady` is its desired available state.
- Takeover Mode transfers a Windows-owned controller to DirectHCI and later
  reconciles it to `WindowsOwned`.

Temporary, device-specific WinUSB rebind of the current AX201 is the Phase 1
mainline. Dedicated WinUSB remains a future auxiliary development mode. UsbDk
is retained as a read-only probe and experimental takeover candidate, not the
recommended default backend. The evidence and route gates are documented in
[`docs/ownership-survey.md`](docs/ownership-survey.md), and the rebind/recovery
contract is in [`docs/temporary-rebind.md`](docs/temporary-rebind.md).

The user-operated AX201 package, staging, recovery, and hardware acceptance
checklist is in [`docs/temporary-rebind.md`](docs/temporary-rebind.md) and
[`docs/windows-test-plan.md`](docs/windows-test-plan.md).

```text
directhci controllers
directhci controllers --json
directhci controller show <id>
directhci controller show <id> --json
directhci doctor
directhci doctor --json
directhci takeover plan <id>
directhci takeover plan <id> --json
directhci takeover roundtrip <id> --execute [--json]
directhci recover --offline [--json]
```

`takeover plan` freshly resolves the selected controller, privilege state,
journal state, and applicable SetupAPI driver candidates/ranks. It never
changes PnP state. `takeover roundtrip` is rejected unless `--execute` is
present and all gates pass. The development-only AX201 package and signing
instructions are under `driver/winusb-ax201-dev/`.

`doctor` includes a strictly read-only UsbDk compatibility probe. It dynamically
loads the system `UsbDkHelper.dll`, validates the minimum enumeration ABI,
lists UsbDk observations, and conservatively correlates them with freshly
enumerated Windows controllers. Redirect exports are never invoked. On the
current validation host the helper DLL and service are absent; DirectHCI does
not install them automatically.

doctor also contains a read-only Dedicated WinUSB readiness probe. It requires
the controller's current service to be WinUSB, reads the provisioned
DeviceInterfaceGUID / DeviceInterfaceGUIDs registration, correlates the
registered interface back to the same PnP instance, then opens it with
overlapped I/O only long enough to query the default interface and pipe
descriptors. Generic USB observation paths are never treated as WinUSB
application interfaces, and no USB transfer is issued.

On non-Windows systems the workspace and portable core can be built, while
Windows device enumeration reports that the platform is unsupported. See
[`docs/development.md`](docs/development.md) for the VM/host workflow.

## Safety boundary

- A controller has exactly one writer: Windows or DirectHCI.
- Hardware-persistent writes are prohibited by default.
- Recovery observes current Windows state and reconciles it; a journal is not
  treated as the source of truth.
- Device interface paths and current devnodes are observations, not permanent
  controller identifiers.

The project license has not been selected yet. Third-party source must not be
copied into this repository until its license and intended use have been
recorded in [`docs/references.md`](docs/references.md).
