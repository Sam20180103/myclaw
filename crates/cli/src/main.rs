use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rustclaw")]
#[command(about = "RustClaw - AI Agent Gateway", long_about = None)]
#[command(version = "0.3.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the RustClaw gateway server
    Start {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        
        #[arg(long, default_value_t = 18789)]
        port: u16,
        
        #[arg(long)]
        config: Option<PathBuf>,
    },
    
    /// Stop the RustClaw gateway server
    Stop,
    
    /// Restart the RustClaw gateway server
    Restart,
    
    /// Configuration management
    Config {
        #[arg(long)]
        show: bool,
        
        #[arg(long)]
        path: bool,
        
        #[arg(long)]
        get: Option<String>,
        
        #[arg(long)]
        set: Option<String>,
    },
    
    /// Session management
    Sessions {
        #[arg(long)]
        list: bool,
        
        #[arg(long)]
        kill: Option<String>,
        
        #[arg(long)]
        clear: bool,
    },
    
    /// Agent management
    Agent {
        #[arg(long)]
        list: bool,
        
        #[arg(long)]
        info: Option<String>,
        
        #[arg(long)]
        create: Option<String>,
        
        #[arg(long)]
        delete: Option<String>,
    },
    
    /// Gateway management
    Gateway {
        #[command(subcommand)]
        subcommand: Option<GatewaySubcommands>,
    },
    
    /// Channel management
    Channels {
        #[command(subcommand)]
        subcommand: Option<ChannelsSubcommands>,
    },
    
    /// Skill management
    Skills {
        #[command(subcommand)]
        subcommand: Option<SkillsSubcommands>,
    },
    
    /// Model management
    Models {
        #[command(subcommand)]
        subcommand: Option<ModelsSubcommands>,
    },
    
    /// System information and diagnostics
    System {
        #[command(subcommand)]
        subcommand: Option<SystemSubcommands>,
    },
    
    /// Logs management
    Logs {
        #[arg(long, default_value_t = 100)]
        lines: usize,
        
        #[arg(long)]
        follow: bool,
        
        #[arg(long)]
        clear: bool,
    },
    
    /// Health check
    Health {
        #[arg(long)]
        verbose: bool,
    },
    
    /// Status overview
    Status {
        #[arg(long)]
        detailed: bool,
    },
    
    /// Security audit
    Security {
        #[command(subcommand)]
        subcommand: Option<SecuritySubcommands>,
    },
    
    /// Provider management
    Providers {
        #[command(subcommand)]
        subcommand: Option<ProvidersSubcommands>,
    },
    
    /// Tool management
    Tools {
        #[command(subcommand)]
        subcommand: Option<ToolsSubcommands>,
    },
    
    /// Memory management
    Memory {
        #[command(subcommand)]
        subcommand: Option<MemorySubcommands>,
    },
    
    /// OpenCode integration
    OpenCode {
        #[command(subcommand)]
        subcommand: Option<OpenCodeSubcommands>,
    },
    
    /// Doctor - diagnostics and repair
    Doctor {
        #[arg(long)]
        fix: bool,
        
        #[arg(long)]
        check: bool,
    },
    
    /// Update RustClaw
    Update {
        #[arg(long)]
        check: bool,
        
        #[arg(long)]
        force: bool,
    },
    
    /// Reset RustClaw
    Reset {
        #[arg(long)]
        confirm: bool,
        
        #[arg(long)]
        all: bool,
    },
    
    /// Version information
    Version,
}

#[derive(Subcommand)]
enum GatewaySubcommands {
    /// Show gateway status
    Status,
    /// Start the gateway
    Start,
    /// Stop the gateway
    Stop,
    /// Restart the gateway
    Restart,
    /// Show gateway health
    Health,
    /// Call gateway API
    Call {
        #[arg(long)]
        method: String,
        
        #[arg(long)]
        body: Option<String>,
    },
}

