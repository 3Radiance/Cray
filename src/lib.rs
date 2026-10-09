pub mod auth;
pub mod config;
pub mod fingerprint;
pub mod inbound;
pub mod logging;
pub mod outbound;
pub mod protocol;
pub mod transport;

pub struct ConnectRequest {
    pub addr: String,
    pub port: u16,
}
