use std::time::{Duration, UNIX_EPOCH};

use directhci_ble::{
    BleAddress, BleCentralConfig, BleConnection, BleError, BleUuid, DirectHciBleCentral, GattEvent,
    GattNotificationStream, NotificationKind, SubscriptionMode, WriteMode,
};

type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
enum Command {
    Scan {
        seconds: u64,
    },
    Connect {
        peer: BleAddress,
        seconds: u64,
    },
    Inspect {
        peer: BleAddress,
    },
    Read {
        peer: BleAddress,
        uuid: BleUuid,
    },
    Write {
        peer: BleAddress,
        uuid: BleUuid,
        value: Vec<u8>,
        mode: WriteMode,
    },
    Subscribe {
        peer: BleAddress,
        uuid: BleUuid,
        mode: SubscriptionMode,
        seconds: u64,
    },
    Listen {
        peer: BleAddress,
        uuid: BleUuid,
        seconds: u64,
    },
    ListenAll {
        peer: BleAddress,
        seconds: u64,
    },
    Exchange {
        peer: BleAddress,
        write_uuid: BleUuid,
        value: Vec<u8>,
        mode: WriteMode,
        listen: ListenTarget,
        seconds: u64,
    },
}

#[derive(Clone, Copy, Debug)]
enum ListenTarget {
    Characteristic(BleUuid),
    All,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("directhci-ble: {error}");
        std::process::exit(1);
    }
}

async fn run() -> AppResult<()> {
    let (controller_id, command) = parse_args()?;
    let central = DirectHciBleCentral::connect(BleCentralConfig {
        controller_id,
        client_name: "directhci-ble".into(),
        client_version: Some(env!("CARGO_PKG_VERSION").into()),
    })
    .await?;

    match command {
        Command::Scan { seconds } => {
            let operation = tokio::select! {
                result = central.scan(Duration::from_secs(seconds)) => {
                    result.map(|records| {
                        for record in records {
                            println!(
                                "{}  rssi={}  name={}  services=[{}]  adv={}  scan_rsp={}",
                                record.address,
                                record.rssi,
                                record.local_name.as_deref().unwrap_or("-"),
                                record
                                    .service_uuids
                                    .iter()
                                    .map(ToString::to_string)
                                    .collect::<Vec<_>>()
                                    .join(","),
                                record
                                    .advertisement_data
                                    .iter()
                                    .map(|value| hex(value))
                                    .collect::<Vec<_>>()
                                    .join(","),
                                record
                                    .scan_response_data
                                    .iter()
                                    .map(|value| hex(value))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            );
                        }
                    })
                }
                _ = tokio::signal::ctrl_c() => {
                    Err(BleError::InvalidState("operation interrupted by Ctrl+C"))
                }
            };
            combine(operation, central.shutdown().await).map_err(Into::into)
        }
        command => {
            let peer = command.peer();
            let connection = central.connect_device(peer).await?;
            let operation = execute_connected(&connection, command).await;
            combine(operation, connection.disconnect().await).map_err(Into::into)
        }
    }
}

