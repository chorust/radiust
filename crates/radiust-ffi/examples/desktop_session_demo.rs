//! Cloud-friendly demo: open in-process session and fetch a Japan radar PNG.
//!
//!   CXX=g++ cargo run -p radiust-ffi --example desktop_session_demo -- /tmp/cache [copy.png]

use radiust_ffi::DesktopSession;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let cache_override = env::args().nth(1).map(PathBuf::from);
    let copy_to = env::args().nth(2).map(PathBuf::from);
    let cache_arg = cache_override
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    println!("=== Radiust Desktop: fetch Japan radar (in-process) ===");
    println!("daemon: false");

    let mut session = DesktopSession::open_desktop_defaults(cache_arg)
        .expect("open desktop session");
    println!("open snapshot: {}", session.snapshot_json().expect("json"));

    match session.fetch_latest_preview() {
        Ok(preview) => {
            println!("fetched PNG:");
            println!("  source  = {}", preview.source_id);
            println!("  product = {}", preview.product);
            println!("  station = {:?}", preview.station);
            println!("  valid   = {}", preview.valid_time);
            println!("  path    = {}", preview.png_path);
            println!("  mode    = {}", preview.mode);
            if let Some(dest) = copy_to {
                if let Some(parent) = dest.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::copy(&preview.png_path, &dest).expect("copy png");
                println!("copied to {}", dest.display());
            }
        }
        Err(err) => {
            eprintln!("fetch failed: {err}");
            std::process::exit(1);
        }
    }
}
