# Rust client SDK

The DirectHCI libraries are currently unpublished. Use path dependencies
from a local checkout. A consumer talks to `directhcid` through
`directhci-client`; it does not open WinUSB or change a Bluetooth driver.

```text
Application
    ├─ Raw HCI: directhci-client → local IPC → directhcid
    └─ BLE: directhci-ble → TrouBLE → directhci-bt-hci
             → directhci-client → local IPC → directhcid
```

## Raw HCI

`DirectHciClient::connect(name, version)` connects to an already running
service. `connect_or_start(name, version)` can request startup when the caller
has Windows `SERVICE_START` rights; it does not grant Raw HCI permission.

Read-only methods include `runtime_status()`, `list_controllers()`,
`controller_status(id)` and `preferences()`. Privileged methods include
`set_preferred_controller(id)`, `prepare_controller(id, trust_acknowledged)`,
`restore_windows()` and `acquire_raw_hci(id)`. The preparation acknowledgement
must come from an explicit user decision about local certificate trust.

`acquire_raw_hci(id)` returns a `RawHciClientSession` after the runtime has
finished guarded takeover and opened Raw HCI. The session exposes
`send_command(opcode, params)`, `send_acl(packet)`,
`receive_packet(timeout)`, typed `receive_event(timeout)` /
`receive_acl(timeout)` convenience methods and `release()`.
Command completion/status correlation is performed by the runtime; unsolicited
events and ACL packets share a bounded FIFO. `receive_packet()` returns
`HciIncomingPacket::Event` or `::Acl` in arrival order; do not mix typed reads
when ordering across packet kinds matters. The bt-hci adapter uses this unified
path and pauses before dequeueing when its own queue is full. Explicitly call
`release()` and handle its restore result. Pipe disconnect is a safety path,
not a substitute for awaited, confirmed release.

Normal acquisition requires an administrator under the current Named Pipe
policy and one active writer session is allowed globally. The runtime owns
WinUSB handles, driver changes and the recovery journal. See
[architecture](architecture.md) and [failure model](failure-model.md).

## BLE Central and GATT

`directhci-bt-hci` adapts the Raw HCI client to the `bt-hci` traits.
`directhci-ble` owns the TrouBLE host and exposes scan, connection,
discovery, read/write, standard CCCD subscription, passive listening and
disconnect APIs. The GATT library remains generic and does not encode a
particular peripheral protocol. Its API, Cargo path dependency example and
cleanup rules are in the [BLE library README](../crates/directhci-ble/README.md).

The BLE library's current default chooses a controller only if exactly one
is present; with multiple controllers supply
`BleCentralConfig::controller_id` explicitly. It does not currently read
the Control Panel's saved preference. Avoid implying that a GUI preference
automatically selects a controller for every external Rust consumer.

Hardware acceptance for later library builds is tracked in
[compatibility](compatibility.md).
