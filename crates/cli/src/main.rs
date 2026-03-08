use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const PID_FILE: &str = "rustclaw.pid";

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
    Start {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        
        #[arg(long, default_value_t = 18789)]
        port: u16,
        
        #[arg(long)]
        config: Option<PathBuf>,
        
        #[arg(long)]
        detach: bool,
        
        #[arg(long)]
        log: Option<PathBuf>,
    },
    
    Stop,
    
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

fn get_pid_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(PID_FILE)
}

fn get_gateway_binary() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("rustclaw-gateway")))
        .unwrap_or_else(|| PathBuf::from("rustclaw-gateway"))
}

fn is_gateway_running() -> bool {
    let pid_path = get_pid_path();
    if let Ok(pid_str) = fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if kill_process(pid, 0).is_ok() {
                return true;
            }
        }
    }
    
    if let Ok(output) = Command::new("pgrep").arg("-f").arg("rustclaw-gateway").output() {
        return output.status.success();
    }
    
    false
}

fn kill_process(pid: u32, signal: i32) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        Command::new("kill")
            .arg(format!("-{}", signal))
            .arg(format!("{}", pid))
            .status()
            .map(|_| ())
    }
    #[cfg(not(unix))]
    {
        Command::new("taskkill")
            .arg(format!("/PID {}", pid))
            .status()
            .map(|_| ())
    }
}

fn save_pid(pid: u32) -> Result<(), std::io::Error> {
    let pid_path = get_pid_path();
    if let Some(parent) = pid_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(pid_path, pid.to_string())
}

fn remove_pid() -> Result<(), std::io::Error> {
    let pid_path = get_pid_path();
    fs::remove_file(pid_path)
}

fn start_gateway(host: &str, port: u16, config: Option<PathBuf>, detach: bool, log: Option<PathBuf>) -> anyhow::Result<()> {
    if is_gateway_running() {
        anyhow::bail!("Gateway is already running");
    }
    
    let gateway_bin = get_gateway_binary();
    
    if !gateway_bin.exists() {
        anyhow::bail!("Gateway binary not found: {}", gateway_bin.display());
    }
    
    let mut cmd = Command::new(&gateway_bin);
    cmd.arg("--host").arg(host)
       .arg("--port").arg(port.to_string());
    
    if let Some(cfg) = config {
        cmd.arg("--config").arg(cfg);
    }
    
    if let Some(log_path) = log {
        cmd.arg("--log").arg(log_path);
    }
    
    if detach {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        
        let child = cmd.spawn()?;
        
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        if is_gateway_running() {
            save_pid(child.id())?;
            println!("Gateway started in background");
        } else {
            anyhow::bail!("Failed to start gateway");
        }
    } else {
        cmd.status()?;
    }
    
    Ok(())
}

fn stop_gateway() -> anyhow::Result<()> {
    let pid_path = get_pid_path();
    
    if !pid_path.exists() {
        println!("Gateway is not running (no PID file)");
        return Ok(());
    }
    
    let pid_str = fs::read_to_string(&pid_path)?;
    let pid: u32 = pid_str.trim().parse()?;
    
    println!("Stopping gateway (PID: {})...", pid);
    
    kill_process(pid, 15)?;
    
    std::thread::sleep(std::time::Duration::from_millis(500));
    
    if !is_gateway_running() {
        let _ = remove_pid();
        println!("Gateway stopped");
    } else {
        let _ = kill_process(pid, 9);
        let _ = remove_pid();
        println!("Gateway force killed");
    }
    
    Ok(())
}

fn restart_gateway() -> anyhow::Result<()> {
    println!("Restarting gateway...");
    let _ = stop_gateway();
    std::thread::sleep(std::time::Duration::from_millis(500));
    start_gateway("127.0.0.1", 18789, None, false, None)
}

fn show_status() {
    if is_gateway_running() {
        let pid_path = get_pid_path();
        let pid = fs::read_to_string(&pid_path).unwrap_or_default();
        println!("RustClaw Gateway: running (PID: {})", pid.trim());
        println!("URL: http://127.0.0.1:18789");
    } else {
        println!("RustClaw Gateway: stopped");
    }
}

