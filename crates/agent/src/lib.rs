// cmtraceopen-agent library root.
//
// Houses the pieces of the Windows endpoint agent that are worth exercising
// from unit / integration tests without spinning up the real service. The
// `cmtraceopen-agent` binary in `src/main.rs` is a thin runtime wrapper
// around this library.
//
// Scaffold status: only the config module lives here today. Future modules
// (collectors, upload queue, state DB, Windows service glue) will plug in
// alongside it as the agent work actually starts — see the TODO block in
// `main.rs` for the ordered list.

// `deny` rather than `forbid`: the Windows service dispatcher work (landing
// next) needs `windows_service::define_windows_service!` plus FFI shims that
// wrap unsafe blocks, and `forbid` has no local `#[allow]` escape hatch. We
// still want the default-off posture, just with an opt-in for the handful of
// modules that genuinely need it.
#![deny(unsafe_code)]

pub mod collectors;
pub mod config;
pub mod queue;
pub mod tls;
pub mod uploader;

/// Human-readable banner string, emitted at startup and handy in tests.
pub fn banner() -> String {
    format!("cmtraceopen-agent v{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_includes_version() {
        let b = banner();
        assert!(b.starts_with("cmtraceopen-agent v"));
        assert!(b.contains(env!("CARGO_PKG_VERSION")));
    }
}
