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
