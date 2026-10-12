use crate::config::{KickstartConfig, Service};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceId(usize);

impl ServiceId {
    pub const fn new(index: usize) -> Self {
        Self(index)
    }

    pub fn index(self) -> usize {
        self.0
    }
}

pub struct Registry<'a> {
    ids: Vec<&'a str>,
    services: Vec<&'a Service>,
    deps: Vec<Vec<ServiceId>>,
}

impl<'a> Registry<'a> {
    pub fn new(config: &'a KickstartConfig) -> Self {
        let mut entries: Vec<_> = config
            .services
            .iter()
            .map(|(id, srv)| (id.as_str(), srv))
            .collect();
        entries.sort_unstable_by_key(|(id, _)| *id);
        let (ids, services): (Vec<_>, Vec<_>) = entries.into_iter().unzip();

        let mut registry = Self {
            ids,
            services,
            deps: Vec::new(),
        };
        registry.deps = registry
            .services
            .iter()
            .map(|srv| {
                srv.depends_on
                    .iter()
                    .filter_map(|dep| {
                        let id = registry.lookup(dep);
                        debug_assert!(id.is_some(), "undefined dependency {dep:?}");
                        id
                    })
                    .collect()
            })
            .collect();
        registry
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn ids(&self) -> impl Iterator<Item = ServiceId> + use<> {
        (0..self.len()).map(ServiceId)
    }

    pub fn lookup(&self, id: &str) -> Option<ServiceId> {
        self.ids.binary_search(&id).ok().map(ServiceId)
    }

    pub fn id(&self, srv: ServiceId) -> &'a str {
        self.ids[srv.0]
    }

    pub fn service(&self, srv: ServiceId) -> &'a Service {
        self.services[srv.0]
    }

    pub fn deps(&self, srv: ServiceId) -> &[ServiceId] {
        &self.deps[srv.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_config;

    fn config() -> KickstartConfig {
        parse_config(
            r#"
            [services.web]
            command = "true"
            ready_condition = "immediate"
            depends_on = ["db", "cache"]

            [services.db]
            command = "true"
            ready_condition = "immediate"

            [services.cache]
            command = "true"
            ready_condition = "immediate"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn ids_are_sorted() {
        let config = config();
        let registry = Registry::new(&config);
        let ids: Vec<_> = registry.ids().map(|id| registry.id(id)).collect();
        assert_eq!(ids, ["cache", "db", "web"]);
    }

    #[test]
    fn lookup_round_trips() {
        let config = config();
        let registry = Registry::new(&config);
        for id in registry.ids() {
            assert_eq!(registry.lookup(registry.id(id)), Some(id));
        }
        assert_eq!(registry.lookup("nope"), None);
    }

    #[test]
    fn dependencies_resolve_to_their_services() {
        let config = config();
        let registry = Registry::new(&config);
        let web = registry.lookup("web").unwrap();
        let deps: Vec<_> = registry
            .deps(web)
            .iter()
            .map(|&dep| registry.id(dep))
            .collect();
        assert_eq!(deps, ["db", "cache"]);
        assert_eq!(registry.service(web).command, "true");
    }
}
