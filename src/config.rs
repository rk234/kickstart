use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    str::FromStr,
};

use serde::{Deserialize, Serialize};

pub type Env = HashMap<String, String>;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Var<T: FromStr + Clone> {
    Literal(T),
    Expr(String),
}

#[derive(Debug)]
pub enum ResolveError {
    EnvParseFail(String),
    DefaultParseFail(String),
    InvalidExpr(String),
}

impl Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ResolveError::EnvParseFail(env) =>
                    format!("failed to parse environment variable: {}", env),
                ResolveError::DefaultParseFail(def) => format!("failed to parse default: {}", def),
                ResolveError::InvalidExpr(expr) => format!("invalid expression: {}", expr),
            }
        )
    }
}

impl<T: FromStr + Clone> Var<T> {
    pub fn resolve(&self, env: &Env) -> Result<T, ResolveError> {
        match self {
            Var::Literal(val) => Ok(val.clone()),
            Var::Expr(expr) => {
                let re = Regex::new(r"^{([A-Z_][A-Z0-9_]*)(:-(.+))?}$").unwrap();

                if let Some(caps) = re.captures(expr) {
                    let env_var = caps
                        .get(1)
                        .map(|c| c.as_str())
                        .ok_or(ResolveError::InvalidExpr(expr.clone()))?;
                    let default = caps.get(2).map(|c| c.as_str());

                    if let Some(val) = env.get(env_var) {
                        let result = val
                            .parse()
                            .ok()
                            .ok_or(ResolveError::EnvParseFail(val.clone()))?;

                        Ok(result)
                    } else {
                        let default = default.ok_or(ResolveError::InvalidExpr(expr.clone()))?;
                        let default_val = default
                            .parse()
                            .ok()
                            .ok_or(ResolveError::DefaultParseFail(String::from(default)))?;

                        Ok(default_val)
                    }
                } else {
                    Err(ResolveError::InvalidExpr(expr.clone()))
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum RawReadyCondition {
    ExitCode(Var<i32>),
    Port(Var<u16>),
    Delay(Var<u64>),
    Immediate,
}

impl RawReadyCondition {
    fn resolve(&self, env: &Env) -> Result<ReadyCondition, ResolveError> {
        Ok(match self {
            RawReadyCondition::ExitCode(var) => ReadyCondition::ExitCode(var.resolve(env)?),
            RawReadyCondition::Port(var) => ReadyCondition::Port(var.resolve(env)?),
            RawReadyCondition::Delay(var) => ReadyCondition::Delay(var.resolve(env)?),
            RawReadyCondition::Immediate => ReadyCondition::Immediate,
        })
    }
}

#[derive(Serialize, Debug, Clone, Copy)]
pub enum ReadyCondition {
    ExitCode(i32),
    Port(u16),
    Delay(u64),
    Immediate,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RawService {
    pub name: Option<String>,
    pub command: String,
    pub ready_condition: RawReadyCondition,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

#[derive(Debug)]
pub struct Service {
    pub name: Option<String>,
    pub command: String,
    pub ready_condition: ReadyCondition,
    pub depends_on: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RawKickstartConfig {
    pub services: HashMap<String, RawService>,
}

impl RawKickstartConfig {
    fn resolve(&self, env: &Env) -> Result<KickstartConfig, ResolveError> {
        let services: HashMap<String, Service> = self
            .services
            .iter()
            .map::<Result<(String, Service), ResolveError>, _>(|(key, s)| {
                let cond = s.ready_condition.resolve(env)?;
                Ok((
                    key.clone(),
                    Service {
                        name: s.name.clone(),
                        command: s.command.clone(),
                        ready_condition: cond,
                        depends_on: s.depends_on.clone(),
                    },
                ))
            })
            .collect::<Result<HashMap<String, Service>, ResolveError>>()?;

        Ok(KickstartConfig { services })
    }
}

pub struct KickstartConfig {
    pub services: HashMap<String, Service>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigWarning {
    UndefinedDependency { service: String, dependency: String },
    Cycle(Vec<String>),
}

impl Display for ConfigWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UndefinedDependency {
                service,
                dependency,
            } => write!(
                f,
                "{service} depends on \"{dependency}\", which isn't defined"
            ),
            Self::Cycle(services) => write!(
                f,
                "dependency cycle {} → {}: these services will never start",
                services.join(" → "),
                services[0]
            ),
        }
    }
}

impl KickstartConfig {
    pub fn validate(&self) -> Vec<ConfigWarning> {
        let mut warnings = self.undefined_dependencies();
        warnings.extend(self.cycles());
        warnings
    }

    fn sorted_ids(&self) -> Vec<&str> {
        let mut ids: Vec<_> = self.services.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }

    fn undefined_dependencies(&self) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();
        for id in self.sorted_ids() {
            let mut seen = HashSet::new();
            for dep in &self.services[id].depends_on {
                if !self.services.contains_key(dep) && seen.insert(dep) {
                    warnings.push(ConfigWarning::UndefinedDependency {
                        service: id.to_owned(),
                        dependency: dep.clone(),
                    });
                }
            }
        }
        warnings
    }

    fn cycles(&self) -> Vec<ConfigWarning> {
        let mut visited = HashSet::new();
        let mut cycles = Vec::new();
        for id in self.sorted_ids() {
            self.find_cycles(id, &mut Vec::new(), &mut visited, &mut cycles);
        }
        cycles
    }

    fn find_cycles<'a>(
        &'a self,
        id: &'a str,
        path: &mut Vec<&'a str>,
        visited: &mut HashSet<&'a str>,
        cycles: &mut Vec<ConfigWarning>,
    ) {
        if let Some(start) = path.iter().position(|&srv| srv == id) {
            let cycle = path[start..].iter().map(|&srv| srv.to_owned()).collect();
            cycles.push(ConfigWarning::Cycle(cycle));
            return;
        }
        if !visited.insert(id) {
            return;
        }
        // Undefined services are reported by `undefined_dependencies`.
        let Some(srv) = self.services.get(id) else {
            return;
        };

        path.push(id);
        for dep in &srv.depends_on {
            self.find_cycles(dep, path, visited, cycles);
        }
        path.pop();
    }
}

#[derive(Debug)]
pub enum LoadError {
    IoError(std::io::Error),
    DeserializeError(toml::de::Error),
    ResolveError(ResolveError),
}

impl Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::IoError(err) => write!(f, "failed to read config file: {}", err),
            LoadError::DeserializeError(err) => write!(f, "failed to parse config file: {}", err),
            LoadError::ResolveError(err) => {
                write!(f, "failed to resolve environment variables: {}", err)
            }
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        Self::IoError(value)
    }
}

impl From<toml::de::Error> for LoadError {
    fn from(value: toml::de::Error) -> Self {
        Self::DeserializeError(value)
    }
}

impl From<ResolveError> for LoadError {
    fn from(value: ResolveError) -> Self {
        Self::ResolveError(value)
    }
}

const DEFAULT_CONFIG_PATH: &str = "kickstart.toml";

pub fn read_config(path: Option<&str>) -> Result<String, std::io::Error> {
    std::fs::read_to_string(path.unwrap_or(DEFAULT_CONFIG_PATH))
}

pub fn parse_config(cfg: &str) -> Result<RawKickstartConfig, toml::de::Error> {
    let config = toml::from_str(cfg)?;
    Ok(config)
}

pub fn load_config(cfg: Option<&str>, env: &Env) -> Result<KickstartConfig, LoadError> {
    let content = read_config(cfg)?;
    let cfg = parse_config(content.as_str())?;
    let resolved = cfg.resolve(env)?;
    Ok(resolved)
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

    fn graph(defs: &[(&str, &[&str])]) -> KickstartConfig {
        let services = defs
            .iter()
            .map(|(id, deps)| {
                let srv = Service {
                    name: None,
                    command: "true".to_owned(),
                    ready_condition: ReadyCondition::Immediate,
                    depends_on: deps.iter().map(|d| d.to_string()).collect(),
                };
                (id.to_string(), srv)
            })
            .collect();
        KickstartConfig { services }
    }

    fn cycle(services: &[&str]) -> ConfigWarning {
        ConfigWarning::Cycle(services.iter().map(|s| s.to_string()).collect())
    }

    fn undefined(service: &str, dependency: &str) -> ConfigWarning {
        ConfigWarning::UndefinedDependency {
            service: service.to_owned(),
            dependency: dependency.to_owned(),
        }
    }

    #[test]
    fn valid_graph_has_no_warnings() {
        let cfg = graph(&[("db", &[]), ("api", &["db"]), ("web", &["api", "db"])]);
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn reports_undefined_dependencies_once_per_service() {
        let cfg = graph(&[("api", &["dbb", "dbb"]), ("web", &["dbb", "api"])]);
        assert_eq!(
            cfg.validate(),
            [undefined("api", "dbb"), undefined("web", "dbb")]
        );
    }

    #[test]
    fn reports_a_two_service_cycle_once() {
        let cfg = graph(&[("a", &["b"]), ("b", &["a"]), ("c", &["a"])]);
        assert_eq!(cfg.validate(), [cycle(&["a", "b"])]);
    }

    #[test]
    fn reports_self_dependency_as_a_cycle() {
        let cfg = graph(&[("a", &["a"])]);
        assert_eq!(cfg.validate(), [cycle(&["a"])]);
    }

    #[test]
    fn reports_only_the_services_in_the_cycle() {
        let cfg = graph(&[
            ("entry", &["x"]),
            ("x", &["y"]),
            ("y", &["z"]),
            ("z", &["x"]),
        ]);
        assert_eq!(cfg.validate(), [cycle(&["x", "y", "z"])]);
    }

    #[test]
    fn reports_undefined_dependencies_before_cycles() {
        let cfg = graph(&[("a", &["b"]), ("b", &["a"]), ("c", &["ghost"])]);
        assert_eq!(
            cfg.validate(),
            [undefined("c", "ghost"), cycle(&["a", "b"])]
        );
    }

    #[test]
    fn warning_messages() {
        assert_eq!(
            undefined("api", "dbb").to_string(),
            "api depends on \"dbb\", which isn't defined"
        );
        assert_eq!(
            cycle(&["a", "b"]).to_string(),
            "dependency cycle a → b → a: these services will never start"
        );
    }
}
