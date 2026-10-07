pub mod auth;
pub mod config;
pub mod fingerprint;
pub mod inbound;
pub mod outbound;
pub mod protocol;
pub mod transport;

pub struct ConnectionData {
    pub addr: String,
    pub port: u16,
}
