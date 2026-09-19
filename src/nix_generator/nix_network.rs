use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::error::{NixError, Result};
use crate::nix_generator::item::host::ServiceParams;
use crate::nix_generator::nix_service::{NixService, ServiceRegistry};
use crate::nix_generator::nix_zone::{NixZone, EXTERNAL_ZONE_KEY};
use crate::nix_generator::schema::{
    Coordination, FleetUpdate, Matrix, NetworkCfg, NetworkDefault, Smtp,
};
use crate::nix_generator::validation::{
    assert_email, assert_profile_list, assert_regex, assert_seconds, assert_timeout_key, RE_FQDN,
    RE_HOSTNAME, RE_LOCALE, RE_SMTP_PROTOCOL, RE_TIMEZONE,
};

const DEFAULT_DOMAIN: &str = "darkone.lan";
const DEFAULT_LOCALE: &str = "fr_FR.UTF-8";
const DEFAULT_TIMEZONE: &str = "Europe/Paris";
const DEFAULT_COORDINATION_DOMAIN: &str = "headscale";

#[derive(Debug, Default)]
pub struct NetworkConfig {
    pub domain: String,
    pub default_locale: String,
    pub default_timezone: String,
    pub coordination_domain: String,
    pub coordination_hostname: String,
    pub coordination_enable: bool,
    /// Typed blocks kept verbatim for `network.nix` emission (only the keys
    /// actually present in the config are emitted).
    pub default: Option<NetworkDefault>,
    pub coordination: Option<Coordination>,
    pub smtp: Option<Smtp>,
    pub matrix: Option<Matrix>,
    pub fleet_update: Option<FleetUpdate>,
}

#[derive(Debug, Default)]
pub struct NixNetwork {
    pub config: NetworkConfig,
    pub zones: HashMap<String, NixZone>,
    /// Services in declaration order (IndexMap preserves insertion order)
    services: IndexMap<String, NixService>,
    /// Track unique services per zone: (zone, service_name) -> bool
    uniq_services: HashMap<(String, String), bool>,
    /// Track global service domain names to detect conflicts
    global_service_domains: HashSet<String>,
    /// Per-service topology flags, extracted from the NixOS service modules.
    pub registry: ServiceRegistry,
}

impl NixNetwork {
    pub fn add_zone(&mut self, zone: NixZone) {
        self.zones.insert(zone.name.clone(), zone);
    }

    pub fn get_zone(&self, name: &str) -> Result<&NixZone> {
        self.zones
            .get(name)
            .ok_or_else(|| NixError::validation(format!("Undefined zone \"{name}\"")))
    }

    pub fn get_zone_mut(&mut self, name: &str) -> Result<&mut NixZone> {
        self.zones
            .get_mut(name)
            .ok_or_else(|| NixError::validation(format!("Undefined zone \"{name}\"")))
    }

    pub fn services(&self) -> &IndexMap<String, NixService> {
        &self.services
    }

    /// Services in declaration order.
    pub fn services_as_vec(&self) -> Vec<&NixService> {
        self.services.values().collect()
    }

    /// Zones holding hosts but no `harmonia` service, sorted by name.
    ///
    /// Without one the zone caches nothing the fleet builds: every host is
    /// served path by path over its uplink, and `fleet-update` has no builder
    /// to elect there. The external zone is skipped — it is not a LAN.
    pub fn zones_without_harmonia(&self) -> Vec<String> {
        let mut missing: Vec<String> = self
            .zones
            .values()
            .filter(|zone| !zone.is_external() && !zone.hosts().is_empty())
            .filter(|zone| {
                !self
                    .services
                    .values()
                    .any(|svc| svc.name == "harmonia" && svc.zone == zone.name)
            })
            .map(|zone| zone.name.clone())
            .collect();
        missing.sort();
        missing
    }

