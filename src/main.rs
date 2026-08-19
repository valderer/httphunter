mod api;
mod ca;
mod capture;
mod config;
mod error;
mod logging;
mod system_proxy;

use std::net::SocketAddr;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::AppConfig;
use hunter_core::HunterRuntime;

#[derive(Debug, Parser)]
#[command(name = "httphunter", version, about = "A local HTTP debugging proxy")]
struct Cli {
    /// Optional TOML configuration file.
    #[arg(long, global = true, env = "HTTPHUNTER_CONFIG")]
    config: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the local proxy.
    Proxy {
        /// Address on which the proxy listens.
        #[arg(long)]
        listen: Option<SocketAddr>,

        /// Enable HTTPS MITM. Reserved for the upcoming MITM implementation.
        #[arg(long)]
        mitm: bool,
    },
    /// Manage the local CA used by HTTPS MITM.
    Ca {
        #[command(subcommand)]
        command: CaCommand,
    },
    /// Print the effective configuration.
    Config,
}

#[derive(Debug, Subcommand)]
enum CaCommand {
    /// Generate the local CA certificate and private key.
    Generate {
        #[arg(long)]
        force: bool,
    },
    /// Print the local CA certificate path.
    Path,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(cli.config.as_deref())?;
    logging::init(&config.logging)?;

    match cli.command {
        Command::Proxy { listen, mitm } => {
            let listen = listen.unwrap_or(config.proxy.listen);
            let mitm_enabled = mitm || config.proxy.mitm;
            if mitm_enabled {
                tracing::info!("HTTPS MITM is enabled");
            }

            let runtime = HunterRuntime::new(config.clone(), listen, mitm_enabled);
            runtime.start().await?;

            if config.api.enabled {
                let api_store = runtime.store();
                let api_listen = config.api.listen;
                let system_proxy = system_proxy::SystemProxyController::new(
                    &config.system_proxy,
                    listen.ip().to_string(),
                    listen.port(),
                );
                tokio::spawn(async move {
                    if let Err(error) = api::run(api_listen, api_store, system_proxy).await {
                        tracing::error!(%error, "local API stopped");
                    }
                });
            }

            tokio::signal::ctrl_c().await?;
            runtime.stop().await?;
        }
        Command::Config => {
            println!("{}", toml::to_string_pretty(&config)?);
        }
        Command::Ca { command } => {
            let store = ca::CaStore::default()?;
            match command {
                CaCommand::Generate { force } => {
                    store.generate(force)?;
                    println!("CA certificate: {}", store.cert_path().display());
                    println!("CA private key: {}", store.key_path().display());
                }
                CaCommand::Path => println!("{}", store.cert_path().display()),
            }
        }
    }

    Ok(())
}
