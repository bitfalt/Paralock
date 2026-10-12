//! USD/ZEC prices: what the maker quotes from and what the taker checks quotes against.

use core::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use crate::Result;

/// A USD-per-ZEC price.
#[async_trait]
pub trait PriceSource: Send + Sync {
    /// The current price (USD per ZEC, finite and positive). Fails with `unavailable` when the source is down.
    async fn usd_per_zec(&self) -> Result<f64>;
}

/// A fixed price. Only for devnets, tests and demos: a fixed price in production lets anyone trade against you at a
/// stale rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedPrice(pub f64);

#[async_trait]
impl PriceSource for FixedPrice {
    async fn usd_per_zec(&self) -> Result<f64> {
        Ok(self.0)
    }
}

type PriceFuture = Pin<Box<dyn Future<Output = Result<f64>> + Send>>;

/// A price from an async closure (e.g. your own oracle).
pub struct PriceFn(Arc<dyn Fn() -> PriceFuture + Send + Sync>);

impl PriceFn {
    /// Wraps `f`.
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<f64>> + Send + 'static,
    {
        Self(Arc::new(move || Box::pin(f())))
    }
}

impl fmt::Debug for PriceFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PriceFn")
    }
}

#[async_trait]
impl PriceSource for PriceFn {
    async fn usd_per_zec(&self) -> Result<f64> {
        (self.0)().await
    }
}

#[cfg(feature = "price-feed")]
pub(crate) fn unavailable() -> crate::Error {
    crate::Error::protocol(paralock_core::protocol::ParalockErrorCode::Unavailable, "ZEC price feed unavailable")
}

#[cfg(feature = "price-feed")]
pub use feed::{CoinGecko, CoinGeckoOptions};

#[cfg(feature = "price-feed")]
mod feed {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use async_trait::async_trait;

    use super::{PriceSource, unavailable};
    use crate::Result;

    /// CoinGecko's simple price API for ZEC in USD.
    pub const COINGECKO_URL: &str = "https://api.coingecko.com/api/v3/simple/price?ids=zcash&vs_currencies=usd";

    /// Options of [`CoinGecko`].
    #[derive(Debug, Clone, PartialEq)]
    pub struct CoinGeckoOptions {
        /// Price endpoint returning `{ "zcash": { "usd": <number> } }`. Default: CoinGecko's simple price API.
        pub url: String,
        /// Reuse a fetched price for this long. Default 60 s.
        pub cache: Duration,
        /// When the feed fails, keep serving the last good price for at most this long. Default 5 min.
        pub max_stale: Duration,
        /// Request timeout. Default 3 s.
        pub timeout: Duration,
        /// Price to use when the feed is down and no recent price is cached. Only for devnets and demos.
        pub fallback: Option<f64>,
        /// The `User-Agent` header (the API refuses requests without one). Default [`crate::DEFAULT_USER_AGENT`].
        pub user_agent: String,
    }

    impl Default for CoinGeckoOptions {
        fn default() -> Self {
            Self {
                url: COINGECKO_URL.to_owned(),
                cache: Duration::from_secs(60),
                max_stale: Duration::from_secs(300),
                timeout: Duration::from_secs(3),
                fallback: None,
                user_agent: crate::DEFAULT_USER_AGENT.to_owned(),
            }
        }
    }

    /// USD per ZEC from CoinGecko (or any endpoint with the same JSON shape), cached, with a bounded-staleness
    /// fallback (the TypeScript `zecUsdPrice`). Without a fresh enough price it fails with `unavailable`, so a maker
    /// stops quoting rather than quoting a stale rate.
    #[derive(Debug)]
    pub struct CoinGecko {
        options: CoinGeckoOptions,
        client: reqwest::Client,
        last: Mutex<Option<(f64, Instant)>>,
    }

    impl CoinGecko {
        /// The feed with default options.
        pub fn new() -> crate::Result<Self> {
            Self::with_options(CoinGeckoOptions::default())
        }

        /// The feed with explicit options.
        pub fn with_options(options: CoinGeckoOptions) -> crate::Result<Self> {
            let client = crate::runtime::http_client_builder(&options.user_agent)
                .timeout(options.timeout)
                .build()
                .map_err(|e| crate::Error::Internal(format!("HTTP client: {}", e.without_url())))?;
            Ok(Self { options, client, last: Mutex::new(None) })
        }

        fn cached(&self, within: Duration) -> Option<f64> {
            let last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            last.filter(|(_, at)| at.elapsed() < within).map(|(price, _)| price)
        }

        async fn fetch(&self) -> Option<f64> {
            let response = match self.client.get(&self.options.url).send().await {
                Ok(response) => response,
                Err(e) => {
                    tracing::warn!(error = %e.without_url(), "the ZEC price feed is unreachable");
                    return None;
                }
            };
            if !response.status().is_success() {
                tracing::warn!(status = response.status().as_u16(), "the ZEC price feed refused the request");
                return None;
            }
            // Read at most 64 KiB, chunk by chunk: a hostile or broken endpoint can't make us buffer a huge body.
            const MAX: usize = 64 * 1024;
            if response.content_length().is_some_and(|n| n > MAX as u64) {
                return None;
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len() + chunk.len() > MAX {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            value.get("zcash")?.get("usd")?.as_f64().filter(|p| p.is_finite() && *p > 0.0)
        }
    }

    #[async_trait]
    impl PriceSource for CoinGecko {
        async fn usd_per_zec(&self) -> Result<f64> {
            if let Some(price) = self.cached(self.options.cache) {
                return Ok(price);
            }
            if let Some(price) = self.fetch().await {
                *self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((price, Instant::now()));
                return Ok(price);
            }
            if let Some(price) = self.cached(self.options.max_stale) {
                tracing::warn!("the ZEC price feed is down; using the last price");
                return Ok(price);
            }
            match self.options.fallback {
                Some(price) if price.is_finite() && price > 0.0 => Ok(price),
                _ => Err(unavailable()),
            }
        }
    }
}
