use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    str::FromStr,
    sync::LazyLock,
};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

pub type Env = HashMap<String, String>;

static EXPR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\{([A-Z_][A-Z0-9_]*)(:-(.+))?\}$").unwrap());

#[derive(Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum Var<T> {
    Literal(T),
    Expr(String),
}

impl<'de, T> Deserialize<'de> for Var<T>
where
    T: Deserialize<'de> + FromStr,
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw<T> {
            Str(String),
            Other(T),
        }

        match Raw::<T>::deserialize(d)? {
            Raw::Str(s) if EXPR_RE.is_match(&s) => Ok(Var::Expr(s)),
            Raw::Str(s) => s
                .parse()
                .map(Var::Literal)
                .map_err(|_| D::Error::custom(format!("invalid value: {s:?}"))),
            Raw::Other(val) => Ok(Var::Literal(val)),
        }
    }
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
                if let Some(caps) = EXPR_RE.captures(expr) {
                    let env_var = caps
                        .get(1)
                        .map(|c| c.as_str())
                        .ok_or_else(|| ResolveError::InvalidExpr(expr.clone()))?;
                    let default = caps.get(3).map(|c| c.as_str());

                    if let Some(val) = env.get(env_var) {
                        val.parse()
                            .map_err(|_| ResolveError::EnvParseFail(val.clone()))
                    } else {
                        let default =
                            default.ok_or_else(|| ResolveError::InvalidExpr(expr.clone()))?;
                        default
                            .parse()
                            .map_err(|_| ResolveError::DefaultParseFail(default.to_string()))
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
#[serde(rename_all = "lowercase")]
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
    pub fn resolve(&self, env: &Env) -> Result<KickstartConfig, ResolveError> {
        let services = self
            .services
            .iter()
            .map(|(key, s)| {
                let service = Service {
                    name: s.name.clone(),
                    command: s.command.clone(),
                    ready_condition: s.ready_condition.resolve(env)?,
                    depends_on: s.depends_on.clone(),
                };
                Ok((key.clone(), service))
            })
            .collect::<Result<_, _>>()?;

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
    Io(std::io::Error),
    Deserialize(toml::de::Error),
    Resolve(ResolveError),
}

impl Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(err) => write!(f, "failed to read config file: {}", err),
            LoadError::Deserialize(err) => write!(f, "failed to parse config file: {}", err),
            LoadError::Resolve(err) => {
                write!(f, "failed to resolve environment variables: {}", err)
            }
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::de::Error> for LoadError {
    fn from(value: toml::de::Error) -> Self {
        Self::Deserialize(value)
    }
}

impl From<ResolveError> for LoadError {
    fn from(value: ResolveError) -> Self {
        Self::Resolve(value)
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

    fn resolve_config(cfg: &str, vars: &[(&str, &str)]) -> KickstartConfig {
        parse_config(cfg).unwrap().resolve(&env(vars)).unwrap()
    }

    #[test]
    fn parses_minimal_service() {
        let cfg = resolve_config(
            r#"
            [services.web]
            command = "echo hi"
            ready_condition = "immediate"
            "#,
            &[],
        );

        let web = cfg.services.get("web").unwrap();
        assert_eq!(web.command, "echo hi");
        assert_eq!(web.name, None);
        assert!(web.depends_on.is_empty());
        assert!(matches!(web.ready_condition, ReadyCondition::Immediate));
    }

    #[test]
    fn parses_full_service_with_dependencies() {
        let cfg = resolve_config(
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
            &[],
        );

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

        let cfg = load_config(Some(path.to_str().unwrap()), &Env::new()).unwrap();
        let web = cfg.services.get("web").unwrap();
        assert_eq!(web.command, "echo hi");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn load_config_errors_on_missing_file() {
        let result = load_config(Some("/nonexistent/path/kickstart.toml"), &Env::new());
        assert!(matches!(result, Err(LoadError::Io(_))));
    }

    #[test]
    fn load_config_errors_on_invalid_toml() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kickstart_load_bad_{}.toml", std::process::id()));
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let result = load_config(Some(path.to_str().unwrap()), &Env::new());
        assert!(matches!(result, Err(LoadError::Deserialize(_))));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn load_error_display_messages() {
        let io_err = LoadError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "nope"));
        assert!(io_err.to_string().contains("failed to read config file"));

        let toml_err = parse_config("bad [[[").unwrap_err();
        let load_err = LoadError::from(toml_err);
        assert!(load_err.to_string().contains("failed to parse config file"));
    }

    #[derive(Deserialize)]
    struct Holder<T: FromStr> {
        v: Var<T>,
    }

    fn var<T: for<'de> Deserialize<'de> + FromStr>(toml_value: &str) -> Var<T> {
        toml::from_str::<Holder<T>>(&format!("v = {toml_value}"))
            .unwrap()
            .v
    }

    fn env(vars: &[(&str, &str)]) -> Env {
        vars.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn string_var_distinguishes_literals_from_expressions() {
        assert!(matches!(var::<String>(r#""hello""#), Var::Literal(s) if s == "hello"));
        assert!(matches!(var::<String>(r#""{HOST}""#), Var::Expr(s) if s == "{HOST}"));
        assert!(matches!(
            var::<String>(r#""{HOST:-localhost}""#),
            Var::Expr(s) if s == "{HOST:-localhost}"
        ));
    }

    #[test]
    fn string_var_treats_non_matching_braces_as_literal() {
        assert!(matches!(var::<String>(r#""{lowercase}""#), Var::Literal(_)));
        assert!(matches!(
            var::<String>(r#""prefix {HOST}""#),
            Var::Literal(_)
        ));
    }

    #[test]
    fn numeric_var_accepts_numbers_strings_and_expressions() {
        assert!(matches!(var::<u16>("5432"), Var::Literal(5432)));
        assert!(matches!(var::<u16>(r#""5432""#), Var::Literal(5432)));
        assert!(matches!(var::<u16>(r#""{PORT}""#), Var::Expr(_)));
    }

    #[test]
    fn numeric_var_rejects_unparseable_strings() {
        let result = toml::from_str::<Holder<u16>>(r#"v = "not a port""#);
        assert!(result.is_err());
    }

    #[test]
    fn resolves_literal_without_env() {
        let v = var::<String>(r#""hello""#);
        assert_eq!(v.resolve(&Env::new()).unwrap(), "hello");
    }

    #[test]
    fn resolves_expression_from_env() {
        let v = var::<String>(r#""{HOST:-localhost}""#);
        assert_eq!(
            v.resolve(&env(&[("HOST", "db.internal")])).unwrap(),
            "db.internal"
        );
    }

    #[test]
    fn resolves_expression_to_default_when_unset() {
        let v = var::<String>(r#""{HOST:-localhost}""#);
        assert_eq!(v.resolve(&Env::new()).unwrap(), "localhost");

        let v = var::<u16>(r#""{PORT:-8080}""#);
        assert_eq!(v.resolve(&Env::new()).unwrap(), 8080);
    }

    #[test]
    fn errors_when_unset_without_default() {
        let v = var::<String>(r#""{HOST}""#);
        assert!(matches!(
            v.resolve(&Env::new()),
            Err(ResolveError::InvalidExpr(_))
        ));
    }

    #[test]
    fn errors_when_env_value_fails_to_parse() {
        let v = var::<u16>(r#""{PORT}""#);
        let result = v.resolve(&env(&[("PORT", "abc")]));
        assert!(matches!(result, Err(ResolveError::EnvParseFail(val)) if val == "abc"));
    }

    #[test]
    fn errors_when_default_fails_to_parse() {
        let v = var::<u16>(r#""{PORT:-abc}""#);
        let result = v.resolve(&Env::new());
        assert!(matches!(result, Err(ResolveError::DefaultParseFail(def)) if def == "abc"));
    }

    #[test]
    fn resolves_ready_conditions_from_env() {
        let cfg = resolve_config(
            r#"
            [services.db]
            command = "postgres"
            ready_condition = { port = "{DB_PORT:-5432}" }

            [services.web]
            command = "node server.js"
            ready_condition = { delay = "{WEB_DELAY}" }
            "#,
            &[("WEB_DELAY", "250")],
        );

        assert!(matches!(
            cfg.services["db"].ready_condition,
            ReadyCondition::Port(5432)
        ));
        assert!(matches!(
            cfg.services["web"].ready_condition,
            ReadyCondition::Delay(250)
        ));
    }

    #[test]
    fn resolve_error_surfaces_through_load_error() {
        let raw = parse_config(
            r#"
            [services.web]
            command = "true"
            ready_condition = { port = "{PORT}" }
            "#,
        )
        .unwrap();

        let Err(err) = raw.resolve(&Env::new()) else {
            panic!("expected resolve to fail");
        };
        let err = LoadError::from(err);
        assert!(
            err.to_string()
                .contains("failed to resolve environment variables")
        );
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
