use std::{
    collections::HashMap,
    io::{self, Write},
};

use anstyle::{AnsiColor, Style};
use serde::Serialize;

use crate::{
    config::{KickstartConfig, ReadyCondition, Service},
    log::{Level, Logger, palette::Palette},
};

const HEADER: [&str; 4] = ["STAGE", "SERVICE", "READY", "DEPENDS"];

#[derive(Serialize, Debug)]
struct Row<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    stage: Option<usize>,
    ready_condition: ReadyCondition,
    depends_on: &'a [String],
}

pub fn run(config: &KickstartConfig, json: bool) {
    let stages = stages(&config.services);
    let rows = rows(&config.services, &stages);

    let mut out = anstream::stdout();
    let _ = if json {
        serde_json::to_writer_pretty(&mut out, &rows)
            .map_err(io::Error::from)
            .and_then(|()| writeln!(out))
    } else {
        render_table(&mut out, &rows)
    };

    let mut logger = Logger::stdout();
    for warning in config.validate() {
        logger.log("error", Level::Error, warning.to_string().as_str());
    }
}

fn stages(services: &HashMap<String, Service>) -> HashMap<&str, usize> {
    let mut stages = HashMap::new();
    loop {
        let mut progressed = false;
        for (id, srv) in services {
            if stages.contains_key(id.as_str()) {
                continue;
            }
            let stage_of_deps: Option<Vec<usize>> = srv
                .depends_on
                .iter()
                .map(|dep| stages.get(dep.as_str()).copied())
                .collect();
            if let Some(stage_of_deps) = stage_of_deps {
                stages.insert(
                    id.as_str(),
                    stage_of_deps.into_iter().max().unwrap_or(0) + 1,
                );
                progressed = true;
            }
        }
        if !progressed {
            return stages;
        }
    }
}

fn rows<'a>(services: &'a HashMap<String, Service>, stages: &HashMap<&str, usize>) -> Vec<Row<'a>> {
    let mut rows: Vec<_> = services
        .iter()
        .map(|(id, srv)| Row {
            id,
            name: srv.name.as_deref(),
            stage: stages.get(id.as_str()).copied(),
            ready_condition: srv.ready_condition,
            depends_on: &srv.depends_on,
        })
        .collect();
    rows.sort_by_key(|row| (row.stage.unwrap_or(usize::MAX), row.id));
    rows
}

fn ready_label(cond: ReadyCondition) -> String {
    match cond {
        ReadyCondition::Port(port) => format!("port:{port}"),
        ReadyCondition::ExitCode(code) => format!("exit:{code}"),
        ReadyCondition::Delay(ms) => format!("delay:{ms}ms"),
        ReadyCondition::Immediate => "immediate".to_owned(),
    }
}

struct Cell {
    plain: String,
    styled: String,
}

impl Cell {
    fn new(text: &str, style: Style) -> Self {
        Self {
            plain: text.to_owned(),
            styled: format!("{style}{text}{style:#}"),
        }
    }
}

