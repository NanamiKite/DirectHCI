# directhci-ble

Generic asynchronous BLE Central and GATT client API backed by the local
DirectHCI runtime.

This first-party crate currently uses `GPL-3.0-only`; see
[LICENSE.txt](../../LICENSE.txt). Its upstream dependencies retain their own
licenses.

The library acquires Raw HCI through `directhci-client`, adapts it through
`directhci-bt-hci`, and owns the TrouBLE Host lifecycle. It does not access
WinUSB or switch Windows drivers itself.

```rust,no_run
use std::time::Duration;
use directhci_ble::{
    BleAddress, BleCentralConfig, DirectHciBleCentral, WriteMode,
};

# async fn example() -> Result<(), directhci_ble::BleError> {
let central = DirectHciBleCentral::connect(BleCentralConfig::default()).await?;
let devices = central.scan(Duration::from_secs(5)).await?;
let peer: BleAddress = devices[0].address;
let connection = central.connect_device(peer).await?;

let services = connection.discover().await?;
let characteristic = services[0].characteristics[0].clone();

// Passive listening does not write a CCCD. The listener is ready when this
// future returns, so writes performed afterward cannot race listener setup.
let mut notifications = connection.listen(&characteristic).await?;
connection
    .write(&characteristic, vec![0x01, 0x02], WriteMode::WithResponse)
    .await?;
if let Some(event) = notifications.recv().await? {
    println!("handle={:#06x} value={:02x?}", event.handle, event.value);
}

notifications.close().await?;
connection.disconnect().await?;
# Ok(())
# }
```

Use `subscribe` for standard CCCD-based notification/indication enablement,
`listen` for passive reception from one characteristic, and `listen_all`
for passive reception from every handle. These operations never fall back to
one another.
