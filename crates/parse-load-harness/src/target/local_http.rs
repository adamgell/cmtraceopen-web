//! Local-http target — wraps a `docker compose` lifecycle around the
//! remote-http client. Brings the stack up at construction, tears it down
//! on shutdown, returns the locally-bound URL.

use crate::auth::Auth;
use crate::bundle::{Bundle, BundleResult};
use crate::target::remote_http::RemoteHttpTarget;
use crate::target::Target;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

pub struct LocalHttpTarget {
    inner: RemoteHttpTarget,
    project_name: String,
    compose_path: PathBuf,
    no_cleanup: bool,
}

impl LocalHttpTarget {
    pub async fn up(
        compose_path: PathBuf,
        image: Option<String>,
        ca_dir: Option<PathBuf>,
        auth: Arc<dyn Auth>,
        no_cleanup: bool,
    ) -> anyhow::Result<Self> {
        let project_name = format!("cmtrace-load-{}", uuid::Uuid::new_v4().simple());
        let mut up = Command::new("docker");
        up.args([
            "compose",
            "-p",
            &project_name,
            "-f",
            compose_path.to_str().unwrap(),
            "up",
            "-d",
            "--quiet-pull",
            "--wait",
        ]);
        if let Some(img) = &image {
            up.env("CMTRACE_API_IMAGE", img);
        }
        if let Some(dir) = &ca_dir {
            up.env("CMTRACE_LOAD_TEST_CA_DIR", dir);
            // Pass the in-container path to the CA bundle.
            up.env("CMTRACE_CLIENT_CA_BUNDLE", "/etc/cmtrace/test-ca/ca.pem");
        }
        let status = up.status().await?;
        anyhow::ensure!(status.success(), "compose up failed: {status}");

        let port = read_compose_port(&project_name, &compose_path, "api", 8080).await?;
        let base_url = format!("http://127.0.0.1:{port}");
        let client = build_client(ca_dir.as_deref()).await?;

        // Wait for /healthz.
        let started = std::time::Instant::now();
        loop {
            if started.elapsed() > Duration::from_secs(60) {
                anyhow::bail!("api never became healthy");
            }
            if let Ok(r) = client.get(format!("{base_url}/healthz")).send().await {
                if r.status().is_success() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let inner = RemoteHttpTarget::new(base_url, client, auth);
        Ok(Self {
            inner,
            project_name,
            compose_path,
            no_cleanup,
        })
    }
}

async fn read_compose_port(
    project: &str,
    compose: &std::path::Path,
    service: &str,
    container_port: u16,
) -> anyhow::Result<u16> {
    let out = Command::new("docker")
        .args([
            "compose",
            "-p",
            project,
            "-f",
            compose.to_str().unwrap(),
            "port",
            service,
            &container_port.to_string(),
        ])
        .output()
        .await?;
    anyhow::ensure!(out.status.success(), "docker compose port failed");
    let raw = String::from_utf8(out.stdout)?;
    // Output looks like "127.0.0.1:54321\n" — split on ':' and trim.
    let port = raw
        .trim()
        .split(':')
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("port output empty"))?
        .parse::<u16>()?;
    Ok(port)
}

async fn build_client(ca_dir: Option<&std::path::Path>) -> anyhow::Result<Client> {
    if let Some(dir) = ca_dir {
        let ca_pem = tokio::fs::read(dir.join("ca.pem")).await?;
        // reqwest with rustls backend uses Identity::from_pem, which expects a
        // single PEM buffer containing both the cert chain and private key.
        let cert_pem = tokio::fs::read(dir.join("client.crt")).await?;
        let key_pem = tokio::fs::read(dir.join("client.key")).await?;
        let mut combined = cert_pem;
        combined.extend_from_slice(&key_pem);
        let identity = reqwest::Identity::from_pem(&combined)?;
        let ca = reqwest::Certificate::from_pem(&ca_pem)?;
        Ok(Client::builder()
            .add_root_certificate(ca)
            .identity(identity)
            .build()?)
    } else {
        Ok(Client::builder().build()?)
    }
}

#[async_trait::async_trait]
impl Target for LocalHttpTarget {
    async fn send(&self, bundle: Bundle) -> BundleResult {
        self.inner.send(bundle).await
    }
    async fn sample_system(&self) -> serde_json::Value {
        self.inner.sample_system().await
    }
    async fn shutdown(self: Box<Self>) -> anyhow::Result<()> {
        if self.no_cleanup {
            tracing::info!(project = %self.project_name, "--no-cleanup: leaving compose up");
            return Ok(());
        }
        let status = Command::new("docker")
            .args([
                "compose",
                "-p",
                &self.project_name,
                "-f",
                self.compose_path.to_str().unwrap(),
                "down",
                "-v",
            ])
            .status()
            .await?;
        anyhow::ensure!(status.success(), "compose down failed: {status}");
        Ok(())
    }
}
