use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rustclaw")]
#[command(about = "RustClaw - AI Agent Gateway", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Start {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        
        #[arg(long, default_value_t = 18789)]
        port: u16,
    },
    Config {
        #[arg(long)]
        show: bool,
        
        #[arg(long)]
        path: bool,
    },
    Version,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { host, port } => {
            println!("Starting RustClaw on {}:{}", host, port);
        }
        Commands::Config { show, path } => {
            if path {
                let config_path = dirs::config_dir()
                    .unwrap_or_default()
                    .join("rustclaw")
                    .join("config.toml");
                println!("{}", config_path.display());
            } else if show {
                let config = rustclaw_config::Config::default();
                println!("{}", toml::to_string_pretty(&config)?);
            }
        }
        Commands::Version => {
            println!("rustclaw {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
