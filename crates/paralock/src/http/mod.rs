//! The maker HTTP API: the axum server (feature `server`) and the reqwest client (feature `client`).

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod server;

#[cfg(feature = "client")]
pub use client::{
    HttpMakerApi, HttpMakerApiOptions, MakerHealth, parse_accepted_response, parse_health, parse_quote_response,
    parse_swap_response,
};
#[cfg(feature = "server")]
pub use server::{
    ClientAddress, CorsOrigins, DEFAULT_RATE_LIMIT, RateLimit, ServerOptions, maker_router, serve, serve_with_shutdown,
};