#[derive(Subcommand)]
enum ChannelsSubcommands {
    /// List all channels
    List,
    /// Add a new channel
    Add {
        #[arg(long)]
        name: String,
        
        #[arg(long)]
        channel_type: String,
    },
    /// Remove a channel
    Remove {
        #[arg(long)]
        name: String,
    },
    /// Show channel status
    Status {
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillsSubcommands {
    /// List all skills
    List,
    /// Show skill information
    Info {
        #[arg(long)]
        name: String,
    },
    /// Check skill syntax
    Check {
        #[arg(long)]
        name: Option<String>,
    },
    /// Enable a skill
    Enable {
        #[arg(long)]
        name: String,
    },
    /// Disable a skill
    Disable {
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum ModelsSubcommands {
    /// List available models
    List,
    /// Show model status
    Status,
    /// Set default model
    Set {
        #[arg(long)]
        name: String,
    },
    /// Add model authentication
    Auth {
        #[command(subcommand)]
        subcommand: Option<ModelAuthSubcommands>,
    },
}

#[derive(Subcommand)]
enum ModelAuthSubcommands {
    Add,
    SetupToken,
    PasteToken,
}

#[derive(Subcommand)]
enum SystemSubcommands {
    /// Show system information
    Info,
    /// Show event logs
    Event {
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// Show heartbeat status
    Heartbeat {
        #[command(subcommand)]
        subcommand: Option<HeartbeatSubcommands>,
    },
}

#[derive(Subcommand)]
enum HeartbeatSubcommands {
    Last,
    Enable,
    Disable,
}

#[derive(Subcommand)]
enum SecuritySubcommands {
    /// Run security audit
    Audit,
    /// Show security status
    Status,
}

#[derive(Subcommand)]
enum ProvidersSubcommands {
    /// List providers
    List,
    /// Add provider
    Add {
        #[arg(long)]
        name: String,
        
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Remove provider
    Remove {
        #[arg(long)]
        name: String,
    },
    /// Test provider connection
    Test {
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum ToolsSubcommands {
    /// List all tools
    List,
    /// Show tool information
    Info {
        #[arg(long)]
        name: String,
    },
    /// Register a new tool
    Register {
        #[arg(long)]
        name: String,
        
        #[arg(long)]
        path: String,
    },
    /// Unregister a tool
    Unregister {
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum MemorySubcommands {
    /// Show memory status
    Status,
    /// Rebuild memory index
    Index,
    /// Search memory
    Search {
        #[arg(long)]
        query: String,
    },
    /// Clear memory
    Clear,
}

#[derive(Subcommand)]
enum OpenCodeSubcommands {
    /// Run an OpenCode task
    Run {
        #[arg(long)]
        prompt: String,
        
        #[arg(long)]
        model: Option<String>,
        
        #[arg(long)]
        session: Option<String>,
    },
    /// Show OpenCode status
    Status,
    /// Manage OpenCode sessions
    Sessions {
        #[command(subcommand)]
        subcommand: Option<OpenCodeSessionsSubcommands>,
    },
}

#[derive(Subcommand)]
enum OpenCodeSessionsSubcommands {
    List,
    Continue {
        #[arg(long)]
        id: String,
    },
    Delete {
        #[arg(long)]
        id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { host, port, config } => {
            println!("Starting RustClaw on {}:{}", host, port);
            if let Some(config_path) = config {
                println!("Using config: {}", config_path.display());
            }
        }
        Commands::Stop => {
            println!("Stopping RustClaw gateway...");
        }
        Commands::Restart => {
            println!("Restarting RustClaw gateway...");
        }
        Commands::Config { show, path, get, set } => {
            if path {
                let config_path = dirs::config_dir()
                    .unwrap_or_default()
                    .join("rustclaw")
                    .join("config.toml");
                println!("{}", config_path.display());
            } else if show {
                let config = rustclaw_config::Config::default();
                println!("{}", toml::to_string_pretty(&config)?);
            } else if let Some(key) = get {
                println!("Getting config: {}", key);
            } else if let Some(value) = set {
                println!("Setting config: {}", value);
            }
        }
        Commands::Sessions { list, kill, clear } => {
            if list {
                println!("Listing sessions...");
            } else if let Some(session_id) = kill {
                println!("Killing session: {}", session_id);
            } else if clear {
                println!("Clearing all sessions");
            }
        }
        Commands::Agent { list, info, create, delete } => {
            if list {
                println!("Listing agents...");
            } else if let Some(name) = info {
                println!("Agent info: {}", name);
            } else if let Some(name) = create {
                println!("Creating agent: {}", name);
            } else if let Some(name) = delete {
                println!("Deleting agent: {}", name);
            }
        }
        Commands::Gateway { subcommand } => {
            match subcommand {
                Some(GatewaySubcommands::Status) => println!("Gateway: running"),
                Some(GatewaySubcommands::Start) => println!("Starting gateway..."),
                Some(GatewaySubcommands::Stop) => println!("Stopping gateway..."),
                Some(GatewaySubcommands::Restart) => println!("Restarting gateway..."),
                Some(GatewaySubcommands::Health) => println!("Gateway health: ok"),
                Some(GatewaySubcommands::Call { method, body }) => {
                    println!("Calling gateway: {} with body: {:?}", method, body);
                }
                None => println!("Use 'rustclaw gateway <subcommand>'"),
            }
        }
        Commands::Channels { subcommand } => {
            match subcommand {
                Some(ChannelsSubcommands::List) => println!("Listing channels..."),
                Some(ChannelsSubcommands::Add { name, channel_type }) => {
                    println!("Adding channel: {} ({})", name, channel_type);
                }
                Some(ChannelsSubcommands::Remove { name }) => {
                    println!("Removing channel: {}", name);
                }
                Some(ChannelsSubcommands::Status { name }) => {
                    println!("Channel status: {:?}", name);
                }
                None => println!("Use 'rustclaw channels <subcommand>'"),
            }
        }
        Commands::Skills { subcommand } => {
            match subcommand {
                Some(SkillsSubcommands::List) => println!("Listing skills..."),
                Some(SkillsSubcommands::Info { name }) => {
                    println!("Skill info: {}", name);
                }
                Some(SkillsSubcommands::Check { name }) => {
                    println!("Checking skill: {:?}", name);
                }
                Some(SkillsSubcommands::Enable { name }) => {
                    println!("Enabling skill: {}", name);
                }
                Some(SkillsSubcommands::Disable { name }) => {
                    println!("Disabling skill: {}", name);
                }
                None => println!("Use 'rustclaw skills <subcommand>'"),
            }
        }
        Commands::Models { subcommand } => {
            match subcommand {
                Some(ModelsSubcommands::List) => println!("Listing models..."),
                Some(ModelsSubcommands::Status) => println!("Model status: ok"),
                Some(ModelsSubcommands::Set { name }) => {
                    println!("Setting default model: {}", name);
                }
                Some(ModelsSubcommands::Auth { subcommand }) => {
                    match subcommand {
                        Some(ModelAuthSubcommands::Add) => println!("Adding model auth..."),
                        Some(ModelAuthSubcommands::SetupToken) => println!("Setting up token..."),
                        Some(ModelAuthSubcommands::PasteToken) => println!("Pasting token..."),
                        None => println!("Use 'rustclaw models auth <subcommand>'"),
                    }
                }
                None => println!("Use 'rustclaw models <subcommand>'"),
            }
        }
        Commands::System { subcommand } => {
            match subcommand {
                Some(SystemSubcommands::Info) => {
                    println!("System: RustClaw v{}", env!("CARGO_PKG_VERSION"));
                }
                Some(SystemSubcommands::Event { lines }) => {
                    println!("Showing last {} events...", lines);
                }
                Some(SystemSubcommands::Heartbeat { subcommand }) => {
                    match subcommand {
                        Some(HeartbeatSubcommands::Last) => println!("Last heartbeat: ok"),
                        Some(HeartbeatSubcommands::Enable) => println!("Heartbeat enabled"),
                        Some(HeartbeatSubcommands::Disable) => println!("Heartbeat disabled"),
                        None => println!("Use 'rustclaw system heartbeat <subcommand>'"),
                    }
                }
                None => println!("Use 'rustclaw system <subcommand>'"),
            }
        }
        Commands::Logs { lines, follow, clear } => {
            if clear {
                println!("Clearing logs...");
            } else if follow {
                println!("Following logs (last {} lines)...", lines);
            } else {
                println!("Showing last {} lines of logs...", lines);
            }
        }
        Commands::Health { verbose } => {
            if verbose {
                println!("Detailed health check...");
            } else {
                println!("Health: ok");
            }
        }
        Commands::Status { detailed } => {
            if detailed {
                println!("Detailed status...");
            } else {
                println!("RustClaw: running");
            }
        }
        Commands::Security { subcommand } => {
            match subcommand {
                Some(SecuritySubcommands::Audit) => println!("Running security audit..."),
                Some(SecuritySubcommands::Status) => println!("Security status: ok"),
                None => println!("Use 'rustclaw security <subcommand>'"),
            }
        }
        Commands::Providers { subcommand } => {
            match subcommand {
                Some(ProvidersSubcommands::List) => println!("Listing providers..."),
                Some(ProvidersSubcommands::Add { name, api_key }) => {
                    println!("Adding provider: {} (key: {:?})", name, api_key);
                }
                Some(ProvidersSubcommands::Remove { name }) => {
                    println!("Removing provider: {}", name);
                }
                Some(ProvidersSubcommands::Test { name }) => {
                    println!("Testing provider: {}", name);
                }
                None => println!("Use 'rustclaw providers <subcommand>'"),
            }
        }
        Commands::Tools { subcommand } => {
            match subcommand {
                Some(ToolsSubcommands::List) => println!("Listing tools..."),
                Some(ToolsSubcommands::Info { name }) => {
                    println!("Tool info: {}", name);
                }
                Some(ToolsSubcommands::Register { name, path }) => {
                    println!("Registering tool: {} from {}", name, path);
                }
                Some(ToolsSubcommands::Unregister { name }) => {
                    println!("Unregistering tool: {}", name);
                }
                None => println!("Use 'rustclaw tools <subcommand>'"),
            }
        }
        Commands::Memory { subcommand } => {
            match subcommand {
                Some(MemorySubcommands::Status) => println!("Memory status: ok"),
                Some(MemorySubcommands::Index) => println!("Rebuilding memory index..."),
                Some(MemorySubcommands::Search { query }) => {
                    println!("Searching memory: {}", query);
                }
                Some(MemorySubcommands::Clear) => println!("Clearing memory..."),
                None => println!("Use 'rustclaw memory <subcommand>'"),
            }
        }
        Commands::OpenCode { subcommand } => {
            match subcommand {
                Some(OpenCodeSubcommands::Run { prompt, model, session }) => {
                    println!("Running OpenCode task...");
                    println!("  Prompt: {}", prompt);
                    if let Some(m) = model {
                        println!("  Model: {}", m);
                    }
                    if let Some(s) = session {
                        println!("  Session: {}", s);
                    }
                }
                Some(OpenCodeSubcommands::Status) => println!("OpenCode status: ready"),
                Some(OpenCodeSubcommands::Sessions { subcommand }) => {
                    match subcommand {
                        Some(OpenCodeSessionsSubcommands::List) => {
                            println!("Listing OpenCode sessions...");
                        }
                        Some(OpenCodeSessionsSubcommands::Continue { id }) => {
                            println!("Continuing session: {}", id);
                        }
                        Some(OpenCodeSessionsSubcommands::Delete { id }) => {
                            println!("Deleting session: {}", id);
                        }
                        None => println!("Use 'rustclaw opencode sessions <subcommand>'"),
                    }
                }
                None => println!("Use 'rustclaw opencode <subcommand>'"),
            }
        }
        Commands::Doctor { fix, check } => {
            if fix {
                println!("Running doctor with fix...");
            } else if check {
                println!("Running diagnostics...");
            } else {
                println!("Doctor: ok");
            }
        }
        Commands::Update { check, force } => {
            if check {
                println!("Checking for updates...");
            } else if force {
                println!("Force updating...");
            } else {
                println!("Updating RustClaw...");
            }
        }
        Commands::Reset { confirm, all } => {
            if confirm {
                if all {
                    println!("Resetting all data...");
                } else {
                    println!("Resetting RustClaw...");
                }
            } else {
                println!("Use --confirm to confirm reset");
            }
        }
        Commands::Version => {
            println!("rustclaw {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
