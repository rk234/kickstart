use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
enum ReadyCondition {
    ExitCode(i32),
    Port(u32),
    NoOp,
}

#[derive(Serialize, Deserialize, Debug)]
struct ServiceConfig {
    name: String,
    ready_condition: ReadyCondition,
    depends_on: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct IgniteConfig {
    service: Vec<ServiceConfig>,
}

fn main() {
    let content = r#"
        [[service]]
        name = "My Service 1"
        dependencies = []
        [[service]]
        name = "My Service 2"
        dependencies = []
    "#;

    // Parse TOML string to Rust struct
    let config: IgniteConfig = IgniteConfig {
        service: vec![ServiceConfig {
            name: String::from("test"),
            ready_condition: ReadyCondition::NoOp,
            depends_on: vec![],
        }],
    };
    // Convert Rust struct back to TOML string
    let output = toml::to_string_pretty(&config).unwrap();
    println!("{}", output);
}
