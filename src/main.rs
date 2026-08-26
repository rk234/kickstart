mod cli;
mod config;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();

    let config = match config::load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    };

    match cli.command {
        cli::Commands::Up => println!("Up!!"),
        cli::Commands::Services => {
            for name in config.services.keys() {
                println!("{}", name);
            }
        }
    }
}
