use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use directhci_bt_hci::DirectHciController;
use directhci_client::DirectHciClient;
use embassy_time::Duration;
use trouble_host::advertise::{AdStructure, AdvertisementDataError};
use trouble_host::prelude::*;

const CONNECTIONS: usize = 1;
const L2CAP_CHANNELS: usize = 3;
const MAX_SERVICES: usize = 64;
const MAX_CHARACTERISTICS: usize = 128;
const MAX_DESCRIPTORS: usize = 64;
const VALUE_BUFFER: usize = 4096;

type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
enum Command {
    Scan {
        seconds: u64,
    },
    Connect {
        peer: Address,
        seconds: u64,
    },
    Inspect {
        peer: Address,
    },
    Read {
        peer: Address,
        uuid: Uuid,
    },
    Write {
        peer: Address,
        uuid: Uuid,
        value: Vec<u8>,
        without_response: bool,
    },
    Subscribe {
        peer: Address,
        uuid: Uuid,
        indications: bool,
        seconds: u64,
    },
}

#[derive(Clone, Debug)]
struct ScanRecord {
    address: Address,
    rssi: i8,
    name: Option<String>,
    raw: Vec<Vec<u8>>,
    service_uuids: Vec<String>,
}

struct ScanCollector {
    records: Mutex<BTreeMap<String, ScanRecord>>,
}
impl ScanCollector {
    fn new() -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
        }
    }
}
impl EventHandler for ScanCollector {
    fn on_adv_reports(&self, reports: LeAdvReportsIter<'_>) {
        let mut records = match self.records.lock() {
            Ok(v) => v,
            Err(_) => return,
        };
        for report in reports.flatten() {
            let address = Address::new(report.addr_kind, report.addr);
            let key = format_address(address);
            let (name, service_uuids) = parse_advertisement(report.data);
            let record = records.entry(key).or_insert_with(|| ScanRecord {
                address,
                rssi: report.rssi,
                name: None,
                raw: Vec::new(),
                service_uuids: Vec::new(),
            });
            record.rssi = report.rssi;
            if name.is_some() {
                record.name = name;
            }
            if !record.raw.iter().any(|value| value == report.data) {
                record.raw.push(report.data.to_vec());
            }
            for uuid in service_uuids {
                if !record.service_uuids.contains(&uuid) {
                    record.service_uuids.push(uuid);
                }
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("directhci-ble: {error}");
        std::process::exit(1);
    }
}

async fn run() -> AppResult<()> {
    let (controller_arg, command) = parse_args()?;
    let client = DirectHciClient::connect("directhci-ble", Some(env!("CARGO_PKG_VERSION").into()))?;
    let controllers = client.list_controllers()?;
    let controller_id = match controller_arg {
        Some(id) => id,
        None if controllers.len() == 1 => controllers[0].id.as_str().to_owned(),
        None => {
            return Err(format!(
                "--controller is required when {} controllers are present",
                controllers.len()
            )
            .into());
        }
    };
    let session = client.acquire_raw_hci(&controller_id)?;
    let controller = DirectHciController::new(session)?;
    let shutdown = controller.clone();
    let collector = ScanCollector::new();
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS, L2CAP_CHANNELS> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(local_address())
        .build();
    let mut runner = stack.runner();
    let mut central = stack.central();

    let operation = execute(&stack, &mut central, &collector, command);
    let outcome = tokio::select! {
        result = runner.run_with_handler(&collector) => Err(format!("TrouBLE host runner stopped: {result:?}").into()),
        result = operation => result,
    };
    drop(runner);
    drop(central);
    let release = shutdown.shutdown().await;
    match (outcome, release) {
        (Err(primary), Err(recovery)) => {
            Err(format!("{primary}; DirectHCI release also failed: {recovery}").into())
        }
        (Err(primary), _) => Err(primary),
        (Ok(()), Err(error)) => Err(error.into()),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn execute<'stack, 'borrow>(
    stack: &'borrow Stack<'stack, DirectHciController, DefaultPacketPool>,
    central: &mut Central<'borrow, DirectHciController, DefaultPacketPool>,
    collector: &ScanCollector,
    command: Command,
) -> AppResult<()> {
    match command {
        Command::Scan { seconds } => {
            let mut scanner = Scanner::new(central);
            let config = ScanConfig {
                active: true,
                phys: PhySet::M1,
                interval: Duration::from_millis(100),
                window: Duration::from_millis(100),
                ..Default::default()
            };
            let session = scanner.scan(&config).await.map_err(debug_error)?;
            tokio::time::sleep(StdDuration::from_secs(seconds)).await;
            session.stop().await;
            for record in collector
                .records
                .lock()
                .map_err(|_| "scan record lock poisoned")?
                .values()
            {
                println!(
                    "{}  rssi={}  name={}  services=[{}]  adv={}",
                    format_address(record.address),
                    record.rssi,
                    record.name.as_deref().unwrap_or("-"),
                    record.service_uuids.join(","),
                    record
                        .raw
                        .iter()
                        .map(|v| hex(v))
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            Ok(())
        }
        command => execute_connected(stack, central, command).await,
    }
}

async fn execute_connected<'stack, 'borrow>(
    stack: &'borrow Stack<'stack, DirectHciController, DefaultPacketPool>,
    central: &mut Central<'borrow, DirectHciController, DefaultPacketPool>,
    command: Command,
) -> AppResult<()> {
    let peer = match command {
        Command::Connect { peer, .. }
        | Command::Inspect { peer }
        | Command::Read { peer, .. }
        | Command::Write { peer, .. }
        | Command::Subscribe { peer, .. } => peer,
        Command::Scan { .. } => unreachable!(),
    };
    let filter = [peer];
    let config = ConnectConfig {
        connect_params: Default::default(),
        scan_config: ScanConfig {
            filter_accept_list: &filter,
            active: true,
            phys: PhySet::M1,
            ..Default::default()
        },
    };
    let connection = central.connect(&config).await.map_err(debug_error)?;
    let result = match command {
        Command::Connect { seconds, .. } => {
            println!("connected {}", format_address(peer));
            tokio::time::sleep(StdDuration::from_secs(seconds)).await;
            Ok(())
        }
        command => {
            match GattClient::<DirectHciController, DefaultPacketPool, MAX_SERVICES>::new(
                stack,
                &connection,
            )
            .await
            {
                Ok(client) => {
                    tokio::select! { result = client.task() => Err(format!("GATT task stopped: {result:?}").into()), result = gatt_operation(&client, command) => result }
                }
                Err(error) => Err(debug_error(error)),
            }
        }
    };
    connection.disconnect();
    tokio::time::sleep(StdDuration::from_millis(100)).await;
    result
}

async fn gatt_operation(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    command: Command,
) -> AppResult<()> {
    match command {
        Command::Inspect { .. } => inspect(client).await,
        Command::Read { uuid, .. } => {
            let characteristic = find_characteristic(client, uuid).await?;
            let mut value = [0u8; VALUE_BUFFER];
            let len = client
                .read_characteristic(&characteristic, &mut value)
                .await
                .map_err(debug_error)?;
            println!("{}", hex(&value[..len]));
            Ok(())
        }
        Command::Write {
            uuid,
            value,
            without_response,
            ..
        } => {
            let characteristic = find_characteristic(client, uuid).await?;
            if without_response {
                client
                    .write_characteristic_without_response(&characteristic, &value)
                    .await
                    .map_err(debug_error)?;
            } else {
                client
                    .write_characteristic(&characteristic, &value)
                    .await
                    .map_err(debug_error)?;
            }
            println!(
                "wrote {} bytes ({})",
                value.len(),
                if without_response {
                    "without response"
                } else {
                    "request"
                }
            );
            Ok(())
        }
        Command::Subscribe {
            uuid,
            indications,
            seconds,
            ..
        } => {
            let characteristic = find_characteristic(client, uuid).await?;
            let mut listener = client
                .subscribe(&characteristic, indications)
                .await
                .map_err(debug_error)?;
            let deadline = tokio::time::Instant::now() + StdDuration::from_secs(seconds);
            loop {
                match tokio::time::timeout_at(deadline, listener.next()).await {
                    Ok(value) => {
                        println!(
                            "handle=0x{:04x} kind={} value={}",
                            value.handle(),
                            if value.is_indication() {
                                "indication"
                            } else {
                                "notification"
                            },
                            hex(value.as_ref())
                        );
                        if value.is_indication() {
                            client.confirm_indication().await.map_err(debug_error)?;
                        }
                    }
                    Err(_) => break,
                }
            }
            client
                .unsubscribe(&characteristic)
                .await
                .map_err(debug_error)?;
            Ok(())
        }
        _ => unreachable!(),
    }
}

async fn inspect(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
) -> AppResult<()> {
    for service in client.services().await.map_err(debug_error)? {
        println!(
            "service {} handles={:04x}-{:04x}",
            format_uuid(service.uuid()),
            service.handle_range().start(),
            service.handle_range().end()
        );
        for characteristic in client
            .characteristics::<MAX_CHARACTERISTICS>(&service)
            .await
            .map_err(debug_error)?
        {
            println!(
                "  characteristic {} handle=0x{:04x} properties=0x{:02x} cccd={}",
                format_uuid(characteristic.uuid),
                characteristic.handle,
                characteristic.props.to_raw(),
                characteristic
                    .cccd_handle
                    .map(|h| format!("0x{h:04x}"))
                    .unwrap_or_else(|| "-".into())
            );
            for descriptor in client
                .descriptors::<_, MAX_DESCRIPTORS>(&characteristic)
                .await
                .map_err(debug_error)?
            {
                println!(
                    "    descriptor {} handle=0x{:04x}",
                    format_uuid(*descriptor.uuid()),
                    descriptor.handle()
                );
            }
        }
    }
    Ok(())
}

async fn find_characteristic(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    uuid: Uuid,
) -> AppResult<Characteristic<[u8]>> {
    for service in client.services().await.map_err(debug_error)? {
        for characteristic in client
            .characteristics::<MAX_CHARACTERISTICS>(&service)
            .await
            .map_err(debug_error)?
        {
            if characteristic.uuid == uuid {
                return Ok(characteristic);
            }
        }
    }
    Err(format!("characteristic {} was not found", format_uuid(uuid)).into())
}

fn parse_args() -> AppResult<(Option<String>, Command)> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let controller = if args.first().map(String::as_str) == Some("--controller") {
        if args.len() < 3 {
            return Err(usage().into());
        }
        let id = args.remove(1);
        args.remove(0);
        Some(id)
    } else {
        None
    };
    let command = match args.first().map(String::as_str) {
        Some("scan") => Command::Scan {
            seconds: option_u64(&args, "--seconds", 10)?,
        },
        Some("connect") if args.len() >= 2 => Command::Connect {
            peer: parse_address(&args[1])?,
            seconds: option_u64(&args, "--seconds", 5)?,
        },
        Some("inspect") if args.len() == 2 => Command::Inspect {
            peer: parse_address(&args[1])?,
        },
        Some("read") if args.len() == 3 => Command::Read {
            peer: parse_address(&args[1])?,
            uuid: parse_uuid(&args[2])?,
        },
        Some("write") if args.len() >= 4 => Command::Write {
            peer: parse_address(&args[1])?,
            uuid: parse_uuid(&args[2])?,
            value: parse_hex(&args[3])?,
            without_response: args.iter().any(|v| v == "--without-response"),
        },
        Some("subscribe") if args.len() >= 3 => Command::Subscribe {
            peer: parse_address(&args[1])?,
            uuid: parse_uuid(&args[2])?,
            indications: args.iter().any(|v| v == "--indications"),
            seconds: option_u64(&args, "--seconds", 30)?,
        },
        _ => return Err(usage().into()),
    };
    Ok((controller, command))
}
fn usage() -> &'static str {
    "usage: directhci-ble [--controller ID] scan [--seconds N]\n       directhci-ble [--controller ID] connect <public|random:AA:BB:CC:DD:EE:FF> [--seconds N]\n       directhci-ble [--controller ID] inspect <address>\n       directhci-ble [--controller ID] read <address> <16-or-128-bit-uuid>\n       directhci-ble [--controller ID] write <address> <uuid> <hex> [--without-response]\n       directhci-ble [--controller ID] subscribe <address> <uuid> [--indications] [--seconds N]"
}
fn option_u64(args: &[String], option: &str, default: u64) -> AppResult<u64> {
    match args.iter().position(|v| v == option) {
        Some(i) => args
            .get(i + 1)
            .ok_or_else(|| format!("{option} requires a value").into())
            .and_then(|v| v.parse().map_err(Into::into)),
        None => Ok(default),
    }
}

fn parse_address(value: &str) -> AppResult<Address> {
    let (kind, raw) = value
        .split_once(':')
        .ok_or("address must start with public: or random:")?;
    let mut bytes = parse_hex(&raw.replace(':', ""))?;
    if bytes.len() != 6 {
        return Err("Bluetooth address must contain 6 bytes".into());
    }
    bytes.reverse();
    let addr = bt_hci::param::BdAddr::new(bytes.try_into().unwrap());
    let kind = match kind.to_ascii_lowercase().as_str() {
        "public" => bt_hci::param::AddrKind::PUBLIC,
        "random" => bt_hci::param::AddrKind::RANDOM,
        _ => return Err("address type must be public or random".into()),
    };
    Ok(Address::new(kind, addr))
}
fn format_address(value: Address) -> String {
    let kind = if value.kind.as_raw() & 1 == 0 {
        "public"
    } else {
        "random"
    };
    let mut bytes = value.addr.into_inner();
    bytes.reverse();
    format!(
        "{kind}:{}",
        bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    )
}
fn parse_uuid(value: &str) -> AppResult<Uuid> {
    let compact = value.replace('-', "");
    match compact.len() {
        4 => Ok(Uuid::new_short(u16::from_str_radix(&compact, 16)?)),
        32 => {
            let number = u128::from_str_radix(&compact, 16)?;
            Ok(Uuid::new_long(number.to_le_bytes()))
        }
        _ => Err("UUID must be 4 hex digits or a 128-bit UUID".into()),
    }
}
fn format_uuid(value: Uuid) -> String {
    match value {
        Uuid::Uuid16(v) => format!("{:04x}", u16::from_le_bytes(v)),
        Uuid::Uuid32(v) => format!("{:08x}", u32::from_le_bytes(v)),
        Uuid::Uuid128(v) => {
            let s = format!("{:032x}", u128::from_le_bytes(v));
            format!(
                "{}-{}-{}-{}-{}",
                &s[0..8],
                &s[8..12],
                &s[12..16],
                &s[16..20],
                &s[20..32]
            )
        }
    }
}
fn parse_hex(value: &str) -> AppResult<Vec<u8>> {
    let compact: String = value
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != ':' && *c != '-')
        .collect();
    if compact.len() % 2 != 0 {
        return Err("hex data must contain complete bytes".into());
    }
    (0..compact.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&compact[i..i + 2], 16).map_err(Into::into))
        .collect()
}
fn hex(value: &[u8]) -> String {
    value.iter().map(|v| format!("{v:02x}")).collect()
}
fn parse_advertisement(data: &[u8]) -> (Option<String>, Vec<String>) {
    let mut name = None;
    let mut uuids = Vec::new();
    for item in AdStructure::decode(data).filter_map(Result::ok) {
        match item {
            AdStructure::CompleteLocalName(v) | AdStructure::ShortenedLocalName(v) => {
                name = Some(String::from_utf8_lossy(v).into_owned())
            }
            AdStructure::IncompleteServiceUuids16(v) | AdStructure::CompleteServiceUuids16(v) => {
                uuids.extend(v.iter().map(|v| format!("{:04x}", u16::from_le_bytes(*v))))
            }
            AdStructure::IncompleteServiceUuids32(v) | AdStructure::CompleteServiceUuids32(v) => {
                uuids.extend(v.iter().map(|v| format!("{:08x}", u32::from_le_bytes(*v))))
            }
            AdStructure::IncompleteServiceUuids128(v) | AdStructure::CompleteServiceUuids128(v) => {
                uuids.extend(v.iter().map(|v| format_uuid(Uuid::new_long(*v))))
            }
            _ => {}
        }
    }
    (name, uuids)
}
fn local_address() -> Address {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut bytes = [0u8; 6];
    bytes.copy_from_slice(&now.to_le_bytes()[..6]);
    bytes[5] = (bytes[5] & 0x3f) | 0xc0;
    Address::random(bytes)
}
fn debug_error(error: impl std::fmt::Debug) -> Box<dyn std::error::Error + Send + Sync> {
    format!("{error:?}").into()
}

#[allow(dead_code)]
fn _keep_advertisement_error_public(_: AdvertisementDataError) {}
