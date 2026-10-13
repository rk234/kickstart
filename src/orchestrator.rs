use std::collections::{HashSet, VecDeque};

use crate::registry::{Registry, ServiceId};

enum ServiceStatus {
    Waiting(HashSet<ServiceId>),
    Ready,
    Running,
    /// Running and its ready condition has fired.
    Up,
    Ended,
    Skipped,
}

pub enum EndOutcome {
    Ended,
    Failed { skipped: Vec<ServiceId> },
}

pub struct Orchestrator {
    status: Vec<ServiceStatus>,
    dependents: Vec<Vec<ServiceId>>,
}

impl Orchestrator {
    pub fn new(registry: &Registry) -> Self {
        let mut status = Vec::with_capacity(registry.len());
        let mut dependents = vec![Vec::new(); registry.len()];

        for id in registry.ids() {
            let deps = registry.deps(id);

            // populate status
            status.push(if deps.is_empty() {
                ServiceStatus::Ready
            } else {
                ServiceStatus::Waiting(deps.iter().copied().collect())
            });

            // populate dependents (reverse edge: dependency -> services waiting on it)
            for dep in deps {
                dependents[dep.index()].push(id);
            }
        }

        Orchestrator { status, dependents }
    }

    pub fn mark_running(&mut self, srv: ServiceId) {
        self.status[srv.index()] = ServiceStatus::Running;
    }

    pub fn mark_ended(&mut self, srv: ServiceId) -> EndOutcome {
        let status = &mut self.status[srv.index()];
        match status {
            ServiceStatus::Up | ServiceStatus::Ended | ServiceStatus::Skipped => {
                *status = ServiceStatus::Ended;
                EndOutcome::Ended
            }
            ServiceStatus::Waiting(_) | ServiceStatus::Ready | ServiceStatus::Running => {
                EndOutcome::Failed {
                    skipped: self.mark_failed(srv),
                }
            }
        }
    }

    pub fn mark_failed(&mut self, srv: ServiceId) -> Vec<ServiceId> {
        self.status[srv.index()] = ServiceStatus::Ended;

        let mut skipped = Vec::new();
        let mut q: VecDeque<_> = self.dependents[srv.index()].iter().copied().collect();

        while let Some(srv) = q.pop_front() {
            if !self.mark_skipped(srv) {
                continue;
            }
            skipped.push(srv);
            q.extend(self.dependents[srv.index()].iter().copied());
        }

        skipped
    }

    fn mark_skipped(&mut self, srv: ServiceId) -> bool {
        let status = &mut self.status[srv.index()];
        if !matches!(status, ServiceStatus::Waiting(_) | ServiceStatus::Ready) {
            return false;
        }
        *status = ServiceStatus::Skipped;
        true
    }

    pub fn mark_ready(&mut self, srv: ServiceId) {
        let status = &mut self.status[srv.index()];
        if matches!(status, ServiceStatus::Running) {
            *status = ServiceStatus::Up;
        }

        for dependent in &self.dependents[srv.index()] {
            let status = &mut self.status[dependent.index()];
            if let ServiceStatus::Waiting(dependencies) = status {
                dependencies.remove(&srv);
                if dependencies.is_empty() {
                    *status = ServiceStatus::Ready
                }
            }
        }
    }

    pub fn skip_pending(&mut self) {
        for status in &mut self.status {
            match status {
                ServiceStatus::Waiting(_) | ServiceStatus::Ready => {
                    *status = ServiceStatus::Skipped
                }
                _ => continue,
            }
        }
    }

