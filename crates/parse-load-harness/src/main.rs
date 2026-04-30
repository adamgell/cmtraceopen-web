use clap::Parser;
use parse_load_harness::config::{Cli, Cmd, RunConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "parse_load_harness=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run(args) => {
            let cfg = RunConfig::from_args(*args)?;
            tracing::info!(run = %cfg.run_uuid, seed = cfg.seed, "run config validated");
            anyhow::bail!("run command not yet implemented");
        }
        Cmd::MineDump(_) => anyhow::bail!("mine-dump command not yet implemented"),
        Cmd::Clean(_) => anyhow::bail!("clean command not yet implemented"),
    }
}