async fn execute_connected(connection: &BleConnection, command: Command) -> Result<(), BleError> {
    match command {
        Command::Connect { peer, seconds } => {
            println!("connected {} att_mtu={}", peer, connection.info().att_mtu);
            wait_or_interrupt(seconds).await
        }
        Command::Inspect { .. } => {
            for service in connection.discover().await? {
                println!(
                    "service {} handles={:04x}-{:04x}",
                    service.uuid, service.start_handle, service.end_handle
                );
                for characteristic in service.characteristics {
                    println!(
                        "  characteristic {} handle=0x{:04x} properties=0x{:02x} cccd={}",
                        characteristic.uuid,
                        characteristic.value_handle,
                        characteristic.properties,
                        characteristic
                            .cccd_handle
                            .map(|handle| format!("0x{handle:04x}"))
                            .unwrap_or_else(|| "-".into())
                    );
                    for descriptor in characteristic.descriptors {
                        println!(
                            "    descriptor {} handle=0x{:04x}",
                            descriptor.uuid, descriptor.handle
                        );
                    }
                }
            }
            Ok(())
        }
        Command::Read { uuid, .. } => {
            let characteristic = connection.find_characteristic(uuid).await?;
            println!("{}", hex(&connection.read(&characteristic).await?));
            Ok(())
        }
        Command::Write {
            uuid, value, mode, ..
        } => {
            let characteristic = connection.find_characteristic(uuid).await?;
            connection
                .write(&characteristic, value.clone(), mode)
                .await?;
            print_write_result(value.len(), mode);
            Ok(())
        }
        Command::Subscribe {
            uuid,
            mode,
            seconds,
            ..
        } => {
            let characteristic = connection.find_characteristic(uuid).await?;
            let stream = connection.subscribe(&characteristic, mode).await?;
            println!("standard subscription ready for {uuid}");
            receive_for(stream, seconds).await
        }
        Command::Listen { uuid, seconds, .. } => {
            let characteristic = connection.find_characteristic(uuid).await?;
            let stream = connection.listen(&characteristic).await?;
            println!("passive listener ready for {uuid}");
            receive_for(stream, seconds).await
        }
        Command::ListenAll { seconds, .. } => {
            let stream = connection.listen_all().await?;
            println!("passive listener ready for all characteristic handles");
            receive_for(stream, seconds).await
        }
        Command::Exchange {
            write_uuid,
            value,
            mode,
            listen,
            seconds,
            ..
        } => {
            let write_characteristic = connection.find_characteristic(write_uuid).await?;
            let stream = match listen {
                ListenTarget::Characteristic(uuid) => {
                    let characteristic = connection.find_characteristic(uuid).await?;
                    let stream = connection.listen(&characteristic).await?;
                    println!("passive listener ready for {uuid}");
                    stream
                }
                ListenTarget::All => {
                    let stream = connection.listen_all().await?;
                    println!("passive listener ready for all characteristic handles");
                    stream
                }
            };
            let operation = connection
                .write(&write_characteristic, value.clone(), mode)
                .await
                .map(|_| print_write_result(value.len(), mode));
            match operation {
                Ok(()) => receive_for(stream, seconds).await,
                Err(primary) => combine(Err(primary), stream.close().await),
            }
        }
        Command::Scan { .. } => unreachable!(),
    }
}

async fn receive_for(mut stream: GattNotificationStream, seconds: u64) -> Result<(), BleError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    let operation = loop {
        tokio::select! {
            result = tokio::time::timeout_at(deadline, stream.recv()) => {
                match result {
                    Ok(Ok(Some(event))) => print_event(&event),
                    Ok(Ok(None)) => break Ok(()),
                    Ok(Err(error)) => break Err(error),
                    Err(_) => break Ok(()),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break Err(BleError::InvalidState("operation interrupted by Ctrl+C"));
            }
        }
    };
    combine(operation, stream.close().await)
}

async fn wait_or_interrupt(seconds: u64) -> Result<(), BleError> {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => Ok(()),
        _ = tokio::signal::ctrl_c() => {
            Err(BleError::InvalidState("operation interrupted by Ctrl+C"))
        }
    }
}

fn print_event(event: &GattEvent) {
    let timestamp = event
        .timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    println!(
        "timestamp_ms={} handle=0x{:04x} kind={} length={} value={}",
        timestamp,
        event.handle,
        match event.kind {
            NotificationKind::Notification => "notification",
            NotificationKind::Indication => "indication",
        },
        event.value.len(),
        hex(&event.value)
    );
}

fn print_write_result(length: usize, mode: WriteMode) {
    println!(
        "wrote {length} bytes ({})",
        match mode {
            WriteMode::WithResponse => "request",
            WriteMode::WithoutResponse => "without response",
        }
    );
}