    pub fn fulfilled(&self) -> Vec<ServiceId> {
        self.status
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, ServiceStatus::Ready))
            .map(|(i, _)| ServiceId::new(i))
            .collect()
    }

    /// True once nothing is left waiting to start or become ready: every
    /// service is up, has ended, or was skipped.
    pub fn settled(&self) -> bool {
        self.status.iter().all(|status| {
            matches!(
                status,
                ServiceStatus::Up | ServiceStatus::Ended | ServiceStatus::Skipped
            )
        })
    }

    pub fn skipped(&self) -> Vec<ServiceId> {
        self.status
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, ServiceStatus::Skipped))
            .map(|(i, _)| ServiceId::new(i))
            .collect()
    }

    pub fn all_ended_or_skipped(&self) -> bool {
        self.status.iter().all(|status| {
            matches!(status, ServiceStatus::Ended) || matches!(status, ServiceStatus::Skipped)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{KickstartConfig, ReadyCondition, Service};

    fn service(depends_on: &[&str]) -> Service {
        Service {
            name: None,
            command: "true".to_owned(),
            ready_condition: ReadyCondition::Immediate,
            depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    /// Wraps an `Orchestrator` so tests can address services by name.
    struct Harness {
        orch: Orchestrator,
        registry: Registry<'static>,
    }

    impl Harness {
        fn id(&self, srv: &str) -> ServiceId {
            self.registry.lookup(srv).expect("unknown service")
        }

        fn names(&self, ids: Vec<ServiceId>) -> Vec<&'static str> {
            let mut names: Vec<_> = ids.into_iter().map(|id| self.registry.id(id)).collect();
            names.sort();
            names
        }

        fn mark_running(&mut self, srv: &str) {
            self.orch.mark_running(self.id(srv));
        }

        fn mark_ready(&mut self, srv: &str) {
            self.orch.mark_ready(self.id(srv));
        }

        fn mark_ended(&mut self, srv: &str) -> EndOutcome {
            self.orch.mark_ended(self.id(srv))
        }

        fn mark_failed(&mut self, srv: &str) -> Vec<ServiceId> {
            self.orch.mark_failed(self.id(srv))
        }

        fn skip_pending(&mut self) {
            self.orch.skip_pending();
        }

        fn fulfilled(&self) -> Vec<&'static str> {
            self.names(self.orch.fulfilled())
        }

        fn all_ended_or_skipped(&self) -> bool {
            self.orch.all_ended_or_skipped()
        }

        fn settled(&self) -> bool {
            self.orch.settled()
        }

        fn skipped(&self) -> Vec<&'static str> {
            self.names(self.orch.skipped())
        }
    }

    fn orchestrator(services: &[(&str, &[&str])]) -> Harness {
        let services = services
            .iter()
            .map(|(id, deps)| ((*id).to_owned(), service(deps)))
            .collect();
        let config = Box::leak(Box::new(KickstartConfig { services }));
        let registry = Registry::new(config);
        Harness {
            orch: Orchestrator::new(&registry),
            registry,
        }
    }

    fn fulfilled(orch: &Harness) -> Vec<&'static str> {
        orch.fulfilled()
    }

    /// Drives a service through the full lifecycle: spawned, ready, exited.
    fn run_to_completion(orch: &mut Harness, srv: &str) {
        orch.mark_running(srv);
        orch.mark_ready(srv);
        orch.mark_ended(srv);
    }

    // --- construction ---

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

    // --- releasing dependents ---

    #[test]
    fn marking_a_dependency_ready_releases_its_dependent() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["web"]);
    }

    #[test]
    fn dependent_waits_for_every_dependency() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[]), ("web", &["db", "cache"])]);

        orch.mark_running("db");
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["cache"]);

        orch.mark_running("cache");
        orch.mark_ready("cache");
        assert_eq!(fulfilled(&orch), vec!["web"]);
    }

    #[test]
    fn one_dependency_releases_all_of_its_dependents() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"]), ("worker", &["db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["web", "worker"]);
    }

    #[test]
    fn chains_are_released_one_level_at_a_time() {
        let mut orch = orchestrator(&[("a", &[]), ("b", &["a"]), ("c", &["b"])]);

        orch.mark_running("a");
        orch.mark_ready("a");
        assert_eq!(fulfilled(&orch), vec!["b"]);

        orch.mark_running("b");
        orch.mark_ready("b");
        assert_eq!(fulfilled(&orch), vec!["c"]);
    }

    #[test]
    fn diamond_dependencies_converge_on_the_last_dependency() {
        let mut orch =
            orchestrator(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);

        orch.mark_running("a");
        orch.mark_ready("a");
        assert_eq!(fulfilled(&orch), vec!["b", "c"]);

        orch.mark_running("b");
        orch.mark_ready("b");
        assert_eq!(fulfilled(&orch), vec!["c"]);

        orch.mark_running("c");
        orch.mark_ready("c");
        assert_eq!(fulfilled(&orch), vec!["d"]);
    }

    #[test]
    fn duplicate_dependencies_are_deduplicated() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db", "db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["web"]);
    }

    #[test]
    fn marking_the_same_dependency_ready_twice_is_idempotent() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[]), ("web", &["db", "cache"])]);

        orch.mark_running("db");
        orch.mark_ready("db");
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["cache"]);
    }

    #[test]
    fn marking_ready_does_not_change_the_services_own_status() {
        // `db` was never spawned, so it stays Ready and is still handed out.
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_ready("db");
        assert_eq!(fulfilled(&orch), vec!["db", "web"]);
    }

    #[test]
    fn dependency_cycles_leave_every_member_waiting() {
        let orch = orchestrator(&[("a", &["b"]), ("b", &["a"])]);
        assert!(orch.fulfilled().is_empty());
    }

    // --- mark_running ---

    #[test]
    fn marking_running_takes_a_service_out_of_fulfilled() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[])]);
        orch.mark_running("db");
        assert_eq!(fulfilled(&orch), vec!["cache"]);
    }

    #[test]
    fn marking_running_does_not_release_dependents() {
        // A spawned service only unblocks dependents once its ready condition fires.
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");
        assert!(orch.fulfilled().is_empty());
    }

    // --- mark_ended ---

    #[test]
    fn a_ready_service_stays_running_until_it_ends() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);

        orch.mark_running("db");
        orch.mark_ready("db");
        assert!(!orch.all_ended_or_skipped());

        orch.mark_ended("db");
        orch.mark_ended("web");
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn marking_ended_takes_a_service_out_of_fulfilled() {
        let mut orch = orchestrator(&[("db", &[]), ("cache", &[])]);
        orch.mark_ended("db");
        assert_eq!(fulfilled(&orch), vec!["cache"]);
    }

    #[test]
    fn ending_before_ready_fails_and_skips_dependents() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"]), ("app", &["web"])]);
        orch.mark_running("db");

        let EndOutcome::Failed { skipped } = orch.mark_ended("db") else {
            panic!("db ended before ready but was not failed");
        };
        assert_eq!(orch.names(skipped), ["app", "web"]);
        assert!(orch.fulfilled().is_empty());
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn ending_after_ready_does_not_skip_dependents() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");

        assert!(matches!(orch.mark_ended("db"), EndOutcome::Ended));
        assert_eq!(fulfilled(&orch), vec!["web"]);
    }

    #[test]
    fn a_failed_service_without_dependents_still_reports_failure() {
        let mut orch = orchestrator(&[("db", &[])]);
        orch.mark_running("db");
        assert!(matches!(
            orch.mark_ended("db"),
            EndOutcome::Failed { skipped } if skipped.is_empty()
        ));
    }

    #[test]
    fn marking_ended_twice_is_idempotent() {
        let mut orch = orchestrator(&[("db", &[])]);
        orch.mark_ended("db");
        orch.mark_ended("db");
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn a_waiting_service_can_be_ended_without_ever_running() {
        // e.g. the run is torn down before a dependency ever became ready.
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);

        orch.mark_ended("db");
        orch.mark_ended("web");

        assert!(orch.all_ended_or_skipped());
    }

    // --- mark_failed ---

    #[test]
    fn failing_skips_transitive_dependents() {
        let mut orch = orchestrator(&[("a", &[]), ("b", &["a"]), ("c", &["b"])]);
        orch.mark_running("a");
        orch.mark_failed("a");
        assert!(orch.fulfilled().is_empty());
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn failing_does_not_skip_running_dependents() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");
        orch.mark_running("web");

        orch.mark_failed("db");

        // `web` is still running, so the run is not complete.
        assert!(!orch.all_ended_or_skipped());
    }

    #[test]
    fn failing_terminates_on_dependency_cycles() {
        let mut orch = orchestrator(&[("x", &[]), ("a", &["b", "x"]), ("b", &["a"])]);
        orch.mark_running("x");
        orch.mark_failed("x");
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn failing_skips_diamond_dependents() {
        let mut orch =
            orchestrator(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
        orch.mark_running("a");
        orch.mark_failed("a");
        assert!(orch.all_ended_or_skipped());
    }

    // --- all_ended ---

    #[test]
    fn all_ended_is_vacuously_true_without_services() {
        let orch = orchestrator(&[]);
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn all_ended_is_false_while_anything_is_waiting_or_ready() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        assert!(!orch.all_ended_or_skipped());

        run_to_completion(&mut orch, "db");
        // `web` is Ready but has not run yet.
        assert!(!orch.all_ended_or_skipped());
    }

    #[test]
    fn all_ended_is_false_while_a_service_is_running() {
        let mut orch = orchestrator(&[("db", &[])]);
        orch.mark_running("db");
        assert!(!orch.all_ended_or_skipped());
    }

    #[test]
    fn all_ended_is_true_once_every_service_has_run_to_completion() {
        let mut orch = orchestrator(&[("a", &[]), ("b", &["a"]), ("c", &["b"])]);

        run_to_completion(&mut orch, "a");
        run_to_completion(&mut orch, "b");
        assert!(!orch.all_ended_or_skipped());

        run_to_completion(&mut orch, "c");
        assert!(orch.all_ended_or_skipped());
    }

    // --- skip_pending ---

    #[test]
    fn skip_pending_skips_waiting_services() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"]), ("worker", &["web"])]);
        orch.mark_running("db");

        orch.skip_pending();

        assert!(fulfilled(&orch).is_empty());
        // `db` is still running, so the run is not over yet.
        assert!(!orch.all_ended_or_skipped());
    }

    #[test]
    fn skip_pending_lets_the_run_finish_once_running_services_end() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");
        orch.mark_running("web");
        orch.mark_ready("web");

        orch.skip_pending();
        assert!(!orch.all_ended_or_skipped());

        orch.mark_ended("db");
        orch.mark_ended("web");
        assert!(orch.all_ended_or_skipped());
    }

    #[test]
    fn skipped_services_are_not_released_by_a_late_ready() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");

        orch.skip_pending();
        // `db` comes up after shutdown began: `web` must stay skipped.
        orch.mark_ready("db");

        assert!(fulfilled(&orch).is_empty());
    }

    // --- settled ---

    #[test]
    fn not_settled_while_services_wait_or_are_starting() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        assert!(!orch.settled());

        orch.mark_running("db");
        // `db` is spawned but not ready yet, and `web` is still waiting on it.
        assert!(!orch.settled());

        orch.mark_ready("db");
        assert!(!orch.settled());
    }

    #[test]
    fn settled_once_every_service_is_up() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"])]);
        orch.mark_running("db");
        orch.mark_ready("db");
        orch.mark_running("web");
        orch.mark_ready("web");
        assert!(orch.settled());
        assert!(orch.skipped().is_empty());
    }

    #[test]
    fn settled_after_a_failure_reports_the_skipped_services() {
        let mut orch = orchestrator(&[("db", &[]), ("web", &["db"]), ("app", &["web"])]);
        orch.mark_running("db");
        orch.mark_ended("db");
        assert!(orch.settled());
        assert_eq!(orch.skipped(), ["app", "web"]);
    }

    #[test]
    fn services_that_exited_after_becoming_ready_count_as_settled() {
        let mut orch = orchestrator(&[("migrate", &[])]);
        run_to_completion(&mut orch, "migrate");
        assert!(orch.settled());
    }
}
