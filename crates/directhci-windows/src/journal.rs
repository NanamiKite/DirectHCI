//! Durable single-controller ownership journal storage.
//!
//! The file records intent and transition history. Windows device state is
//! always re-observed before recovery takes action.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use directhci_core::{OWNERSHIP_JOURNAL_SCHEMA_VERSION, OwnershipJournal};

const JOURNAL_FILE_NAME: &str = "ownership-journal-v1.json";
const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
const STALE_AFTER: Duration = Duration::from_secs(15 * 60);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalStore {
    directory: PathBuf,
    secure_program_data: bool,
}

impl JournalStore {
    pub fn program_data() -> Result<Self, JournalError> {
        platform::program_data_directory().map(|directory| Self {
            directory: directory.join("DirectHCI"),
            secure_program_data: true,
        })
    }

    pub fn at_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            secure_program_data: false,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.directory.join(JOURNAL_FILE_NAME)
    }

    /// Verifies that the journal directory can be used without leaving an
    /// ownership record behind. This is used by takeover preflight.
    pub fn probe_writable(&self) -> Result<(), JournalError> {
        self.ensure_directory()?;
        let probe_path = self.temporary_path();
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe_path)
                .map_err(|error| {
                    JournalError::io("create journal write probe", &probe_path, error)
                })?;
            file.write_all(b"directhci-journal-write-probe\n")
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_all())
                .map_err(|error| {
                    JournalError::io("flush journal write probe", &probe_path, error)
                })?;
            drop(file);
            fs::remove_file(&probe_path).map_err(|error| {
                JournalError::io("remove journal write probe", &probe_path, error)
            })?;
            platform::sync_directory(&self.directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&probe_path);
        }
        result
    }

    pub fn load(&self) -> Result<JournalLoad, JournalError> {
        self.validate_directory()?;
        let path = self.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(JournalLoad::Missing);
            }
            Err(error) => return Err(JournalError::io("read journal metadata", &path, error)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(JournalError::UnsafePath {
                path,
                message: "journal must be a regular file, not a link or special file".into(),
            });
        }
        if self.secure_program_data {
            platform::reject_reparse(&path).map_err(|message| JournalError::UnsafePath {
                path: path.clone(),
                message,
            })?;
        }
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Err(JournalError::Corrupt {
                path,
                message: format!(
                    "journal is {} bytes; maximum is {MAX_JOURNAL_BYTES}",
                    metadata.len()
                ),
            });
        }

        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|error| JournalError::io("read journal", &path, error))?;
        let journal: OwnershipJournal =
            serde_json::from_slice(&bytes).map_err(|error| JournalError::Corrupt {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if journal.schema_version != OWNERSHIP_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                path,
                found: journal.schema_version,
                supported: OWNERSHIP_JOURNAL_SCHEMA_VERSION,
            });
        }

        let now = unix_time_ms()?;
        if journal.lease_id.as_str().trim().is_empty() {
            return Err(JournalError::Corrupt {
                path,
                message: "lease ID is empty".into(),
            });
        }
        if journal.created_unix_ms > journal.updated_unix_ms {
            return Err(JournalError::Corrupt {
                path,
                message: "created timestamp is newer than updated timestamp".into(),
            });
        }
        if journal.updated_unix_ms
            > now.saturating_add(Duration::from_secs(5 * 60).as_millis() as u64)
        {
            return Err(JournalError::Corrupt {
                path,
                message: "updated timestamp is unreasonably far in the future".into(),
            });
        }
        let age_ms = now.saturating_sub(journal.updated_unix_ms);
        Ok(JournalLoad::Present {
            stale: age_ms > STALE_AFTER.as_millis() as u64,
            age_ms,
            journal: Box::new(journal),
        })
    }

    /// Creates the first durable acquisition record without replacing an
    /// existing one. The no-replace commit is the process-level lease
    /// reservation: concurrent acquisitions cannot both pass it.
    pub fn create(&self, journal: &OwnershipJournal) -> Result<(), JournalError> {
        self.commit(journal, CommitMode::CreateNew)
    }

    /// Atomically replaces the journal after flushing the complete temporary
    /// file. On Windows the final move uses MOVEFILE_WRITE_THROUGH.
    pub fn write(&self, journal: &OwnershipJournal) -> Result<(), JournalError> {
        self.commit(journal, CommitMode::Replace)
    }

    fn commit(&self, journal: &OwnershipJournal, mode: CommitMode) -> Result<(), JournalError> {
        if journal.schema_version != OWNERSHIP_JOURNAL_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                path: self.path(),
                found: journal.schema_version,
                supported: OWNERSHIP_JOURNAL_SCHEMA_VERSION,
            });
        }
        self.ensure_directory()?;

        let mut bytes =
            serde_json::to_vec_pretty(journal).map_err(|error| JournalError::Serialization {
                message: error.to_string(),
            })?;
        bytes.push(b'\n');
        let temp_path = self.temporary_path();
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .map_err(|error| JournalError::io("create temporary journal", &temp_path, error))?;
            file.write_all(&bytes)
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_all())
                .map_err(|error| JournalError::io("flush temporary journal", &temp_path, error))?;
            drop(file);
            match mode {
                CommitMode::CreateNew => platform::atomic_create(&temp_path, &self.path())?,
                CommitMode::Replace => platform::atomic_replace(&temp_path, &self.path())?,
            }
            platform::sync_directory(&self.directory)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    /// Removes the journal only after WindowsOwned has been freshly verified.
    /// A crash before removal merely leaves a recoverable, already-satisfied
    /// record behind.
    pub fn clear(&self) -> Result<(), JournalError> {
        self.validate_directory()?;
        let path = self.path();
        if self.secure_program_data {
            match fs::symlink_metadata(&path) {
                Ok(_) => {
                    platform::reject_reparse(&path).map_err(|message| JournalError::UnsafePath {
                        path: path.clone(),
                        message,
                    })?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => {
                    return Err(JournalError::io("inspect journal metadata", &path, error));
                }
            }
        }
        match fs::remove_file(&path) {
            Ok(()) => platform::sync_directory(&self.directory),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(JournalError::io("remove journal", &path, error)),
        }
    }

    fn temporary_path(&self) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.directory.join(format!(
            ".{JOURNAL_FILE_NAME}.tmp.{}.{}",
            std::process::id(),
            sequence
        ))
    }

    fn ensure_directory(&self) -> Result<(), JournalError> {
        if self.secure_program_data {
            platform::ensure_secure_directory(&self.directory).map_err(|message| {
                JournalError::UnsafePath {
                    path: self.directory.clone(),
                    message,
                }
            })?;
        } else {
            fs::create_dir_all(&self.directory).map_err(|error| {
                JournalError::io("create journal directory", &self.directory, error)
            })?;
        }
        self.validate_directory()
    }

    fn validate_directory(&self) -> Result<(), JournalError> {
        if self.secure_program_data {
            platform::validate_secure_directory(&self.directory).map_err(|message| {
                JournalError::UnsafePath {
                    path: self.directory.clone(),
                    message,
                }
            })?;
        }
        if !self.directory.exists() {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(&self.directory).map_err(|error| {
            JournalError::io("inspect journal directory", &self.directory, error)
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(JournalError::UnsafePath {
                path: self.directory.clone(),
                message: "journal directory must be a real directory, not a link or special file"
                    .into(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum CommitMode {
    CreateNew,
    Replace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalLoad {
    Missing,
    Present {
        journal: Box<OwnershipJournal>,
        stale: bool,
        age_ms: u64,
    },
}

#[derive(Debug)]
pub enum JournalError {
    UnsupportedPlatform,
    Io {
        operation: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    Serialization {
        message: String,
    },
    AlreadyExists {
        path: PathBuf,
    },
    UnsafePath {
        path: PathBuf,
        message: String,
    },
    Corrupt {
        path: PathBuf,
        message: String,
    },
    UnsupportedSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    ClockBeforeUnixEpoch,
}

impl JournalError {
    fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_owned(),
            source,
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => formatter
                .write_str("the default DirectHCI journal location is only available on Windows"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} `{}` failed: {source}",
                path.display()
            ),
            Self::Serialization { message } => {
                write!(formatter, "journal serialization failed: {message}")
            }
            Self::AlreadyExists { path } => write!(
                formatter,
                "ownership journal `{}` already exists; recover it before acquiring",
                path.display()
            ),
            Self::UnsafePath { path, message } => {
                write!(
                    formatter,
                    "unsafe journal path `{}`: {message}",
                    path.display()
                )
            }
            Self::Corrupt { path, message } => {
                write!(
                    formatter,
                    "journal `{}` is corrupt: {message}",
                    path.display()
                )
            }
            Self::UnsupportedSchema {
                path,
                found,
                supported,
            } => write!(
                formatter,
                "journal `{}` uses schema {found}; supported schema is {supported}",
                path.display()
            ),
            Self::ClockBeforeUnixEpoch => formatter.write_str("system clock is before Unix epoch"),
        }
    }
}

impl std::error::Error for JournalError {}

pub(crate) fn unix_time_ms() -> Result<u64, JournalError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|_| JournalError::ClockBeforeUnixEpoch)
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    use windows::core::PCWSTR;

    use super::*;

    pub(super) fn ensure_secure_directory(path: &Path) -> Result<(), String> {
        crate::security::ensure_secure_journal_directory(path)
    }

    pub(super) fn validate_secure_directory(path: &Path) -> Result<(), String> {
        crate::security::validate_secure_journal_directory(path)
    }

    pub(super) fn reject_reparse(path: &Path) -> Result<(), String> {
        crate::security::reject_reparse(path)
    }

    pub(super) fn program_data_directory() -> Result<PathBuf, JournalError> {
        let raw = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, KF_FLAG_DEFAULT, None) }
            .map_err(|error| JournalError::Io {
                operation: "resolve ProgramData",
                path: PathBuf::from("%ProgramData%"),
                source: std::io::Error::other(error.to_string()),
            })?;
        let text = unsafe { raw.to_string() }.map_err(|error| JournalError::Io {
            operation: "decode ProgramData",
            path: PathBuf::from("%ProgramData%"),
            source: std::io::Error::other(error.to_string()),
        });
        unsafe { CoTaskMemFree(Some(raw.0.cast::<c_void>())) };
        text.map(PathBuf::from)
    }

    pub(super) fn atomic_replace(source: &Path, destination: &Path) -> Result<(), JournalError> {
        let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination_wide: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        unsafe {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|error| JournalError::Io {
            operation: "atomically replace journal",
            path: destination.to_owned(),
            source: std::io::Error::other(error.to_string()),
        })
    }

    pub(super) fn atomic_create(source: &Path, destination: &Path) -> Result<(), JournalError> {
        let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination_wide: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let result = unsafe {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if result.is_err() && destination.exists() {
            return Err(JournalError::AlreadyExists {
                path: destination.to_owned(),
            });
        }
        result.map_err(|error| JournalError::Io {
            operation: "atomically create journal",
            path: destination.to_owned(),
            source: std::io::Error::other(error.to_string()),
        })
    }

    pub(super) fn sync_directory(_directory: &Path) -> Result<(), JournalError> {
        // MOVEFILE_WRITE_THROUGH provides the Windows durability boundary for
        // the replace. Opening directories for flush requires backup semantics
        // and does not strengthen the documented MoveFileEx contract here.
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) fn ensure_secure_directory(_path: &Path) -> Result<(), String> {
        Ok(())
    }
    pub(super) fn validate_secure_directory(_path: &Path) -> Result<(), String> {
        Ok(())
    }
    pub(super) fn reject_reparse(_path: &Path) -> Result<(), String> {
        Ok(())
    }

    pub(super) fn program_data_directory() -> Result<PathBuf, JournalError> {
        Err(JournalError::UnsupportedPlatform)
    }

    pub(super) fn atomic_replace(source: &Path, destination: &Path) -> Result<(), JournalError> {
        fs::rename(source, destination)
            .map_err(|error| JournalError::io("atomically replace journal", destination, error))
    }

    pub(super) fn atomic_create(source: &Path, destination: &Path) -> Result<(), JournalError> {
        match fs::hard_link(source, destination) {
            Ok(()) => fs::remove_file(source)
                .map_err(|error| JournalError::io("remove temporary journal", source, error)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(JournalError::AlreadyExists {
                    path: destination.to_owned(),
                })
            }
            Err(error) => Err(JournalError::io(
                "atomically create journal",
                destination,
                error,
            )),
        }
    }

    pub(super) fn sync_directory(directory: &Path) -> Result<(), JournalError> {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| JournalError::io("flush journal directory", directory, error))
    }
}

#[cfg(test)]
mod tests {
    use directhci_core::{
        ControllerIdentity, ControllerObservation, DeviceStatus, DriverObservation,
        DriverPackageIdentity, LeaseId, LeaseOwnerMetadata, OwnershipJournal,
    };

    use super::*;

    fn journal(updated_unix_ms: u64) -> OwnershipJournal {
        let identity = ControllerIdentity::new(
            Some(0x1234),
            Some(0xabcd),
            None,
            None,
            "USB\\VID_1234&PID_ABCD\\INSTANCE".into(),
            vec!["USBROOT(0)#USB(1)".into()],
            None,
            vec!["USB\\VID_1234&PID_ABCD".into()],
            vec!["USB\\CLASS_E0&SUBCLASS_01&PROT_01".into()],
        );
        let observation = ControllerObservation::new(
            identity.clone(),
            None,
            None,
            None,
            None,
            None,
            Some("USB".into()),
            Some("BTHUSB".into()),
            DriverObservation {
                inf_path: Some("oem1.inf".into()),
                provider: Some("Vendor".into()),
                version: Some("1.0".into()),
                driver_key: None,
            },
            Vec::new(),
            DeviceStatus {
                present: true,
                status_flags: Some(1),
                problem_code: Some(0),
            },
        );
        OwnershipJournal::new(
            LeaseId::new("test-lease").unwrap(),
            identity,
            observation,
            DriverPackageIdentity {
                provider: "DirectHCI Project".into(),
                description: "DirectHCI WinUSB Controller".into(),
                published_inf: Some("oem2.inf".into()),
                version: Some("0.1.0.0".into()),
                device_interface_guid: "{00000000-0000-0000-0000-000000000001}".into(),
            },
            updated_unix_ms,
            updated_unix_ms,
            LeaseOwnerMetadata::default(),
        )
    }

    fn test_store(label: &str) -> JournalStore {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        JournalStore::at_directory(std::env::temp_dir().join(format!(
            "directhci-journal-test-{}-{label}-{sequence}",
            std::process::id()
        )))
    }

    #[test]
    fn journal_round_trips_and_clear_is_idempotent() {
        let store = test_store("round-trip");
        let journal = journal(unix_time_ms().unwrap());
        store.write(&journal).unwrap();
        assert!(matches!(
            store.load().unwrap(),
            JournalLoad::Present {
                journal: loaded,
                stale: false,
                ..
            } if *loaded == journal
        ));
        store.clear().unwrap();
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), JournalLoad::Missing);
        let _ = fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn journal_create_is_an_exclusive_reservation() {
        let store = test_store("exclusive-create");
        let first = journal(unix_time_ms().unwrap());
        let mut second = first.clone();
        second.owner.client_label = Some("second owner".into());
        store.create(&first).unwrap();
        assert!(matches!(
            store.create(&second),
            Err(JournalError::AlreadyExists { .. })
        ));
        assert!(matches!(
            store.load().unwrap(),
            JournalLoad::Present { journal: loaded, .. } if *loaded == first
        ));
        let _ = fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn corrupt_journal_is_detected_and_preserved() {
        let store = test_store("corrupt");
        fs::create_dir_all(&store.directory).unwrap();
        fs::write(store.path(), b"{not-json").unwrap();
        assert!(matches!(store.load(), Err(JournalError::Corrupt { .. })));
        assert!(store.path().exists());
        let _ = fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let store = test_store("schema");
        let mut value = serde_json::to_value(journal(unix_time_ms().unwrap())).unwrap();
        value["schema_version"] = serde_json::json!(999);
        fs::create_dir_all(&store.directory).unwrap();
        fs::write(store.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            store.load(),
            Err(JournalError::UnsupportedSchema { found: 999, .. })
        ));
        let _ = fs::remove_dir_all(&store.directory);
    }

    #[test]
    fn old_journal_is_marked_stale_not_discarded() {
        let store = test_store("stale");
        store.write(&journal(1)).unwrap();
        assert!(matches!(
            store.load().unwrap(),
            JournalLoad::Present { stale: true, .. }
        ));
        let _ = fs::remove_dir_all(&store.directory);
    }
}