    /// Register all services declared on a host.
    /// `hostname`: the host registering the services
    /// `zone`: the host's zone name
    /// `services`: map of service_name -> service params
    pub fn register_services(
        &mut self,
        hostname: &str,
        zone: &str,
        services: &IndexMap<String, ServiceParams>,
    ) -> Result<()> {
        // Same-host dependencies (modules.nix `require`): a service may not be
        // enabled on a node unless every service it lists is enabled there too.
        for service_name in services.keys() {
            for req in self.registry.requires(service_name) {
                if !services.contains_key(req.as_str()) {
                    return Err(NixError::validation(format!(
                        "Service '{service_name}' on '{hostname}' requires '{req}' enabled on the same host"
                    )));
                }
            }
        }

        for (service_name, params) in services {
            let mut is_global = params.global;
            let service_domain = params.domain.as_deref().unwrap_or(service_name);

            // Services that must be unique per zone
            if self.registry.flags(service_name).unique_per_zone {
                let key = (zone.to_string(), service_name.clone());
                if self.uniq_services.contains_key(&key) {
                    return Err(NixError::validation(format!(
                        "Service {service_name} must be unique in zone {zone}"
                    )));
                }
                self.uniq_services.insert(key, true);
            }

            // External zone services are implicitly global
            if zone == EXTERNAL_ZONE_KEY {
                is_global = true;
            }

            // Global domain conflict check
            if is_global {
                if self.global_service_domains.contains(service_domain) {
                    return Err(NixError::validation(format!(
                        "Global services domain name conflict: {service_name}"
                    )));
                }
                self.global_service_domains
                    .insert(service_domain.to_string());
            }

            // Zone-level domain conflict check
            let key = format!("{zone}:{service_domain}");
            if self.services.contains_key(&key) {
                return Err(NixError::validation(format!(
                    "Service name conflict: {key}"
                )));
            }

            let mut svc = NixService::new(service_name, hostname, zone);
            svc.domain = params.domain.clone();
            svc.title = params.title.clone();
            svc.description = params.description.clone();
            svc.icon = params.icon.clone();
            svc.global = is_global;
            self.services.insert(key, svc);
        }
        Ok(())
    }

