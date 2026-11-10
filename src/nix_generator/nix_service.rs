use std::collections::HashMap;
use std::path::Path;

use rnix::ast::{Attr, Attrpath, Expr, HasEntry, InterpolPart, Str};
use rowan::ast::AstNode as _;

use crate::error::{NixError, Result};

/// Keys of a service entry the generator reads. Every other key (`activation`,
/// `description`) belongs to the Nix side and is skipped.
const TOPOLOGY_KEYS: [&str; 4] = ["reverseProxy", "uniquePerZone", "externalAccess", "require"];

/// The registry is read as source text, never evaluated: a value the parser
/// cannot read as a literal is refused, not replaced by a default.
fn registry_error(msg: impl std::fmt::Display) -> NixError {
    NixError::generate(format!("modules.nix: {msg}"))
}

/// Plain string literal, `None` when interpolated.
fn str_literal(s: &Str) -> Option<String> {
    let mut out = String::new();
    for part in s.normalized_parts() {
        match part {
            InterpolPart::Literal(text) => out.push_str(&text),
            InterpolPart::Interpolation(_) => return None,
        }
    }
    Some(out)
}

/// Literal `true` / `false`, `None` for any other expression.
fn bool_literal(expr: &Expr) -> Option<bool> {
    let Expr::Ident(ident) = expr else {
        return None;
    };
    match ident.ident_token()?.text() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// First segment of an attribute path and whether it is the only one
/// (`a` → `("a", true)`, `a.b` → `("a", false)`). `None` when interpolated.
fn first_key(path: &Attrpath) -> Option<(String, bool)> {
    let mut attrs = path.attrs();
    let name = match attrs.next()? {
        Attr::Ident(ident) => ident.ident_token()?.text().to_string(),
        Attr::Str(s) => str_literal(&s)?,
        Attr::Dynamic(_) => return None,
    };
    Some((name, attrs.next().is_none()))
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
    /// Parse a registry from `modules.nix` Nix source: a literal attribute set,
    /// one `<service> = { ... };` per entry.
    ///
    /// Flags (`reverseProxy`, `uniquePerZone`, `externalAccess`) are literal
    /// booleans, `require` a list of plain strings. A flag absent from an entry
    /// takes its default (see `ServiceFlags::default`); anything else that
    /// cannot be read as a literal is an error.
    pub fn from_nix(content: &str) -> Result<Self> {
        let parsed = rnix::Root::parse(content);
        if let Some(error) = parsed.errors().first() {
            return Err(registry_error(format!("syntax error: {error}")));
        }
        let Some(Expr::AttrSet(root_set)) = parsed.tree().expr() else {
            return Err(registry_error(
                "expected a literal attribute set at root (no `let`, function or `//`)",
            ));
        };
        if root_set.inherits().next().is_some() {
            return Err(registry_error(
                "`inherit` at root: write each service entry out",
            ));
        }

        let mut map = HashMap::new();
        let mut requires: HashMap<String, Vec<String>> = HashMap::new();

        for entry in root_set.attrpath_values() {
            let Some(path) = entry.attrpath() else {
                continue;
            };
            let label = path.syntax().text().to_string();

            // `a.b = …` at root would hide a flag from the reader below.
            let Some((name, true)) = first_key(&path) else {
                return Err(registry_error(format!(
                    "`{label}`: expected a plain service name"
                )));
            };
            let Some(Expr::AttrSet(svc_set)) = entry.value() else {
                return Err(registry_error(format!(
                    "`{name}`: expected a literal attribute set"
                )));
            };
            if svc_set.inherits().next().is_some() {
                return Err(registry_error(format!(
                    "`{name}`: `inherit` hides its keys, write them out"
                )));
            }
            if map.contains_key(&name) {
                return Err(registry_error(format!("`{name}`: declared twice")));
            }

            let mut flags = ServiceFlags::default();
            let mut reqs: Vec<String> = Vec::new();

            for svc_entry in svc_set.attrpath_values() {
                let (Some(key_path), Some(value)) = (svc_entry.attrpath(), svc_entry.value())
                else {
                    continue;
                };
                let Some((key, single)) = first_key(&key_path) else {
                    return Err(registry_error(format!(
                        "`{name}`: interpolated key `{}`",
                        key_path.syntax().text()
                    )));
                };
                if !TOPOLOGY_KEYS.contains(&key.as_str()) {
                    continue;
                }
                if !single {
                    return Err(registry_error(format!(
                        "`{name}.{key}`: unexpected sub-attribute `{}`",
                        key_path.syntax().text()
                    )));
                }

                // `require = [ "svc" ... ]`: same-host service dependencies.
                if key == "require" {
                    let Expr::List(list) = &value else {
                        return Err(registry_error(format!(
                            "`{name}.require`: expected a literal list of service names"
                        )));
                    };
                    for item in list.items() {
                        let Expr::Str(s) = &item else {
                            return Err(registry_error(format!(
                                "`{name}.require`: `{}` is not a plain string",
                                item.syntax().text()
                            )));
                        };
                        let Some(required) = str_literal(s) else {
                            return Err(registry_error(format!(
                                "`{name}.require`: `{}` is interpolated",
                                item.syntax().text()
                            )));
                        };
                        reqs.push(required);
                    }
                    continue;
                }

                let Some(flag) = bool_literal(&value) else {
                    return Err(registry_error(format!(
                        "`{name}.{key}`: expected a literal `true` or `false`, got `{}`",
                        value.syntax().text()
                    )));
                };
                match key.as_str() {
                    "reverseProxy" => flags.reverse_proxy = flag,
                    "uniquePerZone" => flags.unique_per_zone = flag,
                    "externalAccess" => flags.external_access = flag,
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

    /// Load from `modules.nix`. A missing file is an error: default flags
    /// would silently generate another DNS and reverse proxy layout.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(NixError::generate(
                "file not found (is `dnf/` linked? run `nix run .#init`)",
            ));
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

        let mon: Vec<&str> = reg
            .requires("monitoring")
            .iter()
            .map(String::as_str)
            .collect();
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
    fn registry_missing_file_is_an_error() {
        let err = ServiceRegistry::load(Path::new("/nonexistent/modules.nix")).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    /// Message of the error `from_nix` must return for `nix`.
    fn rejected(nix: &str) -> String {
        ServiceRegistry::from_nix(nix)
            .expect_err("the registry must be refused")
            .to_string()
    }

    #[test]
    fn registry_rejects_non_literal_flags() {
        for value in ["!true", "lib.mkDefault false", "(false)", "\"true\"", "1"] {
            let msg = rejected(&format!("{{ ncps = {{ reverseProxy = {value}; }}; }}"));
            assert!(msg.contains("`ncps.reverseProxy`"), "{value}: {msg}");
        }
    }

    #[test]
    fn registry_rejects_non_literal_require() {
        let msg = rejected(r#"{ a = { require = lib.optional true "b"; }; }"#);
        assert!(msg.contains("`a.require`"), "{msg}");
        let msg = rejected(r#"{ a = { require = [ "b" c ]; }; }"#);
        assert!(msg.contains("not a plain string"), "{msg}");
        let msg = rejected(r#"{ a = { require = [ "b-${c}" ]; }; }"#);
        assert!(msg.contains("interpolated"), "{msg}");
    }

    #[test]
    fn registry_rejects_non_literal_structure() {
        // Each of these evaluates fine in Nix and used to yield default flags.
        for nix in [
            "let off = false; in { ncps = { reverseProxy = off; }; }",
            "{ ncps = { reverseProxy = false; }; } // { }",
            "{ ncps.reverseProxy = false; }",
            "{ ncps = flags; }",
            "{ inherit ncps; }",
            "{ ncps = { inherit reverseProxy; }; }",
            "{ ncps = { reverseProxy.value = false; }; }",
            "{ ncps = { ${key} = false; }; }",
        ] {
            let msg = rejected(nix);
            assert!(
                msg.starts_with("Generator error: modules.nix: "),
                "{nix}: {msg}"
            );
        }
    }

    #[test]
    fn registry_rejects_syntax_errors_and_duplicates() {
        let msg = rejected("{ ncps = { reverseProxy = ; }; }");
        assert!(msg.contains("syntax error"), "{msg}");
        let msg = rejected("{ ncps = { }; ncps = { reverseProxy = false; }; }");
        assert!(msg.contains("declared twice"), "{msg}");
    }

    #[test]
    fn registry_accepts_quoted_names_and_nix_side_paths() {
        let nix = r#"{
          "jitsi-meet" = {
            externalAccess = true;
            activation.profiles = {
              gateway.triggers.keys.idm = [ "enable" ];
            };
          };
        }"#;
        let reg = ServiceRegistry::from_nix(nix).unwrap();
        assert!(reg.flags("jitsi-meet").external_access);
    }
}
