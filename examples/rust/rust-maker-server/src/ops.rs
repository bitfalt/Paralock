//! Your own routes next to the protocol: liveness, readiness and metrics, computed from the maker's public views
//! ([`Maker::list_swaps`], [`Maker::unfinished_swaps`], [`Maker::stage`], [`Maker::limits`], [`Maker::policy`]).
//! Nothing here exposes a swap id, a key or an address; serve these routes on an internal network anyway.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use paralock::paralock_core::evm::EvmAddress;
use paralock::{EvmChain, EvmClient, Maker, SwapStatus, ZcashNode};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::warn;

/// State of the ops routes.
#[derive(Clone)]
pub struct Ops {
    /// The maker (a cheap handle).
    pub maker: Maker,
    /// The Zcash node the maker uses.
    pub node: Arc<dyn ZcashNode>,
    /// One read-only client per chain, for the stablecoin inventory.
    pub inventory: Arc<Vec<Inventory>>,
    /// Process start.
    pub started: Instant,
    /// The last snapshot: `list_swaps` reads the whole store, so scrapes reuse it for [`SNAPSHOT_TTL`].
    pub cache: Arc<Mutex<Option<Cached>>>,
}

/// A snapshot and when it was taken.
pub type Cached = (Instant, Arc<Snapshot>);

/// How long a snapshot is reused.
pub const SNAPSHOT_TTL: Duration = Duration::from_secs(10);

/// The maker's stablecoin on one chain.
pub struct Inventory {
    /// The chain.
    pub chain: EvmChain,
    /// The token's symbol.
    pub symbol: String,
    /// The token.
    pub token: EvmAddress,
    /// A client signing with the maker's account (only used to read its balance).
    pub client: Arc<dyn EvmClient>,
}

impl Ops {
    /// Balances of the maker's EVM account, per chain (`None` when the RPC doesn't answer).
    async fn balances(&self) -> Vec<(EvmChain, String, Option<u128>)> {
        let mut out = Vec::new();
        for i in self.inventory.iter() {
            let balance = tokio::time::timeout(Duration::from_secs(3), i.client.token_balance(&i.token, None)).await;
            let units = match balance {
                Ok(Ok(units)) => units.to_u128(),
                _ => None,
            };
            out.push((i.chain, i.symbol.clone(), units));
        }
        out
    }

    /// Logs how to fund the maker if it has no stablecoins to lock (it would refuse every quote).
    pub async fn warn_if_empty(&self) {
        for (chain, symbol, units) in self.balances().await {
            if units == Some(0) {
                let address = self.inventory.iter().find(|i| i.chain == chain).map(|i| i.client.address());
                warn!(%chain, %symbol, account = ?address, "the maker holds no {symbol} on {chain}: quotes there fail with insufficient_liquidity until you fund it (see the README)");
            }
        }
    }

    /// Aggregates the maker's public swap views (cached for [`SNAPSHOT_TTL`]; concurrent scrapes wait for one read).
    async fn snapshot(&self) -> Arc<Snapshot> {
        let mut cache = self.cache.lock().await;
        if let Some((at, snapshot)) = cache.as_ref()
            && at.elapsed() < SNAPSHOT_TTL
        {
            return Arc::clone(snapshot);
        }
        let mut s = Snapshot::default();
        // Every swap in the store, finished ones included: statuses, modes and completed volume.
        match self.maker.list_swaps().await {
            Ok(swaps) => {
                for swap in swaps {
                    *s.by_status.entry(status_name(swap.status)).or_default() += 1;
                    *s.by_mode.entry(swap.mode.as_str()).or_default() += 1;
                    if swap.status == SwapStatus::Completed {
                        let v = s.volume.entry((swap.evm.chain, swap.quote.token_symbol.clone())).or_default();
                        v.swaps += 1;
                        v.zat += u128::from(swap.zcash.amount_zat);
                        v.units += swap.evm.amount;
                    }
                }
            }
            Err(e) => warn!(error = %e, "can't list the maker's swaps"),
        }
        // Swaps in progress, by internal stage (from memory).
        let unfinished = self.maker.unfinished_swaps();
        s.in_flight = unfinished.len() as u64;
        for id in &unfinished {
            if let Some(stage) = self.maker.stage(id) {
                *s.by_stage.entry(stage.as_str()).or_default() += 1;
            }
        }
        s.inventory = self.balances().await;
        s.tip =
            tokio::time::timeout(Duration::from_secs(3), self.node.get_block_count()).await.ok().and_then(Result::ok);
        let snapshot = Arc::new(s);
        *cache = Some((Instant::now(), Arc::clone(&snapshot)));
        snapshot
    }
}

/// Completed swaps on one chain.
#[derive(Default)]
struct Volume {
    swaps: u64,
    zat: u128,
    units: u128,
}

/// Aggregated numbers of one scrape.
#[derive(Default)]
pub struct Snapshot {
    by_status: BTreeMap<String, u64>,
    by_stage: BTreeMap<&'static str, u64>,
    by_mode: BTreeMap<&'static str, u64>,
    in_flight: u64,
    volume: BTreeMap<(EvmChain, String), Volume>,
    inventory: Vec<(EvmChain, String, Option<u128>)>,
    tip: Option<u32>,
}

