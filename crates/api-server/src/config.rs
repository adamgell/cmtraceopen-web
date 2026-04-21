use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Runtime configuration, populated from environment variables.
///
/// All variables use the `CMTRACE_` prefix so they're easy to spot in a
/// `docker-compose` env block or a systemd unit.
#[derive(Debug, Clone)]
pub struct Config {
    /// Socket address to bind the HTTP listener to. Env: `CMTRACE_LISTEN_ADDR`.
    /// Default: `0.0.0.0:8080`.
    pub listen_addr: SocketAddr,

    /// Root directory for blob staging + finalized blobs. Env:
    /// `CMTRACE_DATA_DIR`. Default: `./data`.
    pub data_dir: PathBuf,

    /// SQLite DB path (file or `:memory:`). Env: `CMTRACE_SQLITE_PATH`.
    /// Default: `<data_dir>/meta.sqlite`.
    pub sqlite_path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid CMTRACE_LISTEN_ADDR: {0}")]
    InvalidListenAddr(String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_addr = match env::var("CMTRACE_LISTEN_ADDR") {
            Ok(value) => value
                .parse()
                .map_err(|_| ConfigError::InvalidListenAddr(value))?,
            Err(_) => "0.0.0.0:8080".parse().expect("static default parses"),
        };

        let data_dir = env::var("CMTRACE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));

        let sqlite_path = env::var("CMTRACE_SQLITE_PATH").unwrap_or_else(|_| {
            data_dir
                .join("meta.sqlite")
                .to_string_lossy()
                .to_string()
        });

        Ok(Self {
            listen_addr,
            data_dir,
            sqlite_path,
        })
    }
}
