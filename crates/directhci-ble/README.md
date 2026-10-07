# directhci-ble

Async BLE Central and GATT client library for the local DirectHCI service.
It uses `directhci-client` for the HCI session, `directhci-bt-hci` as the
transport adapter, and TrouBLE for the Bluetooth host stack.

## Setup

The crate is unpublished. Add it from a local DirectHCI checkout:

```toml
[dependencies]
directhci-ble = { path = "../DirectHCI/crates/directhci-ble" }
tokio = { version = "1", features = ["rt", "macros", "time"] }
```

Adjust the path for your project. On Windows, start the DirectHCI service,
select and prepare a controller in the Control Panel, then run the consumer
with administrator rights. A BLE session temporarily takes that controller
away from Windows Bluetooth. See [installation](../../docs/installation.md).

GATT timeout and disconnect regressions have been reported since the library
was extracted from the CLI. Hardware retesting is pending; see
[compatibility](../../docs/compatibility.md).

## Scan

```rust
use std::time::Duration;
use directhci_ble::{BleCentralConfig, BleError, DirectHciBleCentral};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), BleError> {
    let central = DirectHciBleCentral::connect(BleCentralConfig::default()).await?;
    let scan = central.scan(Duration::from_secs(5)).await;
    central.shutdown().await?;

    for device in scan? {
        println!("{} {:?}", device.address, device.local_name);
    }
    Ok(())
}
```

The default configuration selects a controller only when exactly one is
present. With multiple controllers, set `BleCentralConfig::controller_id`
explicitly; this library does not read the panel's saved preference.

Each scan retains at most 512 addresses, 64 service UUIDs per address and 16
distinct payloads each for advertising and scan responses. Old payload variants
are evicted as new ones arrive. Once the address limit is reached, known
addresses continue updating and new addresses are ignored until the next scan.
These bounds prevent long scans from accumulating unlimited broadcast history.

## Connections and GATT

`central.connect_device(address).await` consumes the central and returns a
`BleConnection`. Select a peer from scan results or parse an address such as
`public:AA:BB:CC:DD:EE:FF` or `random:AA:BB:CC:DD:EE:FF`.

| Method | Operation |
| --- | --- |
| `discover` | List services, characteristics and descriptors |
| `find_characteristic` | Find a characteristic by UUID |
| `read` / `write` | Exchange raw characteristic values |
| `subscribe` | Enable notifications or indications by writing the CCCD |
| `listen` | Receive values from one characteristic without writing its CCCD |
| `listen_all` | Receive unsolicited values from all handles |
| `disconnect` | Close the BLE connection and release the HCI session |

Choose UUIDs and write payloads for the target device. Passive listening is
useful for peripherals that send unsolicited values; it does not enable
notifications on a device that requires a CCCD write. `subscribe` and `listen`
return errors from their own operations without falling back to each other.

A listener is ready when `listen(...).await` returns. Keep its stream open
while writing through the same connection, then receive with `recv().await`.
Call `stream.close().await` when finished.

Use `connection.disconnect().await` after a connection, or
`central.shutdown().await` after scanning. Dropping an object requests
best-effort cleanup but does not wait for it to finish.

Shutdown completion is sent after the worker's owned GATT/host, IPC client and
runtime resources are dropped. Native thread joining happens off the caller's
async thread. If a UI needs a shorter wait, retain a completion handle before
consuming the connection:

```rust
let mut completion = connection.shutdown_completion();
match tokio::time::timeout(Duration::from_secs(2), connection.disconnect()).await {
    Ok(result) => result?,
    Err(_) => {
        // Show "Disconnecting", not "Disconnected". The UI can keep this
        // handle in its background task and observe the eventual result.
        completion.wait().await?;
    }
}
```

The central has the same `shutdown_completion()` method. `completion.result()`
returns `None` until local cleanup is done, or the final `Ok`/`Err` afterwards;
waiting again after a timeout is safe. A failed release leaves Windows restore
unconfirmed: the daemon may still be recovering. Check runtime/Control Panel
status rather than treating an SDK wait timeout as successful disconnection.
See [SDK request budgets](../../docs/sdk.md#raw-hci).

## CLI

The `directhci-ble-cli` package builds `directhci-ble.exe`. With the service
running and exactly one controller present and prepared, scan from an elevated
PowerShell:

```powershell
& "$env:ProgramFiles\DirectHCI\directhci-ble.exe" scan --seconds 5
```

Commands include `connect`, `inspect`, `read`, `write`, `subscribe`, `listen`,
`listen-all` and `exchange`. Run the executable without arguments to print
usage. An explicit controller is supplied before the command:
`directhci-ble --controller <id> scan --seconds 5`.

## License

[GPL-3.0-only](../../LICENSE.txt). Upstream dependencies retain their own
licenses; see [references](../../docs/references.md).
