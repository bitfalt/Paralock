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
use zeroize::Zeroizing;

use crate::records::{RecordError, StoredRecord, decode_record};

/// Where [`SwapStore::quarantine`] moves records that can't be read: `quarantine/<key>` (a `quarantine/` directory in
/// a [`FileStore`]).
pub const QUARANTINE_PREFIX: &str = "quarantine/";

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

    /// Moves the record under `key` aside, out of every listing the engines make, without losing it: the engines
    /// call it for a record they can't read (corrupted, edited by hand, written by an incompatible SDK), so that one
    /// bad record never stops the others from loading. An operator inspects it and restores it by hand.
    ///
    /// The default copies it to `quarantine/<key>` ([`QUARANTINE_PREFIX`]) and then deletes the original; a
    /// [`FileStore`] renames the file into its `quarantine/` directory.
    async fn quarantine(&self, key: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        if let Some(json) = self.get(key).await? {
            self.put(&format!("{QUARANTINE_PREFIX}{key}"), &json).await?;
        }
        self.delete(key).await
    }
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

/// Reads and decodes a typed record (migrating older record versions). A record that doesn't decode is
/// [`StoreError::Corrupt`].
pub(crate) async fn get_record<T: StoredRecord>(store: &dyn SwapStore, key: &str) -> Result<Option<T>, StoreError> {
    match store.get(key).await? {
        None => Ok(None),
        Some(json) => decode_record::<T>(&json)
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

/// A store key with the swap id shortened, for logs (swap ids link the two legs of a swap).
pub(crate) fn short_key(key: &str) -> String {
    let cut = key.rfind('/').map_or(0, |i| i + 1);
    let id = &key[cut..];
    format!("{}{}", &key[..cut], id.get(..10).unwrap_or(id))
}

/// Lists and decodes typed records, one by one: a record that can't be read never stops the others from loading.
///
/// - A record written by a newer version of the SDK (a higher `version`) is skipped and left in place, with an error
///   log: upgrade to handle it.
/// - Any other record that doesn't decode is moved aside with [`SwapStore::quarantine`], with an error log.
///
/// Only a failure of the listing itself is an error.
pub(crate) async fn load_records<T: StoredRecord>(
    store: &dyn SwapStore,
    prefix: &str,
) -> Result<Vec<(String, T)>, StoreError> {
    let mut out = Vec::new();
    for (key, json) in store.list(prefix).await? {
        match decode_record::<T>(&json) {
            Ok(record) => out.push((key, record)),
            Err(RecordError::Newer { version }) => tracing::error!(
                key = %short_key(&key),
                version,
                supported = T::VERSION,
                "a {} record was written by a newer version of paralock; skipping it (upgrade to handle it)",
                T::WHAT
            ),
            Err(RecordError::Invalid(message)) => match store.quarantine(&key).await {
                Ok(()) => tracing::error!(
                    key = %short_key(&key),
                    error = %message,
                    "an unreadable {} record was moved to quarantine/; inspect it and restore it by hand",
                    T::WHAT
                ),
                Err(e) => tracing::error!(
                    key = %short_key(&key),
                    error = %message,
                    quarantine_error = %e,
                    "an unreadable {} record could not be moved to quarantine/; skipping it",
                    T::WHAT
                ),
            },
        }
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
/// - The directory is created with mode `0700` and files with `0600` (Unix). [`FileStore::open`] warns about an
///   existing directory that other users can access; [`FileStore::open_private`] (what the builders use on mainnet)
///   restricts it to `0700`, or fails if it can't.
/// - Each `put` writes a temporary file, fsyncs it, renames it over the old one and fsyncs the directory, so a record
///   is either the old version or the new one, never a torn write. A failed directory fsync is an error. Temporary
///   files left by a crash are removed when the store is opened.
/// - Writes are serialized in-process, and an exclusive lock on `<dir>/.lock` keeps a second process (a second maker
///   or taker on the same directory) out.
/// - [`SwapStore::quarantine`] moves a record's file into `<dir>/quarantine/`.
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
    /// Opens (creating it if needed) the store directory and takes its exclusive lock. An existing directory that
    /// other users can access is reported with a warning (see [`FileStore::open_private`]).
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with(dir.into(), false)
    }

    /// [`FileStore::open`], but an existing directory that other users can access is restricted to `0700` (Unix), and
    /// the store fails to open if that isn't possible (e.g. the directory belongs to another user). The builders use
    /// it on mainnet: the records hold private keys and swap secrets.
    pub fn open_private(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        Self::open_with(dir.into(), true)
    }

    fn open_with(dir: PathBuf, strict: bool) -> Result<Self, StoreError> {
        create_private_dir(&dir, strict)?;
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
        // We hold the lock: any temporary file is a leftover of a write that crashed before its rename.
        sweep_temporary_files(&dir)?;
        Ok(Self { dir, write_lock: tokio::sync::Mutex::new(()), _lock: lock })
    }

    /// The store directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A record's file: `<dir>/<key with ~>.json`, and `<dir>/quarantine/<rest with ~>.json` for
    /// `quarantine/<rest>`.
    fn path(&self, key: &str) -> PathBuf {
        match key.strip_prefix(QUARANTINE_PREFIX) {
            Some(rest) => self.dir.join(QUARANTINE_DIR).join(file_name(rest)),
            None => self.dir.join(file_name(key)),
        }
    }
}

/// The subdirectory of a [`FileStore`] that holds quarantined records.
const QUARANTINE_DIR: &str = "quarantine";

fn create_private_dir(dir: &Path, strict: bool) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(io)?;
        }
        let mode = std::fs::metadata(dir).map_err(io)?.permissions().mode();
        if mode & 0o077 != 0 {
            if strict {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
                    StoreError::Io(format!(
                        "the swap store directory {} is accessible by other users and can't be restricted to 0700: {e}",
                        dir.display()
                    ))
                })?;
                tracing::warn!(
                    dir = %dir.display(),
                    "the swap store directory was accessible by other users; restricted it to 0700"
                );
            } else {
                tracing::warn!(
                    dir = %dir.display(),
                    "the swap store directory is accessible by other users; it holds private keys (chmod 700 it)"
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = strict;
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    Ok(())
}

/// Removes `.<name>.<random>.tmp` files: writes interrupted before their rename (the store's lock is held).
fn sweep_temporary_files(dir: &Path) -> Result<(), StoreError> {
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().into_string() else { continue };
        if name.starts_with('.') && name.ends_with(".tmp") && entry.file_type().is_ok_and(|t| t.is_file()) {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => tracing::debug!(file = %name, "removed a temporary file left by an interrupted write"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e)),
            }
        }
    }
    Ok(())
}

