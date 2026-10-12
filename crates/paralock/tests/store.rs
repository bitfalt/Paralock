//! `FileStore`: durability, atomic replacement, permissions, the exclusive lock, and listing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::path::PathBuf;

use paralock::{FileStore, MemoryStore, StoreError, SwapStore};

fn temp_dir(name: &str) -> PathBuf {
    let mut bytes = [0u8; 8];
    rand_core::RngCore::fill_bytes(&mut paralock_core::OsRng, &mut bytes);
    std::env::temp_dir().join(format!("paralock-store-{name}-{}", paralock_core::encoding::to_hex(&bytes)))
}

#[tokio::test]
async fn round_trips_records_and_survives_reopening() {
    let dir = temp_dir("roundtrip");
    {
        let store = FileStore::open(&dir).unwrap();
        assert!(store.durable());
        store.put("taker/0xab", r#"{"a":1}"#).await.unwrap();
        store.put("maker/swap/0000-1", r#"{"b":2}"#).await.unwrap();
        store.put("maker/quote/x", r#"{"c":3}"#).await.unwrap();
        store.put("taker/0xab", r#"{"a":2}"#).await.unwrap(); // replaced
        assert_eq!(store.get("taker/0xab").await.unwrap().unwrap().as_str(), r#"{"a":2}"#);
        assert!(store.get("taker/none").await.unwrap().is_none());
    }
    // A new process (the lock was released when the first store was dropped).
    let store = FileStore::open(&dir).unwrap();
    let keys: Vec<String> = store.list("maker/").await.unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["maker/quote/x", "maker/swap/0000-1"]);
    assert_eq!(store.list("taker/").await.unwrap().len(), 1);
    store.delete("maker/quote/x").await.unwrap();
    store.delete("maker/quote/x").await.unwrap(); // idempotent
    assert_eq!(store.list("maker/").await.unwrap().len(), 1);
    // The TypeScript layout: one `<key with ~>.json` file per record, no temporary files left behind.
    let mut files: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    files.sort();
    assert_eq!(files, [".lock", "maker~swap~0000-1.json", "taker~0xab.json"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn keeps_other_processes_out() {
    let dir = temp_dir("lock");
    let first = FileStore::open(&dir).unwrap();
    assert!(matches!(FileStore::open(&dir), Err(StoreError::Locked(_))));
    drop(first);
    FileStore::open(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn creates_a_private_directory_and_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("perms");
    let store = FileStore::open(&dir).unwrap();
    store.put("taker/secret", r#"{"secret":"0x…"}"#).await.unwrap();
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    let file = dir.join("taker~secret.json");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    drop(store);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn ignores_stray_and_temporary_files_and_rejects_bad_keys() {
    let dir = temp_dir("stray");
    let store = FileStore::open(&dir).unwrap();
    std::fs::write(dir.join(".taker~x.json.abc.tmp"), "torn").unwrap();
    std::fs::write(dir.join("notes.txt"), "hi").unwrap();
    std::fs::write(dir.join("..json"), "{}").unwrap();
    store.put("taker/a", "{}").await.unwrap();
    let keys: Vec<String> = store.list("").await.unwrap().into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["taker/a"]);
    for bad in ["../escape", "/abs", "a b", "", "taker/../x"] {
        assert!(matches!(store.put(bad, "{}").await, Err(StoreError::InvalidKey(_))), "{bad}");
        assert!(store.get(bad).await.is_err());
    }
    drop(store);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn concurrent_writes_to_one_key_leave_a_whole_record() {
    let dir = temp_dir("concurrent");
    let store = std::sync::Arc::new(FileStore::open(&dir).unwrap());
    let mut tasks = Vec::new();
    for i in 0..32 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.put("taker/k", &format!(r#"{{"n":{i},"pad":"{}"}}"#, "x".repeat(4096))).await
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    let json = store.get("taker/k").await.unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value["n"].as_u64().unwrap() < 32);
    drop(store);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn memory_store_is_not_durable() {
    assert!(!MemoryStore::new().durable());
}
