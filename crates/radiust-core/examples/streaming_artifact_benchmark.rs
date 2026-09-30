//! Loopback benchmark helper for bounded HTTP download and object upload.
//!
//! This example is driven by `scripts/validation/benchmark_large_artifact.py`.

use radiust_core::limits::Limits;
use radiust_core::storage::object::ObjectStore;
use radiust_core::transport::http::HttpTransport;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let base_url = required(&mut args, "base URL")?;
    let frames: usize = required(&mut args, "frame count")?.parse()?;
    let body_bytes: u64 = required(&mut args, "artifact size")?.parse()?;
    let temp_root = PathBuf::from(required(&mut args, "temporary directory")?);
    let sink_root = PathBuf::from(required(&mut args, "object sink directory")?);
    if args.next().is_some() || frames == 0 || body_bytes == 0 {
        return Err("invalid benchmark arguments".into());
    }
    std::fs::create_dir_all(&temp_root)?;
    std::fs::create_dir_all(&sink_root)?;
    let sink_root_text = sink_root.to_string_lossy();
    let operator = opendal::Operator::new(opendal::services::Fs::default().root(&sink_root_text))?;
    let store = Arc::new(ObjectStore::from_operator(operator, "generation")?);
    let limits = Limits {
        request_concurrency: frames,
        host_concurrency: frames,
        max_artifact_bytes: body_bytes,
        max_frame_bytes: body_bytes,
        max_temp_bytes: body_bytes.saturating_mul(frames as u64),
        request_timeout_secs: 120,
        ..Limits::default()
    };
    let transport = Arc::new(HttpTransport::new(limits.clone(), false)?);
    let observed_temp_peak = Arc::new(AtomicU64::new(0));
    let mut jobs = JoinSet::new();

    for index in 0..frames {
        let address = format!("{base_url}/artifact/{index}");
        let path = temp_root.join(format!("artifact-{index}.part"));
        let transport = transport.clone();
        let store = store.clone();
        let observed_temp_peak = observed_temp_peak.clone();
        let temp_root = temp_root.clone();
        let size_cap = limits.max_artifact_bytes;
        jobs.spawn(async move {
            let downloaded = transport
                .get_to_path_with_headers(&address, &[], &path)
                .await
                .map_err(|error| error.to_string())?;
            let expected = repeated_byte_digest((index as u8).wrapping_add(17), body_bytes);
            if downloaded.size_bytes != body_bytes || downloaded.sha256 != expected {
                return Err("download receipt differs from the fixture payload".to_owned());
            }
            observed_temp_peak.fetch_max(directory_file_bytes(&temp_root), Ordering::SeqCst);
            let written = store
                .write_path_cancellable(
                    &format!("artifact-{index}.bin"),
                    &path,
                    size_cap,
                    &CancellationToken::new(),
                )
                .await
                .map_err(|error| error.to_string())?;
            if written.size_bytes != body_bytes || written.sha256 != expected {
                return Err("object write receipt differs from the fixture payload".to_owned());
            }
            tokio::fs::remove_file(&path).await.map_err(|error| error.to_string())?;
            Ok::<_, String>((index, expected))
        });
    }

    let mut fingerprints = vec![String::new(); frames];
    while let Some(joined) = jobs.join_next().await {
        let (index, digest) = joined
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .map_err(std::io::Error::other)?;
        fingerprints[index] = digest;
    }
    if std::fs::read_dir(&temp_root)?.next().is_some() {
        return Err("temporary directory contains residual entries".into());
    }

    for (index, expected) in fingerprints.iter().enumerate() {
        let object_path = sink_root.join("generation").join(format!("artifact-{index}.bin"));
        if file_sha256(&object_path)? != *expected {
            return Err("object sink readback checksum differs".into());
        }
    }
    println!(
        "{{\"frames\":{frames},\"bytes_per_artifact\":{body_bytes},\"integrity_ok\":true,\"temporary_entries\":0,\"worker_peak_temporary_bytes\":{},\"fingerprints\":[{}]}}",
        observed_temp_peak.load(Ordering::SeqCst),
        fingerprints.iter().map(|value| format!("\"{value}\"")).collect::<Vec<_>>().join(",")
    );
    Ok(())
}

fn directory_file_bytes(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

fn required(
    args: &mut impl Iterator<Item = String>,
    label: &str,
) -> Result<String, Box<dyn Error>> {
    args.next().ok_or_else(|| format!("missing {label}").into())
}

fn repeated_byte_digest(byte: u8, length: u64) -> String {
    let block = vec![byte; 64 * 1024];
    let mut remaining = length;
    let mut digest = Sha256::new();
    while remaining > 0 {
        let take = remaining.min(block.len() as u64) as usize;
        digest.update(&block[..take]);
        remaining -= take as u64;
    }
    hex::encode(digest.finalize())
}

fn file_sha256(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut digest = Sha256::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}