fn combine(primary: Result<(), BleError>, cleanup: Result<(), BleError>) -> Result<(), BleError> {
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(BleError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}

impl Command {
    fn peer(&self) -> BleAddress {
        match self {
            Self::Connect { peer, .. }
            | Self::Inspect { peer }
            | Self::Read { peer, .. }
            | Self::Write { peer, .. }
            | Self::Subscribe { peer, .. }
            | Self::Listen { peer, .. }
            | Self::ListenAll { peer, .. }
            | Self::Exchange { peer, .. } => *peer,
            Self::Scan { .. } => unreachable!(),
        }
    }
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
    let write_mode = |values: &[String]| {
        if values.iter().any(|value| value == "--without-response") {
            WriteMode::WithoutResponse
        } else {
            WriteMode::WithResponse
        }
    };
    let command = match args.first().map(String::as_str) {
        Some("scan") => Command::Scan {
            seconds: option_u64(&args, "--seconds", 10)?,
        },
        Some("connect") if args.len() >= 2 => Command::Connect {
            peer: args[1].parse()?,
            seconds: option_u64(&args, "--seconds", 5)?,
        },
        Some("inspect") if args.len() == 2 => Command::Inspect {
            peer: args[1].parse()?,
        },
        Some("read") if args.len() == 3 => Command::Read {
            peer: args[1].parse()?,
            uuid: args[2].parse()?,
        },
        Some("write") if args.len() >= 4 => Command::Write {
            peer: args[1].parse()?,
            uuid: args[2].parse()?,
            value: parse_hex(&args[3])?,
            mode: write_mode(&args),
        },
        Some("subscribe") if args.len() >= 3 => Command::Subscribe {
            peer: args[1].parse()?,
            uuid: args[2].parse()?,
            mode: if args.iter().any(|value| value == "--indications") {
                SubscriptionMode::Indications
            } else {
                SubscriptionMode::Notifications
            },
            seconds: option_u64(&args, "--seconds", 30)?,
        },
        Some("listen") if args.len() >= 3 => Command::Listen {
            peer: args[1].parse()?,
            uuid: args[2].parse()?,
            seconds: option_u64(&args, "--seconds", 30)?,
        },
        Some("listen-all") if args.len() >= 2 => Command::ListenAll {
            peer: args[1].parse()?,
            seconds: option_u64(&args, "--seconds", 30)?,
        },
        Some("exchange") if args.len() >= 4 => {
            let write_uuid = args[2].parse()?;
            let listen_uuid = option_value(&args, "--listen")?
                .map(str::parse)
                .transpose()?;
            let listen_all = args.iter().any(|value| value == "--listen-all");
            if listen_uuid.is_some() && listen_all {
                return Err("--listen and --listen-all are mutually exclusive".into());
            }
            Command::Exchange {
                peer: args[1].parse()?,
                write_uuid,
                value: parse_hex(&args[3])?,
                mode: write_mode(&args),
                listen: if listen_all {
                    ListenTarget::All
                } else {
                    ListenTarget::Characteristic(listen_uuid.unwrap_or(write_uuid))
                },
                seconds: option_u64(&args, "--seconds", 30)?,
            }
        }
        _ => return Err(usage().into()),
    };
    Ok((controller, command))
}

fn usage() -> &'static str {
    "usage: directhci-ble [--controller ID] scan [--seconds N]\n       directhci-ble [--controller ID] connect <public|random:AA:BB:CC:DD:EE:FF> [--seconds N]\n       directhci-ble [--controller ID] inspect <address>\n       directhci-ble [--controller ID] read <address> <16-or-128-bit-uuid>\n       directhci-ble [--controller ID] write <address> <uuid> <hex> [--without-response]\n       directhci-ble [--controller ID] subscribe <address> <uuid> [--indications] [--seconds N]\n       directhci-ble [--controller ID] listen <address> <uuid> [--seconds N]\n       directhci-ble [--controller ID] listen-all <address> [--seconds N]\n       directhci-ble [--controller ID] exchange <address> <write-uuid> <hex> [--listen <uuid> | --listen-all] [--without-response] [--seconds N]"
}

fn option_u64(args: &[String], option: &str, default: u64) -> AppResult<u64> {
    match args.iter().position(|value| value == option) {
        Some(index) => args
            .get(index + 1)
            .ok_or_else(|| format!("{option} requires a value").into())
            .and_then(|value| value.parse().map_err(Into::into)),
        None => Ok(default),
    }
}

fn option_value<'a>(args: &'a [String], option: &str) -> AppResult<Option<&'a str>> {
    match args.iter().position(|value| value == option) {
        Some(index) => args
            .get(index + 1)
            .map(String::as_str)
            .map(Some)
            .ok_or_else(|| format!("{option} requires a value").into()),
        None => Ok(None),
    }
}

fn parse_hex(value: &str) -> AppResult<Vec<u8>> {
    let compact: String = value
        .chars()
        .filter(|character| {
            !character.is_ascii_whitespace() && *character != ':' && *character != '-'
        })
        .collect();
    if compact.len() % 2 != 0 {
        return Err("hex data must contain complete bytes".into());
    }
    (0..compact.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&compact[index..index + 2], 16).map_err(Into::into))
        .collect()
}

fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
