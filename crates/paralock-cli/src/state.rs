//! Where the local state lives (the rule of the TypeScript SDK's `state-dir.ts`), and the user's home directory.

use std::path::{Path, PathBuf};

/// Both exist at the root of a Paralock checkout, never in an install of the CLI.
const CHECKOUT_MARKERS: [&str; 2] = ["pnpm-workspace.yaml", "contracts/src/ParalockHTLC.sol"];

/// The root of the Paralock checkout containing `start` (the nearest directory, `start` included, that holds both
/// `pnpm-workspace.yaml` and `contracts/src/ParalockHTLC.sol`), if any.
pub fn find_checkout(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| CHECKOUT_MARKERS.iter().all(|marker| dir.join(marker).exists())).map(Path::to_path_buf)
}

/// The user's home directory (`$HOME`, or `%USERPROFILE%` on Windows).
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where Paralock keeps the local devnet's state (`devnet.json`, swap stores, keys):
///
/// 1. inside a Paralock checkout: `<checkout>/.paralock`, whatever `PARALOCK_HOME` says;
/// 2. otherwise `$PARALOCK_HOME` (relative to `cwd`), when set and not empty;
/// 3. otherwise `~/.paralock`.
pub fn state_dir(cwd: &Path, paralock_home: Option<&str>) -> PathBuf {
    if let Some(checkout) = find_checkout(cwd) {
        return checkout.join(".paralock");
    }
    match paralock_home.map(str::trim).filter(|h| !h.is_empty()) {
        Some(home) => cwd.join(home),
        None => home_dir().unwrap_or_else(|| cwd.to_path_buf()).join(".paralock"),
    }
}

/// [`state_dir`] for this process (current directory and environment).
pub fn current_state_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    state_dir(&cwd, std::env::var("PARALOCK_HOME").ok().as_deref())
}

/// Expands a leading `~/` and resolves a relative path against `base`.
pub fn resolve_path(path: &str, base: &Path) -> PathBuf {
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => home_dir().map_or_else(|| PathBuf::from(path), |home| home.join(rest)),
        None if path == "~" => home_dir().unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    };
    if expanded.is_absolute() { expanded } else { base.join(expanded) }
}
