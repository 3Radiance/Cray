use anyhow::{Result, bail};
use btls::hash::hmac_sha256;

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

pub fn tag(key: &[u8; 32], msg: &[u8]) -> Result<[u8; 32]> {
    Ok(hmac_sha256(key, msg)?)
}

pub fn verify(key: &[u8; 32], msg: &[u8], tag: &[u8]) -> Result<()> {
    let t = btls::hash::hmac_sha256(key, msg)?;
    anyhow::ensure!(btls::memcmp::eq(&t, tag), "bad tag");
    Ok(())
}
