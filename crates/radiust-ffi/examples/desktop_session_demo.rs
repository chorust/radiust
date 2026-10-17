//! Cloud-friendly demo of the in-process Desktop session (no SwiftUI, no daemon).
//!
//! Run:
//!   CXX=g++ cargo run -p radiust-ffi --example desktop_session_demo
//!
//! This is not Desktop product acceptance — only shows session policy wiring.

use radiust_ffi::{DesktopSession, LocationMode};
use std::env;
use std::path::PathBuf;

fn main() {
    let cache_override = env::args().nth(1).map(PathBuf::from);
    let cache_arg = cache_override
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    println!("=== Radiust Desktop Session Demo (in-process) ===");
    println!("daemon: false (locked)");
    println!("shell: SwiftUI (this demo is Rust-only on Linux CI)");
    println!();

    let mut session = DesktopSession::open_desktop_defaults(cache_arg)
        .expect("open desktop session");

    println!("--- open ---");
    println!("{}", session.snapshot_json().expect("snapshot"));
    println!();

    session
        .set_location_mode(LocationMode::Station);
    session.set_manual_selection(Some("jp".into()), Some("JPMAKI".into()));
    println!("--- after station pick (jp / JPMAKI) ---");
    println!("{}", session.snapshot_json().expect("snapshot"));
    println!();

    session
        .set_allow_network(false)
        .expect("disable network");
    println!("--- after disable outbound ---");
    println!("{}", session.snapshot_json().expect("snapshot"));
    println!();

    println!("cache_dir = {}", session.cache_dir().display());
    println!("default_source_id = {}", session.default_source_id());
    println!("closing session (Engine dies with process)...");
    drop(session);
    println!("done. no daemon left behind.");
}