/// A status's wire name (`zcash_locked`, `completed`, …).
fn status_name(status: SwapStatus) -> String {
    serde_json::to_value(status).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// `GET /healthz`: the process is up.
pub async fn healthz(State(ops): State<Ops>) -> Json<Value> {
    Json(json!({ "ok": true, "uptimeSeconds": ops.started.elapsed().as_secs() }))
}

/// `GET /readyz`: the maker can work (its Zcash node answers). Load balancers route quotes only to ready makers.
pub async fn readyz(State(ops): State<Ops>) -> Response {
    let tip = tokio::time::timeout(Duration::from_secs(3), ops.node.get_block_count()).await;
    match tip {
        Ok(Ok(tip)) => Json(json!({ "ready": true, "zcashTip": tip })).into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ready": false, "reason": "the Zcash node is unavailable" })),
        )
            .into_response(),
    }
}

/// `GET /stats`: swaps by status, stage and mode, completed volume, inventory, limits and policy, as JSON.
pub async fn stats(State(ops): State<Ops>) -> Json<Value> {
    let s = ops.snapshot().await;
    let (limits, policy) = (ops.maker.limits(), ops.maker.policy());
    let volume: Vec<Value> = s
        .volume
        .iter()
        .map(|((chain, token), v)| {
            json!({ "chain": chain, "token": token, "swaps": v.swaps, "zecZat": v.zat.to_string(), "tokenUnits": v.units.to_string() })
        })
        .collect();
    let inventory: Vec<Value> = s
        .inventory
        .iter()
        .map(|(chain, token, units)| json!({ "chain": chain, "token": token, "units": units.map(|u| u.to_string()) }))
        .collect();
    Json(json!({
        "uptimeSeconds": ops.started.elapsed().as_secs(),
        "zcashPubkey": ops.maker.zcash_pubkey(),
        "zcashTip": s.tip,
        "chains": ops.maker.chains(),
        "swaps": {
            "inFlight": s.in_flight,
            "unfinishedByStage": s.by_stage,
            "capacity": limits.max_concurrent_swaps,
            "byStatus": s.by_status,
            "byMode": s.by_mode,
        },
        "completedVolume": volume,
        "inventory": inventory,
        "limits": { "minZecAmountZat": limits.min_zec_amount_zat, "maxZecAmountZat": limits.max_zec_amount_zat, "quoteTtlSeconds": limits.quote_ttl_seconds },
        "policy": { "zcashLockBlocks": policy.zcash_lock_blocks(), "evmLockSeconds": policy.evm_lock_seconds(), "safetyFactor": policy.safety_factor() },
    }))
}

/// `GET /metrics`: the same numbers in the Prometheus text format.
pub async fn metrics(State(ops): State<Ops>) -> Response {
    let s = ops.snapshot().await;
    let mut out = String::new();
    let mut gauge = |name: &str, help: &str, samples: Vec<(String, String)>| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
        for (labels, value) in samples {
            let _ = writeln!(out, "{name}{labels} {value}");
        }
    };
    gauge("paralock_maker_up", "1 while the process runs.", vec![(String::new(), "1".into())]);
    gauge(
        "paralock_maker_uptime_seconds",
        "Seconds since the process started.",
        vec![(String::new(), ops.started.elapsed().as_secs().to_string())],
    );
    gauge(
        "paralock_maker_zcash_tip",
        "Height of the Zcash node's tip (absent if it doesn't answer).",
        s.tip.map(|t| (String::new(), t.to_string())).into_iter().collect(),
    );
    gauge(
        "paralock_maker_swaps",
        "Swaps in the maker's store, by public status.",
        s.by_status.iter().map(|(status, n)| (format!("{{status=\"{status}\"}}"), n.to_string())).collect(),
    );
    gauge(
        "paralock_maker_swaps_by_stage",
        "Unfinished swaps by internal stage.",
        s.by_stage.iter().map(|(stage, n)| (format!("{{stage=\"{stage}\"}}"), n.to_string())).collect(),
    );
    gauge("paralock_maker_swaps_in_flight", "Swaps not finished yet.", vec![(String::new(), s.in_flight.to_string())]);
    gauge(
        "paralock_maker_swap_capacity",
        "Swaps the maker takes at once (limits.max_concurrent_swaps).",
        vec![(String::new(), ops.maker.limits().max_concurrent_swaps.to_string())],
    );
    gauge(
        "paralock_maker_completed_zat",
        "ZEC received in completed swaps, in zatoshi.",
        s.volume.iter().map(|((chain, _), v)| (format!("{{chain=\"{chain}\"}}"), v.zat.to_string())).collect(),
    );
    gauge(
        "paralock_maker_completed_token_units",
        "Stablecoins paid in completed swaps, in base units (6 decimals).",
        s.volume
            .iter()
            .map(|((chain, token), v)| (format!("{{chain=\"{chain}\",token=\"{token}\"}}"), v.units.to_string()))
            .collect(),
    );
    gauge(
        "paralock_maker_inventory_token_units",
        "Stablecoins the maker's EVM account holds, in base units.",
        s.inventory
            .iter()
            .filter_map(|(chain, token, units)| {
                units.map(|u| (format!("{{chain=\"{chain}\",token=\"{token}\"}}"), u.to_string()))
            })
            .collect(),
    );
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], out).into_response()
}
