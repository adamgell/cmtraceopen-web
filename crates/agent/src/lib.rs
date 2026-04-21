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

#![forbid(unsafe_code)]

pub mod config;

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
