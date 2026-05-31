//! Service model + the per-service topology flags the generator needs.
//!
//! The flags (reverse-proxy / unique-per-zone / external-access) are NOT
//! hard-coded here anymore: they are declared in each service's NixOS module
//! (`darkone.system.services.service.<name>`) and extracted by `just generate`
//! via `nix eval` into `var/generated/service-registry.json`, which is loaded
//! into a [`ServiceRegistry`]. A service absent from the registry falls back to
//! all-false flags (plain host-local service, no proxy, not unique, not external).

use std::collections::HashMap;
use std::path::Path;

use crate::error::Result;

/// Topology flags for a single service, mirroring the Nix options
/// `darkone.system.services.service.<name>.{reverseProxy,uniquePerZone,externalAccess}`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ServiceFlags {
    /// Reached through the zone gateway reverse proxy (DNS points to gateway LAN IP).
    pub reverse_proxy: bool,

    /// At most one instance allowed per zone (generator validation).
    pub unique_per_zone: bool,

    /// www-zone service reachable from the LAN via a fixed host IP (e.g. headscale, turn).
    pub external_access: bool,
}

/// Service-name → flags map, loaded from the generated JSON registry.
#[derive(Debug, Default, Clone)]
pub struct ServiceRegistry {
    map: HashMap<String, ServiceFlags>,
}

impl ServiceRegistry {
    /// Parse a registry from a JSON (or YAML — superset) string.
    ///
    /// We deserialize into a permissive `name -> { flag -> bool }` map and build
    /// [`ServiceFlags`] by hand: serde_yaml 0.9 mishandles `#[serde(default)]`
    /// bool fields in flow mappings, so an explicit lookup is both robust and
    /// forward-compatible (unknown flags are simply ignored).
    pub fn from_str(content: &str) -> Result<Self> {
        let raw: HashMap<String, HashMap<String, bool>> = serde_yaml::from_str(content)?;
        let map = raw
            .into_iter()
            .map(|(name, flags)| {
                let get = |k: &str| flags.get(k).copied().unwrap_or(false);
                (
                    name,
                    ServiceFlags {
                        reverse_proxy: get("reverse_proxy"),
                        unique_per_zone: get("unique_per_zone"),
                        external_access: get("external_access"),
                    },
                )
            })
            .collect();
        Ok(Self { map })
    }

    /// Load the registry from disk. A missing file yields an empty registry so
    /// that generation still works on a fresh checkout (all flags default false).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Self::from_str(&content)
    }

    /// Flags for `name`, or all-false defaults if the service is unknown.
    pub fn flags(&self, name: &str) -> ServiceFlags {
        self.map.get(name).copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct NixService {
    pub name: String,
    pub host: String,
    pub zone: String,
    pub domain: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub global: bool,
}

impl NixService {
    pub fn new(name: impl Into<String>, host: impl Into<String>, zone: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            host: host.into(),
            zone: zone.into(),
            domain: None,
            title: None,
            description: None,
            icon: None,
            global: false,
        }
    }

    /// Return the effective domain label (custom or defaults to name).
    pub fn domain_label(&self) -> &str {
        self.domain.as_deref().unwrap_or(&self.name)
    }

    /// Build the fully-qualified domain name for this service.
    /// `zone_domain`: the zone's domain (e.g. "lab.example.lan")
    /// `network_domain`: the global network domain (e.g. "example.lan")
    pub fn fqdn(&self, zone_domain: &str, network_domain: &str) -> String {
        let label = self.domain_label();
        if self.global {
            format!("{label}.{network_domain}")
        } else {
            format!("{label}.{zone_domain}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fqdn_local_service() {
        let svc = NixService::new("nextcloud", "nas", "lab");
        assert_eq!(
            svc.fqdn("lab.example.lan", "example.lan"),
            "nextcloud.lab.example.lan"
        );
    }

    #[test]
    fn fqdn_global_service() {
        let mut svc = NixService::new("headscale", "vpn", "lab");
        svc.global = true;
        assert_eq!(
            svc.fqdn("lab.example.lan", "example.lan"),
            "headscale.example.lan"
        );
    }

    #[test]
    fn fqdn_custom_domain() {
        let mut svc = NixService::new("auth", "server", "prod");
        svc.domain = Some("sso".to_string());
        assert_eq!(
            svc.fqdn("prod.example.lan", "example.lan"),
            "sso.prod.example.lan"
        );
    }

    #[test]
    fn registry_parses_flags() {
        let reg = ServiceRegistry::from_str(
            r#"{
              "ncps": { "unique_per_zone": true },
              "headscale": { "external_access": true },
              "nextcloud": { "reverse_proxy": true }
            }"#,
        )
        .unwrap();
        assert!(reg.flags("ncps").unique_per_zone);
        assert!(reg.flags("headscale").external_access);
        assert!(reg.flags("nextcloud").reverse_proxy);
    }

    #[test]
    fn registry_unknown_service_is_all_false() {
        let reg = ServiceRegistry::default();
        let f = reg.flags("whatever");
        assert!(!f.reverse_proxy);
        assert!(!f.unique_per_zone);
        assert!(!f.external_access);
    }

    #[test]
    fn registry_missing_file_is_empty() {
        let reg = ServiceRegistry::load(Path::new("/nonexistent/registry.json")).unwrap();
        assert!(!reg.flags("anything").reverse_proxy);
    }
}