    /// Validate and store network configuration.
    pub fn register_network_config(&mut self, cfg: Option<&NetworkCfg>) -> Result<()> {
        let default = cfg.and_then(|c| c.default.as_ref());
        let coord = cfg.and_then(|c| c.coordination.as_ref());
        let smtp = cfg.and_then(|c| c.smtp.as_ref());
        let fleet_update = cfg.and_then(|c| c.fleet_update.as_ref());

        // Apply defaults
        let domain = cfg
            .and_then(|c| c.domain.as_deref())
            .unwrap_or(DEFAULT_DOMAIN)
            .to_string();
        let locale = default
            .and_then(|d| d.locale.as_deref())
            .unwrap_or(DEFAULT_LOCALE)
            .to_string();
        let timezone = default
            .and_then(|d| d.timezone.as_deref())
            .unwrap_or(DEFAULT_TIMEZONE)
            .to_string();
        let coord_domain = coord
            .and_then(|c| c.domain.as_deref())
            .unwrap_or(DEFAULT_COORDINATION_DOMAIN)
            .to_string();
        let coord_hostname = coord
            .and_then(|c| c.hostname.as_deref())
            .unwrap_or("")
            .to_string();
        let coord_enable = coord.and_then(|c| c.enable).unwrap_or(false);

        // Validate
        assert_regex(RE_LOCALE, &locale, "Bad default network locale syntax")?;
        assert_regex(
            RE_TIMEZONE,
            &timezone,
            "Bad default network timezone syntax",
        )?;
        if !coord_hostname.is_empty() {
            assert_regex(
                RE_HOSTNAME,
                &coord_hostname,
                "Bad coordination hostname type",
            )?;
        }
        assert_regex(RE_HOSTNAME, &coord_domain, "Bad Headscale domain name")?;

        // SMTP validation
        if let Some(smtp) = smtp {
            if let Some(proto) = smtp.protocol.as_deref() {
                assert_regex(RE_SMTP_PROTOCOL, proto, "Bad SMTP protocol")?;
            }
            if let Some(server) = smtp.server.as_deref() {
                assert_regex(RE_FQDN, server, "Bad SMTP Server")?;
            }
            if let Some(user) = smtp.username.as_deref() {
                assert_email(user, "Bad SMTP Email")?;
            }
        }

        // fleet-update defaults: syntax only, the tool applies them
        if let Some(fu) = fleet_update {
            if let Some(order) = fu.deployment_order.as_deref() {
                assert_profile_list(order, true, "Bad fleetUpdate deploymentOrder")?;
            }
            if let Some(critical) = fu.critical_profiles.as_deref() {
                assert_profile_list(critical, false, "Bad fleetUpdate criticalProfiles")?;
            }
            if let Some(timeouts) = &fu.timeouts {
                for (key, seconds) in timeouts {
                    assert_timeout_key(key, "Bad fleetUpdate timeouts")?;
                    assert_seconds(*seconds, &format!("Bad fleetUpdate timeouts {key}"))?;
                }
            }
            if let Some(interval) = fu.ping_interval {
                assert_seconds(interval, "Bad fleetUpdate pingInterval")?;
            }
        }

        self.config = NetworkConfig {
            domain,
            default_locale: locale,
            default_timezone: timezone,
            coordination_domain: coord_domain,
            coordination_hostname: coord_hostname,
            coordination_enable: coord_enable,
            default: default.cloned(),
            coordination: coord.cloned(),
            smtp: smtp.cloned(),
            matrix: cfg.and_then(|c| c.matrix.clone()),
            fleet_update: fleet_update.cloned(),
        };

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nix_generator::item::host::ServiceParams;

    #[test]
    fn register_network_config_defaults() {
        let mut net = NixNetwork::default();
        let cfg: NetworkCfg = serde_yaml::from_str("domain: mynet.lan\ncoordination:\n  domain: headscale\n  hostname: hcs\n  enable: false\ndefault:\n  locale: fr_FR.UTF-8\n  timezone: Europe/Paris").unwrap();
        assert!(net.register_network_config(Some(&cfg)).is_ok());
        assert_eq!(net.config.domain, "mynet.lan");
    }

    #[test]
    fn register_network_config_fleet_update() {
        let mut net = NixNetwork::default();
        let cfg: NetworkCfg = serde_yaml::from_str("fleetUpdate:\n  deploymentOrder: \"hcs:gateway:[others]\"\n  criticalProfiles: \"hcs:gateway\"").unwrap();
        assert!(net.register_network_config(Some(&cfg)).is_ok());
        let fu = net.config.fleet_update.as_ref().unwrap();
        assert_eq!(fu.deployment_order.as_deref(), Some("hcs:gateway:[others]"));

        let bad: NetworkCfg =
            serde_yaml::from_str("fleetUpdate:\n  criticalProfiles: \"hcs:[others]\"").unwrap();
        assert!(NixNetwork::default()
            .register_network_config(Some(&bad))
            .is_err());
        assert!(serde_yaml::from_str::<NetworkCfg>("fleetUpdate:\n  order: \"hcs\"").is_err());
    }

    #[test]
    fn register_network_config_fleet_update_delays() {
        let mut net = NixNetwork::default();
        let cfg: NetworkCfg = serde_yaml::from_str(
            "fleetUpdate:\n  pingInterval: 20\n  timeouts:\n    ssh: 45\n    build: 7200",
        )
        .unwrap();
        assert!(net.register_network_config(Some(&cfg)).is_ok());
        let fu = net.config.fleet_update.as_ref().unwrap();
        assert_eq!(fu.ping_interval, Some(20));
        assert_eq!(fu.timeouts.as_ref().unwrap()["build"], 7200);

        let mistyped: NetworkCfg =
            serde_yaml::from_str("fleetUpdate:\n  timeouts:\n    biuld: 7200").unwrap();
        assert!(NixNetwork::default()
            .register_network_config(Some(&mistyped))
            .is_err());

        let zero: NetworkCfg =
            serde_yaml::from_str("fleetUpdate:\n  timeouts:\n    ping: 0").unwrap();
        assert!(NixNetwork::default()
            .register_network_config(Some(&zero))
            .is_err());

        let negative: NetworkCfg =
            serde_yaml::from_str("fleetUpdate:\n  pingInterval: -1").unwrap();
        assert!(NixNetwork::default()
            .register_network_config(Some(&negative))
            .is_err());
    }

    #[test]
    fn register_network_config_invalid_locale() {
        let mut net = NixNetwork::default();
        let cfg: NetworkCfg = serde_yaml::from_str("domain: x.lan\ncoordination:\n  domain: hcs\n  hostname: h\n  enable: false\ndefault:\n  locale: bad\n  timezone: Europe/Paris").unwrap();
        assert!(net.register_network_config(Some(&cfg)).is_err());
    }

    fn make_service(name: &str, global: bool) -> (String, ServiceParams) {
        (
            name.to_string(),
            ServiceParams {
                title: None,
                description: None,
                domain: None,
                icon: None,
                global,
            },
        )
    }

    #[test]
    fn register_service_conflict_in_zone() {
        let mut net = NixNetwork::default();
        let services1: IndexMap<_, _> = [make_service("nextcloud", false)].into_iter().collect();
        let services2: IndexMap<_, _> = [make_service("nextcloud", false)].into_iter().collect();
        net.register_services("nas1", "lab", &services1).unwrap();
        assert!(net.register_services("nas2", "lab", &services2).is_err());
    }

    #[test]
    fn zones_without_harmonia_skips_served_and_external_zones() {
        let mut net = NixNetwork::default();
        for name in ["ag", "lg", EXTERNAL_ZONE_KEY] {
            let mut zone = NixZone::new(name);
            zone.register_host("h1", Some("10.0.0.1"), false).unwrap();
            net.add_zone(zone);
        }

        // Declared but empty: nothing to serve, nothing to warn about.
        net.add_zone(NixZone::new("empty"));
        let harmonia: IndexMap<_, _> = [make_service("harmonia", false)].into_iter().collect();
        net.register_services("h1", "ag", &harmonia).unwrap();

        assert_eq!(net.zones_without_harmonia(), vec!["lg".to_string()]);
    }

    #[test]
    fn register_unique_service_per_zone() {
        let mut net = NixNetwork::default();
        net.registry =
            ServiceRegistry::from_nix(r#"{ adguardhome = { uniquePerZone = true; }; }"#).unwrap();
        let s1: IndexMap<_, _> = [make_service("adguardhome", false)].into_iter().collect();
        let s2: IndexMap<_, _> = [make_service("adguardhome", false)].into_iter().collect();
        net.register_services("dns1", "lab", &s1).unwrap();
        assert!(net.register_services("dns2", "lab", &s2).is_err());
    }

    #[test]
    fn register_global_domain_conflict() {
        let mut net = NixNetwork::default();
        let s1: IndexMap<_, _> = [make_service("auth", true)].into_iter().collect();
        let s2: IndexMap<_, _> = [make_service("auth", true)].into_iter().collect();
        net.register_services("srv1", "lab", &s1).unwrap();
        assert!(net.register_services("srv2", "prod", &s2).is_err());
    }

    #[test]
    fn register_require_missing_dependency_fails() {
        let mut net = NixNetwork::default();
        net.registry =
            ServiceRegistry::from_nix(r#"{ monitoring = { require = [ "prometheus" ]; }; }"#)
                .unwrap();
        let s: IndexMap<_, _> = [make_service("monitoring", false)].into_iter().collect();
        assert!(net.register_services("box", "lab", &s).is_err());
    }

    #[test]
    fn register_require_satisfied_same_host_ok() {
        let mut net = NixNetwork::default();
        net.registry =
            ServiceRegistry::from_nix(r#"{ monitoring = { require = [ "prometheus" ]; }; }"#)
                .unwrap();
        let s: IndexMap<_, _> = [
            make_service("monitoring", false),
            make_service("prometheus", false),
        ]
        .into_iter()
        .collect();
        assert!(net.register_services("box", "lab", &s).is_ok());
    }

    #[test]
    fn external_service_implicit_global() {
        let mut net = NixNetwork::default();
        let services: IndexMap<_, _> = [make_service("headscale", false)].into_iter().collect();
        assert!(net
            .register_services("vpn", EXTERNAL_ZONE_KEY, &services)
            .is_ok());
        let svc = net.services().values().next().unwrap();
        assert!(svc.global);
    }
}
