//! Durable storage for swap state.
//!
//! The engines write a record **before** every irreversible step (broadcasting a funding transaction, locking
//! stablecoins, handing out a pre-signature, broadcasting a claim): invariant 5, "persist before broadcast". A crash
//! at any point leaves enough in the store to finish the swap or refund it.
//!
//! Records hold secrets (swap secrets, adaptor secrets, per-swap private keys, refund signatures). Treat a store like
//! a wallet file: restrict access, back it up, and never share it.

use core::fmt;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

/// A store failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A key outside `[A-Za-z0-9._:/-]{1,200}` (or with `..`, or starting with `/`).
    #[error("invalid store key {0:?}")]
    InvalidKey(String),
    /// Another process holds the store directory.
    #[error("the store directory {} is in use by another process", .0.display())]
    Locked(PathBuf),
    /// A filesystem or database error.
    #[error("{0}")]
    Io(String),
    /// A record that doesn't decode.
    #[error("record {key} is corrupt: {message}")]
    Corrupt {
        /// Its key.
        key: String,
        /// Why.
        message: String,
    },
}

/// Key-value persistence used by [`crate::Taker`] and [`crate::Maker`]. Keys are short ASCII paths such as
/// `taker/<id>` or `maker/swap/<id>` (`[A-Za-z0-9._:/-]`, at most 200 characters); values are JSON documents.
///
/// Implementations must be durable: when `put` returns, the record must survive a crash or power loss. Writes to the
/// same key may be issued concurrently and the last one must win.
#[async_trait]
pub trait SwapStore: Send + Sync {
    /// False for stores that lose data on restart ([`MemoryStore`]). The engines refuse risky options then, and the
    /// builders refuse them on mainnet.
    fn durable(&self) -> bool;
    /// The record under `key`, or `None`.
    async fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>, StoreError>;
    /// Creates or replaces the record under `key`. Returns once it is durably stored.
    async fn put(&self, key: &str, json: &str) -> Result<(), StoreError>;
    /// Removes `key` (no-op if absent).
    async fn delete(&self, key: &str) -> Result<(), StoreError>;
    /// All records whose key starts with `prefix`, with their keys, sorted by key.
    async fn list(&self, prefix: &str) -> Result<Vec<(String, Zeroizing<String>)>, StoreError>;
}

/// Fails unless `key` is a valid store key.
pub fn check_store_key(key: &str) -> Result<(), StoreError> {
    let ok = !key.is_empty()
        && key.len() <= 200
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
        && !key.contains("..")
        && !key.starts_with('/');
    if ok { Ok(()) } else { Err(StoreError::InvalidKey(key.chars().take(200).collect())) }
}

/// Reads and decodes a typed record.
pub(crate) async fn get_record<T: DeserializeOwned>(store: &dyn SwapStore, key: &str) -> Result<Option<T>, StoreError> {
    match store.get(key).await? {
        None => Ok(None),
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| StoreError::Corrupt { key: key.to_owned(), message: e.to_string() }),
    }
}

/// Encodes and writes a typed record.
pub(crate) async fn put_record<T: Serialize>(store: &dyn SwapStore, key: &str, record: &T) -> Result<(), StoreError> {
    let json = Zeroizing::new(
        serde_json::to_string(record)
            .map_err(|e| StoreError::Corrupt { key: key.to_owned(), message: e.to_string() })?,
    );
    store.put(key, &json).await
}

/// Lists and decodes typed records.
pub(crate) async fn list_records<T: DeserializeOwned>(
    store: &dyn SwapStore,
    prefix: &str,
) -> Result<Vec<(String, T)>, StoreError> {
    let mut out = Vec::new();
    for (key, json) in store.list(prefix).await? {
        let record = serde_json::from_str(&json)
            .map_err(|e| StoreError::Corrupt { key: key.clone(), message: e.to_string() })?;
        out.push((key, record));
    }
    Ok(out)
}

// -----------------------------------------------------------------------------------------------------------------

/// An in-process store for tests and throwaway sessions. **Not durable**: everything is lost on exit.
#[derive(Default)]
pub struct MemoryStore {
    data: Mutex<BTreeMap<String, Zeroizing<String>>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn data(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Zeroizing<String>>> {
        self.data.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore").field("records", &self.data().len()).finish()
    }
}

#[async_trait]
impl SwapStore for MemoryStore {
    fn durable(&self) -> bool {
        false
    }

    async fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>, StoreError> {
        check_store_key(key)?;
        Ok(self.data().get(key).cloned())
    }

