use std::{collections::HashMap, fmt::Display};

use serde::{Deserialize, Serialize};
use toml;

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum ReadyCondition {
    ExitCode(i32),
    Port(u16),
    Delay(u64),
    Immediate,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Service {
    pub name: Option<String>,
    pub command: String,
    pub ready_condition: ReadyCondition,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct IgniteConfig {
    pub services: HashMap<String, Service>,
}

#[derive(Debug)]
pub enum LoadError {
    IoError(std::io::Error),
    DeserializeError(toml::de::Error),
}

impl Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::IoError(err) => write!(f, "failed to read config file: {}", err),
            LoadError::DeserializeError(err) => write!(f, "failed to parse config file: {}", err),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        return Self::IoError(value);
    }
}

impl From<toml::de::Error> for LoadError {
    fn from(value: toml::de::Error) -> Self {
        return Self::DeserializeError(value);
    }
}

const DEFAULT_CONFIG_PATH: &str = "kickstart.toml";

pub fn read_config(path: Option<&str>) -> Result<String, std::io::Error> {
    std::fs::read_to_string(path.unwrap_or(DEFAULT_CONFIG_PATH))
}

pub fn parse_config(cfg: &str) -> Result<IgniteConfig, toml::de::Error> {
    let config = toml::from_str(cfg)?;
    Ok(config)
}

pub fn load_config(cfg: Option<&str>) -> Result<IgniteConfig, LoadError> {
    let content = read_config(cfg)?;
    let cfg = parse_config(content.as_str())?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_service() {
        let cfg = parse_config(
            r#"
            [services.web]
            command = "echo hi"
            ready_condition = "immediate"
            "#,
        )
        .unwrap();

        let web = cfg.services.get("web").unwrap();
        assert_eq!(web.command, "echo hi");
        assert_eq!(web.name, None);
        assert!(web.depends_on.is_empty());
        assert!(matches!(web.ready_condition, ReadyCondition::Immediate));
    }

    #[test]
    fn parses_full_service_with_dependencies() {
        let cfg = parse_config(
            r#"
            [services.db]
            command = "postgres"
            ready_condition = { port = 5432 }

            [services.web]
            name = "webserver"
            command = "node server.js"
            ready_condition = { exitcode = 0 }
            depends_on = ["db"]
            "#,
        )
        .unwrap();

        assert_eq!(cfg.services.len(), 2);

        let db = cfg.services.get("db").unwrap();
        assert!(matches!(db.ready_condition, ReadyCondition::Port(5432)));

        let web = cfg.services.get("web").unwrap();
        assert_eq!(web.name.as_deref(), Some("webserver"));
        assert_eq!(web.depends_on, vec!["db".to_string()]);
        assert!(matches!(web.ready_condition, ReadyCondition::ExitCode(0)));
    }

    #[test]
    fn parses_empty_services_table() {
        let cfg = parse_config("[services]\n").unwrap();
        assert!(cfg.services.is_empty());
    }

    #[test]
    fn rejects_invalid_toml() {
        let result = parse_config("this is not valid toml [[[");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_missing_required_field() {
        let result = parse_config(
            r#"
            [services.web]
            ready_condition = "immediate"
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_ready_condition_variant() {
        let result = parse_config(
            r#"
            [services.web]
            command = "echo hi"
            ready_condition = "Bogus"
            "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn read_config_returns_file_contents() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kickstart_test_{}.toml", std::process::id()));
        std::fs::write(&path, "[services]\n").unwrap();

        let result = read_config(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(result, "[services]\n");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn read_config_errors_on_missing_file() {
        let result = read_config(Some("/nonexistent/path/kickstart.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn load_config_reads_and_parses_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kickstart_load_test_{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"
            [services.web]
            command = "echo hi"
            ready_condition = "immediate"
            "#,
        )
        .unwrap();

        let cfg = load_config(Some(path.to_str().unwrap())).unwrap();
        let web = cfg.services.get("web").unwrap();
        assert_eq!(web.command, "echo hi");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn load_config_errors_on_missing_file() {
        let result = load_config(Some("/nonexistent/path/kickstart.toml"));
        assert!(matches!(result, Err(LoadError::IoError(_))));
    }

    #[test]
    fn load_config_errors_on_invalid_toml() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kickstart_load_bad_{}.toml", std::process::id()));
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let result = load_config(Some(path.to_str().unwrap()));
        assert!(matches!(result, Err(LoadError::DeserializeError(_))));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn load_error_display_messages() {
        let io_err = LoadError::IoError(std::io::Error::new(std::io::ErrorKind::NotFound, "nope"));
        assert!(io_err.to_string().contains("failed to read config file"));

        let toml_err = parse_config("bad [[[").unwrap_err();
        let load_err = LoadError::from(toml_err);
        assert!(load_err.to_string().contains("failed to parse config file"));
    }
}
