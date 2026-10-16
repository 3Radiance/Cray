use anyhow::{Result, bail};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn parse_key(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    let raw = hex::decode(s).map_err(|_| anyhow::anyhow!("key: not valid hex"))?;
    if raw.len() != 32 {
        bail!("key: need 32 bytes (64 hex chars), got {} bytes", raw.len());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&raw);
    Ok(key)
}

pub fn tag(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("32-byte key always fits");
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

pub fn verify(key: &[u8; 32], msg: &[u8], tag: &[u8]) -> Result<()> {
    let mut mac = HmacSha256::new_from_slice(key).expect("32-byte key always fits");
    mac.update(msg);
    mac.verify_slice(tag)
        .map_err(|_| anyhow::anyhow!("bad tag"))
}
