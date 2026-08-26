use clap::{Parser, Subcommand};

#[derive(Parser)]
// Configure the top-level command metadata
#[command(name = "kickstart")]
#[command(version = "0.1.0")]
#[command(about = "Kickstarts your local development environments", long_about = None)]
pub struct Cli {
    #[arg(short, long)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Up,
    Services,
}