/// Persists renames and removals in `dir`. A failure is an error (the caller can't know the change is durable), except
/// on filesystems that don't support syncing a directory at all, where the rename itself is still atomic.
fn sync_dir(dir: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        let handle = std::fs::File::open(dir).map_err(io)?;
        match handle.sync_all() {
            Ok(()) => Ok(()),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::InvalidInput | std::io::ErrorKind::Unsupported) => Ok(()),
            Err(e) => Err(io(e)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
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
    sync_dir(dir)
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
        let target = self.path(key);
        let dir = target.parent().map_or_else(|| self.dir.clone(), Path::to_path_buf);
        let name = target.file_name().and_then(|n| n.to_str()).map(str::to_owned).unwrap_or_else(|| file_name(key));
        let json = Zeroizing::new(json.as_bytes().to_vec());
        let quarantine = key.starts_with(QUARANTINE_PREFIX);
        blocking(move || {
            if quarantine {
                create_private_dir(&dir, true)?;
            }
            write_atomically(&dir, &target, &name, &json)
        })
        .await
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        let _guard = self.write_lock.lock().await;
        let path = self.path(key);
        let dir = path.parent().map_or_else(|| self.dir.clone(), Path::to_path_buf);
        blocking(move || {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e)),
            }
            sync_dir(&dir)
        })
        .await
    }

    async fn quarantine(&self, key: &str) -> Result<(), StoreError> {
        check_store_key(key)?;
        let _guard = self.write_lock.lock().await;
        let (dir, name) = (self.dir.clone(), file_name(key));
        blocking(move || {
            let from = dir.join(&name);
            if !from.exists() {
                return Ok(());
            }
            let quarantine = dir.join(QUARANTINE_DIR);
            create_private_dir(&quarantine, true)?;
            let mut to = quarantine.join(&name);
            if to.exists() {
                // Keep every quarantined version.
                let mut suffix = [0u8; 4];
                rand_core::RngCore::fill_bytes(&mut paralock_core::OsRng, &mut suffix);
                to = quarantine.join(format!("{name}.{}", paralock_core::encoding::to_hex(&suffix)));
            }
            std::fs::rename(&from, &to).map_err(io)?;
            sync_dir(&quarantine)?;
            sync_dir(&dir)
        })
        .await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<(String, Zeroizing<String>)>, StoreError> {
        // `quarantine/…` keys live in the quarantine directory; any other prefix lists the store's own records.
        let (dir, prefix, mapped) = match prefix.strip_prefix(QUARANTINE_PREFIX) {
            Some(rest) => (self.dir.join(QUARANTINE_DIR), rest.to_owned(), true),
            None => (self.dir.clone(), prefix.to_owned(), false),
        };
        blocking(move || {
            if mapped && !dir.exists() {
                return Ok(Vec::new());
            }
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
                let key = if mapped { format!("{QUARANTINE_PREFIX}{key}") } else { key };
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
