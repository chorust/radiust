use crate::errors::{CoreError, CoreResult};
use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub size_bytes: u64,
    pub sha256: String,
}

pub fn digest_reader<R: Read>(mut reader: R, limit: u64) -> CoreResult<(Vec<u8>, Receipt)> {
    let mut output = Vec::new();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|e| CoreError::Transport(e.to_string()))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > limit {
            return Err(CoreError::ResourceLimit(format!("payload {size} > {limit}")));
        }
        hasher.update(&buffer[..read]);
        output.extend_from_slice(&buffer[..read]);
    }
    Ok((output, Receipt { size_bytes: size, sha256: hex::encode(hasher.finalize()) }))
}
