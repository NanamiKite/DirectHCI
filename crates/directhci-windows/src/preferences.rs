//! Minimal daemon-owned preference file. Controller identity remains an opaque
//! index; every privileged operation still performs fresh enumeration.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use directhci_core::RuntimePreferences;
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::PCWSTR;

use crate::JournalStore;

const CONFIG_FILE: &str = "config.json";
const MAX_CONFIG_BYTES: u64 = 16 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct PreferencesStore {
    directory: PathBuf,
}

impl PreferencesStore {
    pub fn program_data() -> Result<Self, String> {
        let path = JournalStore::program_data()
            .map_err(|error| error.to_string())?
            .path();
        Ok(Self {
            directory: path
                .parent()
                .ok_or("journal path has no parent")?
                .to_owned(),
        })
    }

    pub fn load(&self) -> Result<RuntimePreferences, String> {
        crate::security::validate_secure_journal_directory(&self.directory)?;
        let path = self.directory.join(CONFIG_FILE);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RuntimePreferences::default());
            }
            Err(error) => return Err(format!("inspect config: {error}")),
        };
        if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            return Err("config is not a regular, bounded file".into());
        }
        crate::security::validate_secure_data_file(&path)?;
        let bytes = fs::read(&path).map_err(|error| format!("read config: {error}"))?;
        serde_json::from_slice(&bytes).map_err(|error| format!("decode config: {error}"))
    }

    pub fn save(&self, preferences: &RuntimePreferences) -> Result<(), String> {
        crate::security::ensure_secure_journal_directory(&self.directory)?;
        let path = self.directory.join(CONFIG_FILE);
        if fs::symlink_metadata(&path).is_ok() {
            crate::security::reject_reparse(&path)?;
        }
        let temp = self.directory.join(format!(
            ".config.json.tmp.{}.{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|error| format!("create temporary config: {error}"))?;
            serde_json::to_writer_pretty(&mut file, preferences)
                .map_err(|error| format!("encode config: {error}"))?;
            file.write_all(b"\n")
                .and_then(|_| file.flush())
                .and_then(|_| file.sync_all())
                .map_err(|error| format!("flush config: {error}"))?;
            drop(file);
            let from = wide(&temp);
            let to = wide(&path);
            // SAFETY: both NUL-terminated paths stay live through the call.
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
            .map_err(|error| format!("replace config: {error}"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
