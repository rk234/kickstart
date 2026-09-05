use std::collections::{HashMap, HashSet};

use crate::config::Service;

enum ServiceStatus {
    Waiting(HashSet<String>),
    Ready,
    Running,
    Ended,
}

struct Orchestrator {
    srv_to_status: HashMap<String, ServiceStatus>,
    srv_to_dependents: HashMap<String, Vec<String>>,
}

impl Orchestrator {
    pub fn new(services: &HashMap<String, Service>) -> Self {
        let mut srv_to_status = HashMap::new();
        let mut srv_to_dependents: HashMap<String, Vec<String>> = HashMap::new();

        for (id, srv) in services {
            let deps = &srv.depends_on;

            // populate status
            let status = if deps.is_empty() {
                ServiceStatus::Ready
            } else {
                ServiceStatus::Waiting(deps.iter().cloned().collect())
            };
            srv_to_status.insert(id.clone(), status);

            // populate dependents (reverse edge: dependency -> services waiting on it)
            for dep in deps {
                srv_to_dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(id.clone());
            }
        }

        Orchestrator {
            srv_to_status,
            srv_to_dependents,
        }
    }

    pub fn mark_ready(&mut self, srv: &str) {
        let Some(dependents) = self.srv_to_dependents.get(srv) else {
            return;
        };

        for dependent in dependents {
            let Some(status) = self.srv_to_status.get_mut(dependent) else {
                continue;
            };
            if let ServiceStatus::Waiting(dependencies) = status {
                dependencies.remove(srv);
                if dependencies.is_empty() {
                    *status = ServiceStatus::Ready
                }
            }
        }
    }

    pub fn fulfilled(&self) -> Vec<String> {
        self.srv_to_status
            .iter()
            .filter(|(_, s)| matches!(s, ServiceStatus::Ready))
            .map(|(s, _)| s)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ReadyCondition;

    fn service(depends_on: &[&str]) -> Service {
        Service {
            name: None,
            command: "true".to_owned(),
            ready_condition: ReadyCondition::Immediate,
            depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    fn orchestrator(services: &[(&str, &[&str])]) -> Orchestrator {
        let services = services
            .iter()
            .map(|(id, deps)| ((*id).to_owned(), service(deps)))
            .collect();
        Orchestrator::new(&services)
    }

    fn fulfilled(orch: &Orchestrator) -> Vec<String> {
        let mut ids = orch.fulfilled();
        ids.sort();
        ids
    }

    #[test]
    fn empty_config_has_nothing_fulfilled() {
        let orch = orchestrator(&[]);
        assert!(orch.fulfilled().is_empty());
    }

    #[test]
    fn services_without_dependencies_start_ready() {
        let orch = orchestrator(&[("db", &[]), ("cache", &[])]);
        assert_eq!(fulfilled(&orch), vec!["cache", "db"]);
    }

    #[test]
    fn services_with_dependencies_start_waiting() {
        let orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        assert_eq!(fulfilled(&orch), vec!["db"]);
    }

    #[test]
    fn marking_a_dependency_ready_releases_its_dependent() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["db", "web"]);
    }

    #[test]
    fn dependent_waits_for_every_dependency() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[]), ("web", &["db", "cache"])]);

        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["cache", "db"]);

        orch.mark_ready("cache");
        assert_eq!(fulfilled(&orch), vec!["cache", "db", "web"]);
    }

    #[test]
    fn one_dependency_releases_all_of_its_dependents() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"]), ("worker", &["db"])]);
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["db", "web", "worker"]);
    }

    #[test]
    fn chains_are_released_one_level_at_a_time() {
        let mut orch = orchestrator(&[("a", &[]), ("b", &["a"]), ("c", &["b"])]);

        orch.mark_ready("a");
        assert_eq!(fulfilled(&orch), vec!["a", "b"]);

        orch.mark_ready("b");
        assert_eq!(fulfilled(&orch), vec!["a", "b", "c"]);
    }

    #[test]
    fn diamond_dependencies_converge_on_the_last_dependency() {
        let mut orch = orchestrator(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b", "c"]),
        ]);

        orch.mark_ready("a");
        assert_eq!(fulfilled(&orch), vec!["a", "b", "c"]);

        orch.mark_ready("b");
        assert_eq!(fulfilled(&orch), vec!["a", "b", "c"]);

        orch.mark_ready("c");
        assert_eq!(fulfilled(&orch), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn duplicate_dependencies_are_deduplicated() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db", "db"])]);
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["db", "web"]);
    }

    #[test]
    fn marking_the_same_dependency_twice_is_idempotent() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[]), ("web", &["db", "cache"])]);

        orch.mark_ready("db");
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["cache", "db"]);
    }

    #[test]
    fn marking_an_unknown_service_ready_is_a_no_op() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_ready("nope");
        assert_eq!(fulfilled(&orch), vec!["db"]);
    }

    #[test]
    fn marking_a_service_with_no_dependents_is_a_no_op() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[])]);
        orch.mark_ready("cache");
        assert_eq!(fulfilled(&orch), vec!["cache", "db"]);
    }

    #[test]
    fn dependency_on_an_undeclared_service_never_resolves() {
        let mut orch = orchestrator(&[("web", &["ghost"])]);
        assert!(orch.fulfilled().is_empty());

        orch.mark_ready("web");
        assert!(orch.fulfilled().is_empty());
    }

    #[test]
    fn undeclared_dependency_can_still_be_marked_ready() {
        // `ghost` is never registered as a service, but the reverse edge exists,
        // so marking it ready still releases anything waiting on it.
        let mut orch = orchestrator(&[("web", &["ghost"])]);
        orch.mark_ready("ghost");
        assert_eq!(fulfilled(&orch), vec!["web"]);
    }

    #[test]
    fn dependency_cycles_leave_every_member_waiting() {
        let orch = orchestrator(&[("a", &["b"]), ("b", &["a"])]);
        assert!(orch.fulfilled().is_empty());
    }
}
