use rustclaw_gateway::Gateway;
use rustclaw_config::Config;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Starting RustClaw Gateway v{}", env!("CARGO_PKG_VERSION"));

    let config = Config::load(None)?;
    
    let gateway = Gateway::new(config);

    gateway.start().await?;

    Ok(())
}