    async fn put(&self, key: &str, json: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        self.data().insert(key.to_owned(), Zeroizing::new(json.to_owned()));
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        self.data().remove(key);
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, Zeroizing<String>)>, StoreError> {
        Ok(self
            .data()
            .range(prefix.to_owned()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

// -----------------------------------------------------------------------------------------------------------------

/// One JSON file per record in a directory: the durable store for real funds.
///
/// - The directory is created with mode `0700` and files with `0600` (Unix). A directory readable by others is
///   reported with a warning.
/// - Each `put` writes a temporary file, fsyncs it, renames it over the old one and fsyncs the directory, so a record
///   is either the old version or the new one, never a torn write.
/// - Writes are serialized in-process, and an exclusive lock on `<dir>/.lock` keeps a second process (a second maker
///   or taker on the same directory) out.
///
/// The file layout is the TypeScript `FileSwapStore`'s (`taker~<id>.json`, `maker~swap~<id>.json`), but the record
/// formats of the two SDKs differ: don't point both at the same directory.
pub struct FileStore {
    dir: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
    // Holds the exclusive lock for the store's lifetime.
    _lock: std::fs::File,
}

impl fmt::Debug for FileStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileStore").field("dir", &self.dir).finish_non_exhaustive()
    }
}

fn io(e: std::io::Error) -> StoreError {
    StoreError::Io(e.to_string())
}

fn file_name(key: &str) -> String {
    format!("{}.json", key.replace('/', "~"))
}

fn key_of(file: &str) -> Option<String> {
    file.strip_suffix(".json").map(|stem| stem.replace('~', "/"))
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| StoreError::Io(format!("store task failed: {e}")))?
}

impl FileStore {
    /// Opens (creating it if needed) the store directory and takes its exclusive lock.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        create_private_dir(&dir)?;
        let lock_path = dir.join(".lock");
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(&lock_path).map_err(io)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StoreError::Locked(dir)),
            Err(std::fs::TryLockError::Error(e)) => return Err(io(e)),
        }
        Ok(Self { dir, write_lock: tokio::sync::Mutex::new(()), _lock: lock })
    }

    /// The store directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(file_name(key))
    }
}

fn create_private_dir(dir: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(io)?;
        }
        let mode = std::fs::metadata(dir).map_err(io)?.permissions().mode();
        if mode & 0o077 != 0 {
            tracing::warn!(
                dir = %dir.display(),
                "the swap store directory is accessible by other users; it holds private keys (chmod 700 it)"
            );
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir).map_err(io)?;
    Ok(())
}

fn sync_dir(dir: &Path) {
    // Persists renames and removals. Not supported on every platform; the rename itself is atomic regardless.
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

fn write_atomically(dir: &Path, target: &Path, name: &str, json: &[u8]) -> Result<(), StoreError> {
    let mut suffix = [0u8; 6];
    rand_core::RngCore::fill_bytes(&mut paralock_core::OsRng, &mut suffix);
    let tmp = dir.join(format!(".{name}.{}.tmp", paralock_core::encoding::to_hex(&suffix)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(json)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, target)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    sync_dir(dir);
    Ok(())
}

#[async_trait]
impl SwapStore for FileStore {
    fn durable(&self) -> bool {
        true
    }

    async fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>, StoreError> {
        check_store_key(key)?;
        let path = self.path(key);
        blocking(move || match std::fs::read_to_string(&path) {
            Ok(json) => Ok(Some(Zeroizing::new(json))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        })
        .await
    }

    async fn put(&self, key: &str, json: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        let _guard = self.write_lock.lock().await;
        let (dir, target, name) = (self.dir.clone(), self.path(key), file_name(key));
        let json = Zeroizing::new(json.as_bytes().to_vec());
        blocking(move || write_atomically(&dir, &target, &name, &json)).await
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        let _guard = self.write_lock.lock().await;
        let (dir, path) = (self.dir.clone(), self.path(key));
        blocking(move || {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e)),
            }
            sync_dir(&dir);
            Ok(())
        })
        .await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, Zeroizing<String>)>, StoreError> {
        let dir = self.dir.clone();
        let prefix = prefix.to_owned();
        blocking(move || {
            let mut files: Vec<String> = std::fs::read_dir(&dir)
                .map_err(io)?
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|f| f.ends_with(".json") && !f.starts_with('.'))
                .collect();
            files.sort();
            let mut out = Vec::new();
            for file in files {
                let Some(key) = key_of(&file) else { continue };
                if !key.starts_with(&prefix) || check_store_key(&key).is_err() {
                    continue;
                }
                match std::fs::read_to_string(dir.join(&file)) {
                    Ok(json) => out.push((key, Zeroizing::new(json))),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io(e)),
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(out)
        })
        .await
    }
}

/// A store handle shared by an engine.
pub type SharedStore = Arc<dyn SwapStore>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn validates_keys() {
        for good in ["taker/abc", "maker/swap/0xab", "maker/quote/00000000-0000-4000-8000-000000000001", "a.b:c_d"] {
            assert!(check_store_key(good).is_ok(), "{good}");
        }
        for bad in ["", "/abs", "a/../b", "a b", "ä", &"x".repeat(201), "a\\b"] {
            assert!(check_store_key(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn memory_store_lists_by_prefix_in_key_order() {
        let s = MemoryStore::new();
        assert!(!s.durable());
        s.put("taker/b", "2").await.unwrap();
        s.put("taker/a", "1").await.unwrap();
        s.put("maker/x", "3").await.unwrap();
        let listed: Vec<String> = s.list("taker/").await.unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(listed, ["taker/a", "taker/b"]);
        s.delete("taker/a").await.unwrap();
        assert!(s.get("taker/a").await.unwrap().is_none());
        assert!(s.put("../x", "1").await.is_err());
    }
}
