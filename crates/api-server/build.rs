//! Build-time metadata capture.
//!
//! Captures the compiler version (as reported by `rustc -V`) and exposes it
//! to the crate as the `RUSTC_VERSION` env var via `env!()`. Surfaced on the
//! dev status page — purely informational, no behavior branches on it.

use std::process::Command;

fn main() {
    let rustc_version = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=RUSTC_VERSION={rustc_version}");
    // Re-run whenever the compiler changes underneath us (best effort).
    println!("cargo:rerun-if-env-changed=RUSTC");
}
