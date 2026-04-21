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

    /// Exact origins permitted to call the API from a browser context. Env:
    /// `CMTRACE_CORS_ORIGINS`, comma-separated (e.g.
    /// `http://localhost:5173,http://localhost:4173`). Default: empty, which
    /// means the CORS layer rejects every cross-origin request (fail closed).
    ///
    /// Typical dev values:
    /// - `http://localhost:5173` — Vite dev server
    /// - `http://localhost:4173` — Vite preview server
    ///
    /// Prod deployments that serve the viewer same-origin can leave this
    /// empty; set it to the viewer's public origin only if the viewer lives
    /// on a different host/port.
    pub allowed_origins: Vec<String>,

    /// Whether browsers may include credentials (cookies, `Authorization`
    /// headers set via `fetch({ credentials: "include" })`) on cross-origin
    /// requests. Env: `CMTRACE_CORS_CREDENTIALS`, default `false`.
    ///
    /// Per the CORS spec, when this is `true` the server MUST echo an exact
    /// origin (not a `*` wildcard) in `Access-Control-Allow-Origin`. We always
    /// use an exact-list `AllowOrigin::list(...)`, so this constraint is
    /// already satisfied — but we document it here so the config surface is
    /// self-explanatory.
    pub allow_credentials: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid CMTRACE_LISTEN_ADDR: {0}")]
    InvalidListenAddr(String),
    #[error("invalid CMTRACE_CORS_CREDENTIALS: {0} (expected true/false/1/0)")]
    InvalidCorsCredentials(String),
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

        let allowed_origins = env::var("CMTRACE_CORS_ORIGINS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let allow_credentials = match env::var("CMTRACE_CORS_CREDENTIALS") {
            Ok(v) => parse_bool(&v).ok_or(ConfigError::InvalidCorsCredentials(v))?,
            Err(_) => false,
        };

        Ok(Self {
            listen_addr,
            data_dir,
            sqlite_path,
            allowed_origins,
            allow_credentials,
        })
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}
