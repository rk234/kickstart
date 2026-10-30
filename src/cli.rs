use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kickstart")]
#[command(version = "0.2.0")]
#[command(
    about = "Orchestrate your development environments",
    long_about = "Kickstart spins up your local development services in dependency order, launching each one once its dependencies are ready."
)]
pub struct Cli {
    /// Optional configuration file path.
    /// Default: kickstart.toml
    #[arg(short, long, global = true)]
    pub config: Option<String>,

    /// Optional env file path.
    /// Default: .env
    #[arg(short, long, global = true)]
    pub env: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Starts services
    Up,
    /// List configured services, dependencies, and ready conditions
    Services {
        /// Print services as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
}
