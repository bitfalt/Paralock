//! Human and `--json` output.
//!
//! In human mode, progress and results go to stdout and warnings and errors to stderr. With `--json`, stdout carries
//! exactly one JSON document (the result, or `{ "ok": false, "error": … }`), so scripts can parse it; warnings and
//! logs still go to stderr.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex};

use paralock::paralock_core::protocol::SwapEvent;
use paralock::{Swap, SwapStatus};
use serde_json::{Value, json};

use crate::error::CliError;

/// Prints a warning to stderr (in both modes).
pub fn warn(message: &str) {
    let _ = writeln!(std::io::stderr(), "warning: {message}");
}

/// The wire name of a status (`zcash_locked`, …).
pub const fn status_str(status: SwapStatus) -> &'static str {
    match status {
        SwapStatus::Quoted => "quoted",
        SwapStatus::ZcashLocked => "zcash_locked",
        SwapStatus::EvmLocked => "evm_locked",
        SwapStatus::EvmClaimed => "evm_claimed",
        SwapStatus::Completed => "completed",
        SwapStatus::RefundedZcash => "refunded_zcash",
        SwapStatus::RefundedEvm => "refunded_evm",
        SwapStatus::Failed => "failed",
    }
}

/// One timeline line: `  [zcash   ] zcash_locked    Taker funded the Zcash HTLC  <txid>`.
pub fn event_line(event: &SwapEvent) -> String {
    let detail = event.detail.as_deref().unwrap_or("");
    let txid = event.txid.as_deref().map(|t| format!("  {t}")).unwrap_or_default();
    format!("  [{:<8}] {:<15} {detail}{txid}", event.chain.as_str(), status_str(event.status))
}

/// Where output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    /// `--json`.
    pub json: bool,
}

/// A callback that receives swaps as they progress.
pub type Progress = Arc<dyn Fn(&Swap) + Send + Sync>;

impl Output {
    /// A line of human output (nothing with `--json`).
    pub fn say(&self, line: impl AsRef<str>) {
        if !self.json {
            let _ = writeln!(std::io::stdout(), "{}", line.as_ref());
        }
    }

    /// Prints each swap event once, as it happens (human mode only). `prefix` goes before the swap's lines (e.g. a
    /// short swap id for a maker serving many swaps).
    pub fn progress(&self, with_id: bool) -> Progress {
        let json = self.json;
        let printed: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
        Arc::new(move |swap: &Swap| {
            if json {
                return;
            }
            let mut printed = printed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let done = printed.entry(swap.id.clone()).or_insert(0);
            let mut out = std::io::stdout().lock();
            if with_id && *done == 0 && swap.events.is_empty() {
                let _ = writeln!(
                    out,
                    "[{}] new {} swap: {} ZEC → {} {} on {}",
                    short(&swap.id),
                    swap.mode.as_str(),
                    crate::amount::zec(swap.zcash.amount_zat),
                    crate::amount::usd(swap.evm.amount, 6),
                    swap.quote.token_symbol,
                    swap.evm.chain
                );
            }
            for event in swap.events.iter().skip(*done) {
                let line = event_line(event);
                if with_id {
                    let _ = writeln!(out, "[{}]{line}", short(&swap.id));
                } else {
                    let _ = writeln!(out, "{line}");
                }
            }
            *done = swap.events.len();
        })
    }

    /// The result document (`--json` only): `{ "ok": true, …fields }`.
    pub fn result(&self, fields: Value) {
        if !self.json {
            return;
        }
        let mut doc = json!({ "ok": true });
        if let (Some(doc), Value::Object(fields)) = (doc.as_object_mut(), fields) {
            doc.extend(fields);
        }
        print_json(&doc);
    }

    /// Reports an error: `{ "ok": false, "error": { code, message, hint }, "swap"? }` with `--json`, otherwise
    /// `error: …` and `hint: …` on stderr.
    pub fn error(&self, error: &CliError) {
        if self.json {
            let mut doc = json!({
                "ok": false,
                "error": {
                    "code": error.code.map(|c| c.as_str()),
                    "message": error.message,
                    "hint": error.hint,
                    "exitCode": error.exit.code(),
                },
            });
            if let (Some(swap), Some(map)) = (&error.swap, doc.as_object_mut()) {
                map.insert("swap".into(), serde_json::to_value(swap).unwrap_or(Value::Null));
            }
            print_json(&doc);
            return;
        }
        let mut err = std::io::stderr().lock();
        let code = error.code.map(|c| format!(" ({})", c.as_str())).unwrap_or_default();
        let _ = writeln!(err, "error{code}: {}", error.message);
        if let Some(hint) = &error.hint {
            let _ = writeln!(err, "  hint: {hint}");
        }
    }
}

/// Prints a JSON document on stdout as is (for results that are not `ok`).
pub fn print_document(value: &Value) {
    print_json(value);
}

/// One line of newline-delimited JSON on stdout (the maker's `--json` records).
pub fn print_record(value: &Value) {
    let _ = writeln!(std::io::stdout(), "{value}");
}

fn print_json(value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{\"ok\":false}".into());
    let _ = writeln!(std::io::stdout(), "{text}");
}

/// The first 10 characters of a swap id.
pub fn short(id: &str) -> &str {
    id.get(..10).unwrap_or(id)
}