fn render_table<W: Write>(out: &mut W, rows: &[Row]) -> io::Result<()> {
    let mut palette = Palette::default();
    let mut ids: Vec<_> = rows.iter().map(|row| row.id).collect();
    ids.sort_unstable();
    for id in &ids {
        palette.style_for(id);
    }

    let red = Style::new().fg_color(Some(AnsiColor::Red.into()));
    let dim = Style::new().dimmed();

    let mut table = vec![HEADER.map(|title| Cell::new(title, dim))];
    for row in rows {
        let stage = match row.stage {
            Some(stage) => Cell::new(&stage.to_string(), Style::new()),
            None => Cell::new("!", red),
        };
        let deps = if row.depends_on.is_empty() {
            Cell::new("-", dim)
        } else {
            let styled: Vec<_> = row
                .depends_on
                .iter()
                .map(|dep| {
                    let style = if ids.contains(&dep.as_str()) {
                        palette.style_for(dep)
                    } else {
                        red
                    };
                    format!("{style}{dep}{style:#}")
                })
                .collect();
            Cell {
                plain: row.depends_on.join(","),
                styled: styled.join(","),
            }
        };
        table.push([
            stage,
            Cell::new(row.id, palette.style_for(row.id)),
            Cell::new(&ready_label(row.ready_condition), Style::new()),
            deps,
        ]);
    }

    let mut widths = [0; HEADER.len()];
    for cells in &table {
        for (width, cell) in widths.iter_mut().zip(cells) {
            *width = (*width).max(cell.plain.chars().count());
        }
    }

    for cells in &table {
        let last = cells.len() - 1;
        for (i, cell) in cells.iter().enumerate() {
            write!(out, "{}", cell.styled)?;
            if i < last {
                let pad = widths[i] - cell.plain.chars().count() + 2;
                write!(out, "{:pad$}", "")?;
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(ready_condition: ReadyCondition, depends_on: &[&str]) -> Service {
        Service {
            name: None,
            command: "true".to_owned(),
            ready_condition,
            depends_on: depends_on.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn services(defs: &[(&str, &[&str])]) -> HashMap<String, Service> {
        defs.iter()
            .map(|(id, deps)| (id.to_string(), service(ReadyCondition::Immediate, deps)))
            .collect()
    }

    fn stage_of(services: &HashMap<String, Service>, id: &str) -> Option<usize> {
        stages(services).get(id).copied()
    }

    fn table(services: &HashMap<String, Service>) -> String {
        let stages = stages(services);
        let mut buf = Vec::new();
        render_table(&mut buf, &rows(services, &stages)).unwrap();

        let mut plain = Vec::new();
        anstream::StripStream::new(&mut plain)
            .write_all(&buf)
            .unwrap();
        String::from_utf8(plain).unwrap()
    }

    #[test]
    fn services_without_dependencies_are_stage_one() {
        let srvs = services(&[("db", &[]), ("cache", &[])]);
        assert_eq!(stage_of(&srvs, "db"), Some(1));
        assert_eq!(stage_of(&srvs, "cache"), Some(1));
    }

    #[test]
    fn stage_follows_the_latest_dependency() {
        let srvs = services(&[("db", &[]), ("api", &["db"]), ("worker", &["db", "api"])]);
        assert_eq!(stage_of(&srvs, "api"), Some(2));
        assert_eq!(stage_of(&srvs, "worker"), Some(3));
    }

    #[test]
    fn undefined_dependencies_never_start_nor_do_their_dependents() {
        let srvs = services(&[("api", &["dbb"]), ("web", &["api"])]);
        assert_eq!(stage_of(&srvs, "api"), None);
        assert_eq!(stage_of(&srvs, "web"), None);
    }

    #[test]
    fn cycles_never_start() {
        let srvs = services(&[("a", &["b"]), ("b", &["a"]), ("c", &[])]);
        assert_eq!(stage_of(&srvs, "a"), None);
        assert_eq!(stage_of(&srvs, "c"), Some(1));
    }

    #[test]
    fn table_is_sorted_by_stage_then_id_with_unstartable_services_last() {
        let mut srvs = HashMap::new();
        srvs.insert(
            "backend".to_owned(),
            service(ReadyCondition::Port(8000), &[]),
        );
        srvs.insert(
            "seed".to_owned(),
            service(ReadyCondition::ExitCode(0), &["backend"]),
        );
        srvs.insert(
            "frontend".to_owned(),
            service(ReadyCondition::Delay(500), &["backend", "seed"]),
        );
        srvs.insert(
            "orphan".to_owned(),
            service(ReadyCondition::Immediate, &["ghost"]),
        );

        assert_eq!(
            table(&srvs),
            "\
STAGE  SERVICE   READY        DEPENDS
1      backend   port:8000    -
2      seed      exit:0       backend
3      frontend  delay:500ms  backend,seed
!      orphan    immediate    ghost
"
        );
    }

    #[test]
    fn json_includes_unstartable_services_with_a_null_stage() {
        let mut srvs = services(&[("api", &["ghost"])]);
        srvs.get_mut("api").unwrap().name = Some("API".to_owned());
        let stages = stages(&srvs);
        let json = serde_json::to_value(rows(&srvs, &stages)).unwrap();
        assert_eq!(
            json,
            serde_json::json!([{
                "id": "api",
                "name": "API",
                "stage": null,
                "ready_condition": "immediate",
                "depends_on": ["ghost"],
            }])
        );
    }
}
