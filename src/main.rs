mod cli;
mod config;
mod exec;
mod orchestrator;

use std::{
    collections::{HashMap, HashSet},
    // sync::mpsc,
};

use clap::Parser;

use crate::{
    cli::Cli,
    config::IgniteConfig,
    // exec::{monitor_srv, spawn_srv},
};

fn dispatch(cli: &Cli, config: &IgniteConfig) {
    match cli.command {
        cli::Commands::Up => {
            let mut deps: HashMap<String, HashSet<String>> = HashMap::new();

            for (id, srv) in &config.services {
                deps.insert(
                    id.clone(),
                    srv.depends_on
                        .iter()
                        .map(|d| d.clone())
                        .into_iter()
                        .collect(),
                );
            }

            // let (tx, rx) = mpsc::channel();
            //
            // loop {
            //     let ready: Vec<String> = deps
            //         .iter()
            //         .filter(|(k, v)| v.is_empty())
            //         .map(|(k, _)| k.clone())
            //         .collect();
            //
            //     for id in ready {
            //         if let Some(srv) = config.services.get(&id) {
            //             let child = spawn_srv(&id, &srv);
            //             tokio::spawn(monitor_srv(id.clone(), &srv, &child, tx));
            //         }
            //     }
            // }
        }
        cli::Commands::Services => {
            for name in config.services.keys() {
                println!("{}", name);
            }
        }
    }
}

fn main() {
    let cli = cli::Cli::parse();

    let config = match config::load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    };

    dispatch(&cli, &config);
}