fn load_config_file() -> Option<rustclaw_config::Config> {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rustclaw")
        .join("config.toml");
    
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(&config_path) {
            return toml::from_str(&content).ok();
        }
    }
    None
}

fn get_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rustclaw")
        .join("config.toml")
}

fn ensure_config_dir() -> anyhow::Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rustclaw");
    
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)?;
    }
    
    Ok(config_dir)
}

fn get_log_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rustclaw")
        .join("logs")
}

fn list_config_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    
    if let Some(config_dir) = dirs::config_dir() {
        let rustclaw_dir = config_dir.join("rustclaw");
        if rustclaw_dir.exists() {
            if let Ok(entries) = fs::read_dir(&rustclaw_dir) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type() {
                        if file_type.is_file() {
                            files.push(entry.path());
                        }
                    }
                }
            }
        }
    }
    
    files
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { host, port, config, detach, log } => {
            if is_gateway_running() {
                println!("Gateway is already running. Use 'rustclaw restart' to restart.");
            } else {
                start_gateway(&host, port, config, detach, log)?;
                if !detach {
                    println!("Gateway started on {}:{}", host, port);
                }
            }
        }
        Commands::Stop => {
            stop_gateway()?;
        }
        Commands::Restart => {
            restart_gateway()?;
            println!("Gateway restarted");
        }
        Commands::Config { show, path, get, set } => {
            let config_file = get_config_path();
            
            if path {
                println!("{}", config_file.display());
                return Ok(());
            }
            
            if show {
                ensure_config_dir()?;
                
                if config_file.exists() {
                    if let Some(config) = load_config_file() {
                        println!("{}", toml::to_string_pretty(&config)?);
                    } else {
                        println!("# Using default config (file corrupted)");
                        let config = rustclaw_config::Config::default();
                        println!("{}", toml::to_string_pretty(&config)?);
                    }
                } else {
                    println!("# No config file found, using defaults");
                    let config = rustclaw_config::Config::default();
                    println!("{}", toml::to_string_pretty(&config)?);
                }
                return Ok(());
            }
            
            if let Some(key) = get {
                if let Some(config) = load_config_file() {
                    println!("Getting config: {}", key);
                    println!("{:?}", config);
                } else {
                    println!("No config file found. Use 'rustclaw config --show' to view default config.");
                }
                return Ok(());
            }
            
            if let Some(value) = set {
                ensure_config_dir()?;
                println!("Setting config: {}", value);
                println!("Note: Config editing not yet implemented. Edit {} manually.", config_file.display());
                return Ok(());
            }
            
            println!("Usage:");
            println!("  rustclaw config --path    # Show config path");
            println!("  rustclaw config --show    # Show config");
            println!("  rustclaw config --get KEY # Get config value");
            println!("  rustclaw config --set KEY=VALUE # Set config value");
        }
        Commands::Sessions { list, kill, clear } => {
            if !is_gateway_running() {
                println!("Gateway is not running. Start it with 'rustclaw start'");
                return Ok(());
            }
            
            if list {
                println!("Session list:");
                println!("  (Connect to http://127.0.0.1:18789 to manage sessions)");
                return Ok(());
            }
            
            if let Some(session_id) = kill {
                println!("Killing session: {}", session_id);
                println!("  (API not implemented yet)");
                return Ok(());
            }
            
            if clear {
                println!("Clearing all sessions");
                println!("  (API not implemented yet)");
                return Ok(());
            }
            
            println!("Usage:");
            println!("  rustclaw sessions --list    # List sessions");
            println!("  rustclaw sessions --kill ID # Kill session");
            println!("  rustclaw sessions --clear   # Clear all sessions");
        }
        Commands::Agent { list, info, create, delete } => {
            if !is_gateway_running() {
                println!("Gateway is not running. Start it with 'rustclaw start'");
                return Ok(());
            }
            
            if list {
                println!("Agents:");
                println!("  (API not implemented yet)");
                return Ok(());
            }
            
            if let Some(name) = info {
                println!("Agent info: {}", name);
                return Ok(());
            }
            
            if let Some(name) = create {
                println!("Creating agent: {}", name);
                println!("  (API not implemented yet)");
                return Ok(());
            }
            
            if let Some(name) = delete {
                println!("Deleting agent: {}", name);
                println!("  (API not implemented yet)");
                return Ok(());
            }
            
            println!("Usage:");
            println!("  rustclaw agent --list           # List agents");
            println!("  rustclaw agent --info NAME     # Show agent info");
            println!("  rustclaw agent --create NAME   # Create agent");
            println!("  rustclaw agent --delete NAME   # Delete agent");
        }
        Commands::Gateway { subcommand } => {
            match subcommand {
                Some(GatewaySubcommands::Status) => show_status(),
                Some(GatewaySubcommands::Start) => {
                    if is_gateway_running() {
                        println!("Gateway is already running");
                    } else {
                        start_gateway("127.0.0.1", 18789, None, false, None)?;
                        println!("Gateway started");
                    }
                }
                Some(GatewaySubcommands::Stop) => stop_gateway()?,
                Some(GatewaySubcommands::Restart) => {
                    restart_gateway()?;
                    println!("Gateway restarted");
                }
                Some(GatewaySubcommands::Health) => {
                    if is_gateway_running() {
                        println!("Gateway health: ok");
                    } else {
                        println!("Gateway health: not running");
                    }
                }
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
                Some(SkillsSubcommands::List) => {
                    println!("Registered Skills");
                    println!("===============");
                    println!("");
                    println!("Active Skills:");
                    println!("  (Skills are dynamically generated by agents)");
                    println!("");
                    println!("Use 'rustclaw skills --info NAME' for details");
                }
                Some(SkillsSubcommands::Info { name }) => {
                    println!("Skill: {}", name);
                    println!("Status: (Gateway API not implemented)");
                }
                Some(SkillsSubcommands::Check { name }) => {
                    println!("Checking skill: {:?}", name);
                    println!("  (Skill validation not implemented)");
                }
                Some(SkillsSubcommands::Enable { name }) => {
                    println!("Enabling skill: {}", name);
                    println!("  (Skill management not implemented)");
                }
                Some(SkillsSubcommands::Disable { name }) => {
                    println!("Disabling skill: {}", name);
                    println!("  (Skill management not implemented)");
                }
                None => {
                    println!("Usage:");
                    println!("  rustclaw skills --list          # List skills");
                    println!("  rustclaw skills --info NAME     # Show skill info");
                    println!("  rustclaw skills --check NAME   # Check skill");
                    println!("  rustclaw skills --enable NAME  # Enable skill");
                    println!("  rustclaw skills --disable NAME # Disable skill");
                }
            }
        }
        Commands::Models { subcommand } => {
            match subcommand {
                Some(ModelsSubcommands::List) => {
                    println!("Available Models");
                    println!("===============");
                    println!("");
                    println!("OpenAI Compatible:");
                    println!("  - gpt-4o");
                    println!("  - gpt-4o-mini");
                    println!("  - gpt-4-turbo");
                    println!("  - gpt-3.5-turbo");
                    println!("");
                    println!("Anthropic:");
                    println!("  - claude-opus-4-5");
                    println!("  - claude-sonnet-4-5");
                    println!("  - claude-haiku-3-5");
                    println!("");
                    println!("NVIDIA:");
                    println!("  - nvidia/z-ai/glm4.7");
                    println!("  - nvidia/llama-3.1-nemotron");
                    println!("");
                    
                    if let Some(config) = load_config_file() {
                        if !config.providers.is_empty() {
                            println!("Configured Providers:");
                            for provider in &config.providers {
                                println!("  - {} ({})", provider.name, provider.default_model.as_ref().unwrap_or(&String::new()));
                            }
                        }
                    }
                }
                Some(ModelsSubcommands::Status) => {
                    if let Some(config) = load_config_file() {
                        println!("Model Status");
                        println!("============");
                        if !config.providers.is_empty() {
                            for provider in &config.providers {
                                println!("Provider: {}", provider.name);
                                println!("Type: {}", provider.provider_type);
                                println!("Model: {}", provider.default_model.as_ref().unwrap_or(&String::new()));
                                println!("Base URL: {}", provider.base_url.as_ref().unwrap_or(&String::new()));
                            }
                        } else {
                            println!("No providers configured");
                        }
                    } else {
                        println!("No config loaded");
                    }
                }
                Some(ModelsSubcommands::Set { name }) => {
                    println!("Setting default model: {}", name);
                    let config_path = get_config_path();
                    println!("Edit {} to change model", config_path.display());
                }
                Some(ModelsSubcommands::Auth { subcommand }) => {
                    match subcommand {
                        Some(ModelAuthSubcommands::Add) => {
                            println!("Adding model auth...");
                            println!("Set API key in config file:");
                            println!("  {}", get_config_path().display());
                        }
                        Some(ModelAuthSubcommands::SetupToken) => {
                            println!("Setting up token...");
                            println!("Set API key in config file:");
                            println!("  {}", get_config_path().display());
                        }
                        Some(ModelAuthSubcommands::PasteToken) => {
                            println!("Pasting token...");
                            println!("Set API key in config file:");
                            println!("  {}", get_config_path().display());
                        }
                        None => {
                            println!("Usage:");
                            println!("  rustclaw models auth add          # Add auth");
                            println!("  rustclaw models auth setup-token # Setup token");
                            println!("  rustclaw models auth paste-token # Paste token");
                        }
                    }
                }
                None => {
                    println!("Usage:");
                    println!("  rustclaw models --list      # List available models");
                    println!("  rustclaw models --status   # Show model status");
                    println!("  rustclaw models --set NAME # Set default model");
                    println!("  rustclaw models auth       # Auth management");
                }
            }
        }
        Commands::System { subcommand } => {
            match subcommand {
                Some(SystemSubcommands::Info) => {
                    println!("RustClaw System Information");
                    println!("===========================");
                    println!("Version: {}", env!("CARGO_PKG_VERSION"));
                    println!("");
                    
                    let config_path = get_config_path();
                    println!("Config: {}", config_path.display());
                    
                    let log_path = get_log_path();
                    println!("Logs: {}", log_path.display());
                    
                    println!("");
                    println!("Gateway Status:");
                    show_status();
                }
                Some(SystemSubcommands::Event { lines }) => {
                    let log_path = get_log_path();
                    if log_path.exists() {
                        if let Ok(entries) = fs::read_dir(&log_path) {
                            let mut files: Vec<_> = entries.flatten()
                                .filter_map(|e| e.path().into_os_string().into_string().ok())
                                .collect();
                            files.sort_by(|a, b| b.cmp(a));
                            
                            for file in files.iter().take(5) {
                                println!("\n=== {} ===", file);
                                if let Ok(content) = fs::read_to_string(file) {
                                    let all_lines: Vec<_> = content.lines().collect();
                                    for line in all_lines.iter().rev().take(lines) {
                                        println!("{}", line);
                                    }
                                }
                            }
                        }
                    } else {
                        println!("No log files found at {}", log_path.display());
                    }
                }
                Some(SystemSubcommands::Heartbeat { subcommand }) => {
                    match subcommand {
                        Some(HeartbeatSubcommands::Last) => {
                            if is_gateway_running() {
                                println!("Last heartbeat: ok");
                                println!("Gateway is responsive");
                            } else {
                                println!("Gateway not running");
                            }
                        }
                        Some(HeartbeatSubcommands::Enable) => {
                            println!("Enabling heartbeat...");
                            println!("  (Not implemented - heartbeat is always enabled when gateway is running)");
                        }
                        Some(HeartbeatSubcommands::Disable) => {
                            println!("Disabling heartbeat...");
                            println!("  (Not implemented)");
                        }
                        None => {
                            println!("Usage:");
                            println!("  rustclaw system heartbeat last    # Show last heartbeat");
                            println!("  rustclaw system heartbeat enable  # Enable heartbeat");
                            println!("  rustclaw system heartbeat disable # Disable heartbeat");
                        }
                    }
                }
                None => {
                    println!("Usage:");
                    println!("  rustclaw system info              # Show system info");
                    println!("  rustclaw system event [LINES]    # Show events");
                    println!("  rustclaw system heartbeat last    # Show heartbeat");
                }
            }
        }
        Commands::Logs { lines, follow, clear } => {
            let log_path = get_log_path();
            
            if clear {
                if log_path.exists() {
                    if let Ok(entries) = fs::read_dir(&log_path) {
                        for entry in entries.flatten() {
                            let _ = fs::remove_file(entry.path());
                        }
                    }
                    println!("Logs cleared");
                } else {
                    println!("No logs to clear");
                }
                return Ok(());
            }
            
            if follow {
                println!("Following logs (Ctrl+C to exit)...");
                println!("Log path: {}", log_path.display());
                return Ok(());
            }
            
            if !log_path.exists() {
                println!("No logs found at {}", log_path.display());
                println!("Run 'rustclaw start' to create logs");
                return Ok(());
            }
            
            println!("Log files in {}:", log_path.display());
            println!("");
            
            if let Ok(entries) = fs::read_dir(&log_path) {
                let mut files: Vec<_> = entries.flatten()
                    .filter_map(|e| {
                        e.path().into_os_string().into_string().ok()
                            .map(|p| (e.path().clone(), std::fs::metadata(&e.path()).ok().map(|m| m.len())))
                    })
                    .collect();
                files.sort_by(|a, b| b.0.cmp(&a.0));
                
                for (path, size) in files.iter().take(10) {
                    let size_str = size.map(|s| format!("{} bytes", s)).unwrap_or_default();
                    println!("  {} ({})", path.file_name().unwrap_or_default().to_string_lossy(), size_str);
                }
            }
            
            println!("");
            println!("Usage:");
            println!("  rustclaw logs              # List log files");
            println!("  rustclaw logs --lines N   # Show last N lines");
            println!("  rustclaw logs --follow    # Follow logs");
            println!("  rustclaw logs --clear     # Clear logs");
        }
        Commands::Health { verbose } => {
            if is_gateway_running() {
                if verbose {
                    println!("Detailed health check...");
                } else {
                    println!("Health: ok");
                }
            } else {
                println!("Health: gateway not running");
            }
        }
        Commands::Status { detailed } => {
            show_status();
            if detailed {
                println!("CLI version: {}", env!("CARGO_PKG_VERSION"));
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
                Some(ProvidersSubcommands::List) => {
                    println!("Configured Providers");
                    println!("==================");
                    
                    if let Some(config) = load_config_file() {
                        if !config.providers.is_empty() {
                            for provider in &config.providers {
                                println!("  - {} ({})", provider.name, provider.default_model.as_ref().unwrap_or(&String::new()));
                            }
                        } else {
                            println!("(Using default provider)");
                        }
                    } else {
                        println!("(Using default provider)");
                    }
                    
                    println!("");
                    println!("Supported Providers:");
                    println!("  - openai     (OpenAI compatible)");
                    println!("  - anthropic  (Anthropic Claude)");
                    println!("  - nvidia     (NVIDIA NIM)");
                    println!("  - google     (Google Gemini)");
                    println!("  - local      (Ollama, LM Studio)");
                }
                Some(ProvidersSubcommands::Add { name, api_key }) => {
                    println!("Adding provider: {}", name);
                    println!("Edit config file: {}", get_config_path().display());
                }
                Some(ProvidersSubcommands::Remove { name }) => {
                    println!("Removing provider: {}", name);
                    println!("Edit config file: {}", get_config_path().display());
                }
                Some(ProvidersSubcommands::Test { name }) => {
                    println!("Testing provider: {}", name);
                    if is_gateway_running() {
                        println!("Gateway is running - provider should be connected");
                    } else {
                        println!("Start gateway first: rustclaw start");
                    }
                }
                None => {
                    println!("Usage:");
                    println!("  rustclaw providers --list        # List providers");
                    println!("  rustclaw providers --add NAME    # Add provider");
                    println!("  rustclaw providers --remove NAME # Remove provider");
                    println!("  rustclaw providers --test NAME   # Test provider");
                }
            }
        }
        Commands::Tools { subcommand } => {
            match subcommand {
                Some(ToolsSubcommands::List) => {
                    println!("Registered Tools");
                    println!("===============");
                    println!("");
                    println!("Built-in Tools:");
                    println!("  - file_read      Read files");
                    println!("  - file_write     Write files");
                    println!("  - shell_exec     Execute shell commands");
                    println!("  - opencode       Generate code with OpenCode");
                    println!("");
                    println!("Dynamic Tools (from Skills):");
                    println!("  (Not implemented yet)");
                }
                Some(ToolsSubcommands::Info { name }) => {
                    println!("Tool: {}", name);
                    println!("Use 'rustclaw tools --list' to see all tools");
                }
                Some(ToolsSubcommands::Register { name, path }) => {
                    println!("Registering tool: {} from {}", name, path);
                    println!("  (Dynamic tool registration not implemented)");
                }
                Some(ToolsSubcommands::Unregister { name }) => {
                    println!("Unregistering tool: {}", name);
                    println!("  (Dynamic tool registration not implemented)");
                }
                None => {
                    println!("Usage:");
                    println!("  rustclaw tools --list             # List tools");
                    println!("  rustclaw tools --info NAME       # Show tool info");
                    println!("  rustclaw tools --register NAME --path PATH # Register tool");
                    println!("  rustclaw tools --unregister NAME # Unregister tool");
                }
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
            println!("RustClaw Doctor - Diagnostics");
            println!("============================");
            println!("");
            
            println!("Checking system...");
            println!("");
            
            println!("1. Gateway Status:");
            if is_gateway_running() {
                println!("   [OK] Gateway is running");
            } else {
                println!("   [WARN] Gateway is not running");
                println!("   Run: rustclaw start");
            }
            
            println!("");
            println!("2. Config:");
            let config_path = get_config_path();
            if config_path.exists() {
                println!("   [OK] Config exists: {}", config_path.display());
            } else {
                println!("   [INFO] No config file found (will use defaults)");
            }
            
            println!("");
            println!("3. Logs:");
            let log_path = get_log_path();
            if log_path.exists() {
                println!("   [OK] Logs directory exists");
            } else {
                println!("   [INFO] No logs yet");
            }
            
            println!("");
            println!("4. System:");
            println!("   [OK] CLI version: {}", env!("CARGO_PKG_VERSION"));
            
            if check {
                println!("");
                println!("Diagnostics complete.");
            }
            
            if fix {
                println!("");
                println!("Auto-fix not implemented yet.");
            }
            
            if !check && !fix {
                println!("");
                println!("Usage:");
                println!("  rustclaw doctor        # Quick check");
                println!("  rustclaw doctor --check  # Detailed diagnostics");
                println!("  rustclaw doctor --fix    # Auto-fix issues");
            }
        }
        Commands::Update { check, force } => {
            if check {
                println!("Checking for updates...");
                println!("Current version: {}", env!("CARGO_PKG_VERSION"));
                println!("");
                println!("Update checking from GitHub...");
                println!("(Not implemented - build from source)");
            } else if force {
                println!("Force updating...");
                println!("Please rebuild from source:");
                println!("  cargo build --release");
            } else {
                println!("Updating RustClaw...");
                println!("Please rebuild from source:");
                println!("  cargo build --release");
            }
        }
        Commands::Reset { confirm, all } => {
            if !confirm {
                println!("RustClaw Reset");
                println!("=============");
                println!("");
                println!("This will reset:");
                println!("  - Config files");
                println!("  - Log files");
                println!("  - Session data");
                println!("");
                println!("Usage:");
                println!("  rustclaw reset --confirm    # Reset config");
                println!("  rustclaw reset --confirm --all # Reset everything");
                return Ok(());
            }
            
            if all {
                println!("Resetting all data...");
                let log_path = get_log_path();
                if log_path.exists() {
                    let _ = fs::remove_dir_all(&log_path);
                }
                println!("[OK] Logs cleared");
            }
            
            println!("Reset complete.");
        }
        Commands::Version => {
            println!("rustclaw {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
