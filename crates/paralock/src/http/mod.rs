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
pub use server::{CorsOrigins, ServerOptions, maker_router, serve};
