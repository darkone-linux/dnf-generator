use std::collections::HashMap;
use std::path::Path;

use rnix::ast::{Attr, Expr, HasEntry, InterpolPart};

use crate::error::{NixError, Result};

/// Plain string literal behind an `Expr` (no interpolation expected in
/// modules.nix). Returns `None` for interpolated or non-string expressions.
fn expr_str_literal(expr: &Expr) -> Option<String> {
    let Expr::Str(s) = expr else {
        return None;
    };
    let mut out = String::new();
    for part in s.normalized_parts() {
        match part {
            InterpolPart::Literal(text) => out.push_str(&text),
            InterpolPart::Interpolation(_) => return None,
        }
    }
    Some(out)
}

/// Topology flags for a single service, read from `dnf/config/modules.nix`.
#[derive(Debug, Clone, Copy)]
pub struct ServiceFlags {
    /// Reached through the zone gateway reverse proxy (DNS points to gateway LAN IP).
    pub reverse_proxy: bool,

    /// At most one instance allowed per zone (generator validation).
    pub unique_per_zone: bool,

    /// www-zone service reachable from the LAN via a fixed host IP (e.g. headscale, turn).
    pub external_access: bool,
}

/// Defaults match the modules.nix header: reverseProxy=true, others false.
impl Default for ServiceFlags {
    fn default() -> Self {
        Self {
            reverse_proxy: true,
            unique_per_zone: false,
            external_access: false,
        }
    }
}

/// Service-name → flags map, loaded from `dnf/config/modules.nix`.
#[derive(Debug, Default, Clone)]
pub struct ServiceRegistry {
    map: HashMap<String, ServiceFlags>,

    /// Same-host dependencies declared via `require = [ ... ]`: a service may
    /// not be enabled on a node unless every listed service is too.
    requires: HashMap<String, Vec<String>>,
}

impl ServiceRegistry {
    /// Parse a registry from `modules.nix` Nix source.
    /// Keys are camelCase (`reverseProxy`, `uniquePerZone`, `externalAccess`).
    /// Flags absent from a service entry use the defaults (see `ServiceFlags::default`).
    pub fn from_nix(content: &str) -> Result<Self> {
        let parsed = rnix::Root::parse(content);
        let Some(Expr::AttrSet(root_set)) = parsed.tree().expr() else {
            return Err(NixError::generate("modules.nix: expected attrset at root"));
        };

        let mut map = HashMap::new();
        let mut requires: HashMap<String, Vec<String>> = HashMap::new();

        for av in root_set.attrpath_values() {
            let (Some(attrpath), Some(Expr::AttrSet(svc_set))) = (av.attrpath(), av.value())
            else {
                continue;
            };

            let name = attrpath
                .attrs()
                .next()
                .and_then(|a| match a {
                    Attr::Ident(i) => i.ident_token().map(|t| t.text().to_string()),
                    _ => None,
                })
                .unwrap_or_default();

            if name.is_empty() {
                continue;
            }

            let mut flags = ServiceFlags::default();
            let mut reqs: Vec<String> = Vec::new();

            for svc_av in svc_set.attrpath_values() {
                let (Some(key_path), Some(val)) = (svc_av.attrpath(), svc_av.value()) else {
                    continue;
                };
                let key = key_path
                    .attrs()
                    .next()
                    .and_then(|a| match a {
                        Attr::Ident(i) => i.ident_token().map(|t| t.text().to_string()),
                        _ => None,
                    })
                    .unwrap_or_default();

                // `require = [ "svc" ... ]`: same-host service dependencies.
                if key == "require" {
                    if let Expr::List(list) = &val {
                        reqs.extend(list.items().filter_map(|item| expr_str_literal(&item)));
                    }
                    continue;
                }

                let bool_val = if let Expr::Ident(i) = &val {
                    i.ident_token().map(|t| t.text() == "true").unwrap_or(false)
                } else {
                    false
                };

                match key.as_str() {
                    "reverseProxy" => flags.reverse_proxy = bool_val,
                    "uniquePerZone" => flags.unique_per_zone = bool_val,
                    "externalAccess" => flags.external_access = bool_val,
                    _ => {}
                }
            }

            if !reqs.is_empty() {
                requires.insert(name.clone(), reqs);
            }
            map.insert(name, flags);
        }

        Ok(Self { map, requires })
    }

    /// Load from `modules.nix`. Missing file → empty registry (all defaults).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        Self::from_nix(&content)
    }

    /// Flags for `name`, or defaults if the service is unknown.
    pub fn flags(&self, name: &str) -> ServiceFlags {
        self.map.get(name).copied().unwrap_or_default()
    }

    /// Services that `name` requires on the same host (empty if none).
    pub fn requires(&self, name: &str) -> &[String] {
        self.requires.get(name).map_or(&[], Vec::as_slice)
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
    fn registry_parses_nix_flags() {
        let nix = r#"{
          ncps = { reverseProxy = false; uniquePerZone = true; };
          headscale = { reverseProxy = false; externalAccess = true; };
          nextcloud = {};
          turn = { externalAccess = true; };
        }"#;
        let reg = ServiceRegistry::from_nix(nix).unwrap();
        assert!(!reg.flags("ncps").reverse_proxy);
        assert!(reg.flags("ncps").unique_per_zone);
        assert!(!reg.flags("headscale").reverse_proxy);
        assert!(reg.flags("headscale").external_access);
        assert!(reg.flags("nextcloud").reverse_proxy); // default = true
        assert!(!reg.flags("nextcloud").unique_per_zone);
        assert!(reg.flags("turn").external_access);
        assert!(reg.flags("turn").reverse_proxy); // default = true
    }

    #[test]
    fn registry_parses_require_list() {
        let nix = r#"{
          monitoring = { require = [ "prometheus" ]; };
          prometheus = {};
          grafana = { reverseProxy = false; require = [ "prometheus" "postfix" ]; };
        }"#;
        let reg = ServiceRegistry::from_nix(nix).unwrap();

        let mon: Vec<&str> = reg.requires("monitoring").iter().map(String::as_str).collect();
        assert_eq!(mon, ["prometheus"]);

        let graf: Vec<&str> = reg.requires("grafana").iter().map(String::as_str).collect();
        assert_eq!(graf, ["prometheus", "postfix"]);

        assert!(reg.requires("prometheus").is_empty());
        assert!(reg.requires("unknown").is_empty());
    }

    #[test]
    fn registry_ignores_non_topology_keys() {
        let nix = r#"{
          ncps = {
            reverseProxy = false;
            description = "something";
            activation.profiles.minimal.triggers.keys.ncps = [ "enable" ];
          };
        }"#;
        let reg = ServiceRegistry::from_nix(nix).unwrap();
        assert!(!reg.flags("ncps").reverse_proxy);
        assert!(!reg.flags("ncps").unique_per_zone);
    }

    #[test]
    fn registry_unknown_service_defaults_reverse_proxy() {
        let reg = ServiceRegistry::default();
        let f = reg.flags("whatever");
        assert!(f.reverse_proxy); // default = true
        assert!(!f.unique_per_zone);
        assert!(!f.external_access);
    }

    #[test]
    fn registry_missing_file_is_empty() {
        let reg = ServiceRegistry::load(Path::new("/nonexistent/modules.nix")).unwrap();
        assert!(reg.flags("anything").reverse_proxy); // default = true
    }
}
