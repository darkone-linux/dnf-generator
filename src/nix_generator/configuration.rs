//! Top-level loader: turns the merged YAML config into a fully-populated
//! [`Configuration`] (users, hosts, zones, services, DNS records).
//!
//! Loading proceeds in order:
//! 1. parse + deep-merge `etc/config.yaml` and `var/generated/config.yaml`,
//!    then validate the result against the strict [`schema`](crate::nix_generator::schema)
//! 2. load network defaults
//! 3. load zones (and their `extraHosts`)
//! 4. load users (regular + special `nix` maintenance user)
//! 5. load hosts in three flavours — static, range, list
//! 6. propagate cross-cutting state (gateways, external hosts replicated in
//!    every local zone)

use std::collections::HashMap;
use std::path::Path;

use indexmap::IndexMap;

use crate::error::{NixError, Result};
use crate::nix_generator::item::host::{Host, ServiceParams};
use crate::nix_generator::item::user::{filter_profile, User, UserBuildConfig};
use crate::nix_generator::nix_network::NixNetwork;
use crate::nix_generator::nix_service::ServiceRegistry;
use crate::nix_generator::nix_zone::{NixZone, EXTERNAL_ZONE_KEY};
use crate::nix_generator::schema::{
    ConfigFile, ExtraHost, HostEntry, Mac, NetworkCfg, ServiceCfg, UserCfg, ZoneCfg,
};
use crate::nix_generator::yaml::deep_merge;

const MAX_RANGE_BOUND: i64 = 1000;
const DEFAULT_PROFILE: &str = "minimal";
const NIX_USER_NAME: &str = "nix";
const NIX_USER_UID: u32 = 65000;
const NIX_USER_DISPLAY: &str = "Nix Maintenance User";
const NIX_USER_PROFILE: &str = "nix-admin";

pub struct Configuration {
    pub users: IndexMap<String, User>,
    pub hosts: IndexMap<String, Host>,
    pub network: NixNetwork,
    /// Static host DNS records: `"hostname,hostname.zonedomain,ip"`
    pub host_records: Vec<String>,
}

impl Configuration {
    pub fn load(
        main_yaml: &Path,
        generated_yaml: &Path,
        registry: ServiceRegistry,
    ) -> Result<Self> {
        let main_str = std::fs::read_to_string(main_yaml)?;
        // The generated YAML acts as an overlay: it may not yet exist on a
        // fresh checkout, in which case we treat it as an empty mapping.
        let gen_str = if generated_yaml.exists() {
            std::fs::read_to_string(generated_yaml)?
        } else {
            "{}".to_string()
        };

        let merged = deep_merge(
            serde_yaml::from_str(&main_str)?,
            serde_yaml::from_str(&gen_str)?,
        );

        // Strict schema gate: reject unknown keys / type mismatches up-front,
        // reporting the exact path of any violation.
        let schema: ConfigFile = serde_path_to_error::deserialize(merged.clone())
            .map_err(|e| NixError::validation(format!("config.yaml: {} (at {})", e.inner(), e.path())))?;

        let project_root = main_yaml
            .parent()
            .and_then(|p| p.parent())
            .ok_or_else(|| NixError::generate("Cannot determine project root"))?;

        let mut cfg = Configuration {
            users: IndexMap::new(),
            hosts: IndexMap::new(),
            network: NixNetwork::default(),
            host_records: vec![],
        };

        // Topology flags consumed by `register_services` and DNS computation.
        cfg.network.registry = registry;

        cfg.load_network(schema.network.as_ref())?;
        cfg.load_zones(schema.zones.as_ref())?;
        cfg.load_users(&schema.users, project_root)?;
        cfg.load_hosts(&schema.hosts, project_root)?;

        Ok(cfg)
    }

    // ─── network ────────────────────────────────────────────────────────────

    fn load_network(&mut self, network: Option<&NetworkCfg>) -> Result<()> {
        self.network.register_network_config(network)
    }

    // ─── zones ──────────────────────────────────────────────────────────────

    fn load_zones(&mut self, zones: Option<&IndexMap<String, ZoneCfg>>) -> Result<()> {
        // Always declare the external "www" zone so service generation can
        // reference it even when the YAML doesn't list it.
        let mut www = NixZone::new(EXTERNAL_ZONE_KEY);
        www.register_zone_config(
            None,
            &self.network.config.default_locale,
            &self.network.config.default_timezone,
            &self.network.config.domain,
        )?;
        self.network.add_zone(www);

        let Some(zones) = zones else {
            return Ok(());
        };

        // `zones.common` carries cross-zone defaults but is not itself a zone;
        // it is skipped here.
        for (zone_name, zone_cfg) in zones {
            if zone_name == "common" {
                continue;
            }

            let mut zone = NixZone::new(zone_name.as_str());
            zone.register_zone_config(
                Some(zone_cfg),
                &self.network.config.default_locale,
                &self.network.config.default_timezone,
                &self.network.config.domain,
            )?;
            let ip_prefix = zone.ip_prefix().to_string();
            self.network.add_zone(zone);

            if let Some(extra_hosts) = &zone_cfg.extra_hosts {
                self.process_extra_hosts(zone_name, &ip_prefix, extra_hosts)?;
            }
        }

        Ok(())
    }

    /// Declare each `extraHosts` entry as a synthetic host on its zone (DHCP,
    /// aliases, services, DNS).
    fn process_extra_hosts(
        &mut self,
        zone_name: &str,
        ip_prefix: &str,
        extra_hosts: &IndexMap<String, ExtraHost>,
    ) -> Result<()> {
        for (hostname, host_cfg) in extra_hosts {
            let host_ip = format!("{ip_prefix}.{}", host_cfg.ip);
            let aliases = host_cfg.aliases.clone().unwrap_or_default();
            let services = parse_services(host_cfg.services.as_ref(), hostname)?;

            {
                let zone = self.network.get_zone_mut(zone_name)?;
                zone.register_host(hostname, Some(&host_ip), false)?;
                if let Some(mac) = host_cfg.mac.as_deref() {
                    zone.register_mac_addresses(mac, &host_ip)?;
                }
                if !aliases.is_empty() {
                    zone.register_aliases(hostname, &aliases)?;
                }
            }

            if !services.is_empty() {
                self.network
                    .register_services(hostname, zone_name, &services)?;
            }
        }
        Ok(())
    }

    // ─── users ──────────────────────────────────────────────────────────────

    fn load_users(
        &mut self,
        users: &IndexMap<String, UserCfg>,
        project_root: &Path,
    ) -> Result<()> {
        // Pre-reserve the special nix UID so a regular user can't steal it.
        let mut uid_tracker: HashMap<u32, String> = HashMap::new();
        uid_tracker.insert(NIX_USER_UID, NIX_USER_NAME.to_string());

        for (login, user_cfg) in users {
            let user = User::build(UserBuildConfig {
                login,
                uid: user_cfg.uid,
                name: &user_cfg.name,
                email: user_cfg.email.as_deref(),
                profile: user_cfg.profile.as_deref().unwrap_or(DEFAULT_PROFILE),
                groups: user_cfg.groups.clone(),
                disabled: user_cfg.disabled,
                uid_tracker: &mut uid_tracker,
                project_root,
            })?;
            self.users.insert(login.clone(), user);
        }

        // Append the special nix maintenance user LAST. The profile lookup may
        // legitimately fail in test fixtures; in that case fall back to a
        // hard-coded path.
        let nix_user = User {
            login: NIX_USER_NAME.to_string(),
            uid: NIX_USER_UID,
            name: NIX_USER_DISPLAY.to_string(),
            email: None,
            profile: filter_profile(NIX_USER_PROFILE, project_root)
                .unwrap_or_else(|_| format!("dnf/home/profiles/{NIX_USER_PROFILE}")),
            groups: vec![],
            disabled: false,
        };
        self.users.insert(NIX_USER_NAME.to_string(), nix_user);

        Ok(())
    }

    // ─── hosts ──────────────────────────────────────────────────────────────

    /// Hosts come in three flavours, dispatched by which key is present:
    /// - `range:` → a group templated over an integer range (e.g. `fd-01`..`fd-02`)
    /// - `hosts:` → a group templated over a list of named sub-hosts
    /// - otherwise → a single static host
    fn load_hosts(&mut self, hosts: &[HostEntry], project_root: &Path) -> Result<()> {
        // Expand range/list groups into concrete hosts, preserving the original
        // ordering: plain static hosts first, then range-generated, then
        // list-generated.
        let mut static_hosts: Vec<HostEntry> = vec![];
        let mut range_hosts: Vec<HostEntry> = vec![];
        let mut list_hosts: Vec<HostEntry> = vec![];
        for host in hosts {
            if let Some(range) = &host.range {
                let (start, end) = parse_range(range)?;
                for i in start..=end {
                    range_hosts.push(build_range_entry(host, i)?);
                }
            } else if let Some(sub_hosts) = &host.hosts {
                for (hostname, host_cfg) in sub_hosts {
                    let host_name = host_cfg.name.as_deref().ok_or_else(|| {
                        NixError::validation(format!("Bad host description for {hostname}"))
                    })?;
                    list_hosts.push(build_list_entry(host, host_cfg, hostname, host_name));
                }
            } else {
                static_hosts.push(host.clone());
            }
        }

        for host in static_hosts.iter().chain(&range_hosts).chain(&list_hosts) {
            self.load_static_host(host, project_root)?;
        }
        self.populate_zones()?;

        Ok(())
    }

    fn load_static_host(&mut self, host_val: &HostEntry, project_root: &Path) -> Result<()> {
        let hostname = host_val
            .hostname
            .as_deref()
            .ok_or_else(|| NixError::validation("A hostname is required"))?;
        let name = host_val.name.as_deref().ok_or_else(|| {
            NixError::validation(format!("A name is required for \"{hostname}\""))
        })?;
        let profile = host_val.profile.as_deref().ok_or_else(|| {
            NixError::validation(format!("A host profile is required for \"{hostname}\""))
        })?;

        let (zone_name, ip) = self.extract_zone_and_ip(host_val, hostname)?;
        let zone_domain = if zone_name == EXTERNAL_ZONE_KEY {
            self.network.config.domain.clone()
        } else {
            self.network.get_zone(&zone_name)?.domain().to_string()
        };

        let groups = host_val.groups.clone().unwrap_or_default();
        let users = self.expand_users(&host_val.users.clone().unwrap_or_default(), &groups);
        let services = parse_services(host_val.services.as_ref(), hostname)?;
        let aliases = host_val.aliases.clone().unwrap_or_default();

        // Build the Host struct itself.
        let mut host = Host::new(hostname);
        host.name = name.to_string();
        host.zone = zone_name.clone();
        host.profile = profile.to_string();
        host.arch = host_val.arch.clone();
        host.zone_domain = zone_domain.clone();
        host.network_domain = self.network.config.domain.clone();
        host.groups = groups;
        host.set_users(users)?;
        host.set_features(&host_val.features.clone().unwrap_or_default());
        host.tags = host_val.tags.clone().unwrap_or_default();
        host.ip = ip.clone();
        host.vpn_ip = host_val.ipv4.as_ref().and_then(|v| v.internal.clone());
        host.services = services;
        host.set_disko(disko_profile(host_val), disko_devices(host_val), project_root)?;

        // Mirror the host into the zone (DHCP, aliases) and into the service
        // registry, then publish a DNS record.
        let mac = mac_single(&host_val.mac);
        let zone = self.network.get_zone_mut(&zone_name)?;
        zone.register_host(hostname, ip.as_deref(), false)?;
        if let (Some(ip_str), Some(mac)) = (&ip, mac.as_deref()) {
            zone.register_mac_addresses(mac, ip_str)?;
        }
        if !aliases.is_empty() {
            zone.register_aliases(hostname, &aliases)?;
        }

        self.network
            .register_services(hostname, &zone_name, &host.services)?;

        if let Some(ref ip_str) = ip {
            self.register_host_record(hostname, &zone_domain, ip_str);
        }

        self.hosts.insert(hostname.to_string(), host);
        Ok(())
    }

    /// Two cross-cutting steps after every host is loaded:
    /// - any local host whose IP ends in `.1.1` is the zone's gateway;
    /// - hosts declared in the external (`www`) zone are replicated as
    ///   read-only entries in every local zone, so DNS / dnsmasq can address
    ///   them by name from inside the LAN.
    fn populate_zones(&mut self) -> Result<()> {
        let snapshots: Vec<(String, String, Option<String>, Option<String>)> = self
            .hosts
            .values()
            .map(|h| {
                (
                    h.hostname.clone(),
                    h.zone.clone(),
                    h.ip.clone(),
                    h.vpn_ip.clone(),
                )
            })
            .collect();

        for (hostname, zone, ip, vpn_ip) in &snapshots {
            // Local-zone gateway detection by IP convention.
            if zone != EXTERNAL_ZONE_KEY && ip.as_deref().is_some_and(|s| s.ends_with(".1.1")) {
                let z = self.network.get_zone_mut(zone)?;
                z.set_gateway_hostname(hostname)?;
                if let Some(ip_str) = ip {
                    z.set_gateway_lan_ip(ip_str);
                }
            }

            // External hosts are visible from every local zone.
            if zone == EXTERNAL_ZONE_KEY {
                let local_zones: Vec<String> = self
                    .network
                    .zones
                    .keys()
                    .filter(|n| n.as_str() != EXTERNAL_ZONE_KEY)
                    .cloned()
                    .collect();
                for local in local_zones {
                    self.network.get_zone_mut(&local)?.register_host(
                        hostname,
                        ip.as_deref(),
                        true,
                    )?;
                }
                if let Some(vpn) = vpn_ip {
                    let z = self.network.get_zone_mut(EXTERNAL_ZONE_KEY)?;
                    z.set_gateway_hostname(hostname)?;
                    z.set_gateway_vpn_ipv4(vpn);
                }
            }
        }

        Ok(())
    }

    // ─── shared host helpers ────────────────────────────────────────────────

    /// Resolve a host's zone and IP from either the `zone:` shorthand
    /// (`"<name>:<ip-suffix>"`) or the explicit `ipv4.external` form.
    fn extract_zone_and_ip(
        &self,
        host: &HostEntry,
        hostname: &str,
    ) -> Result<(String, Option<String>)> {
        if let Some(zone_field) = host.zone.as_deref() {
            let mut parts = zone_field.splitn(2, ':');
            let zone_name = parts.next().unwrap_or("").to_string();
            let ip_suffix = parts.next().unwrap_or("");
            let ip = if ip_suffix.is_empty() {
                None
            } else {
                let prefix = self
                    .network
                    .get_zone(&zone_name)
                    .map(|z| z.ip_prefix().to_string())
                    .unwrap_or_default();
                Some(format!("{prefix}.{ip_suffix}"))
            };
            return Ok((zone_name, ip));
        }
        if let Some(ext_ip) = host.ipv4.as_ref().and_then(|v| v.external.as_deref()) {
            return Ok((EXTERNAL_ZONE_KEY.to_string(), Some(ext_ip.to_string())));
        }
        Err(NixError::validation(format!(
            "A zone name or ipv4 is required for \"{hostname}\""
        )))
    }

    /// Final user list = `nix` + explicit users + members of the host's groups.
    fn expand_users(&self, host_users: &[String], groups: &[String]) -> Vec<String> {
        let mut users: Vec<String> = vec![NIX_USER_NAME.to_string()];
        users.extend_from_slice(host_users);
        for group in groups {
            for user in self.users.values() {
                if user.groups.contains(group) {
                    users.push(user.login.clone());
                }
            }
        }
        users.sort();
        users.dedup();
        users
    }

    fn register_host_record(&mut self, hostname: &str, zone_domain: &str, ip: &str) {
        if !ip.is_empty() {
            self.host_records
                .push(format!("{hostname},{hostname}.{zone_domain},{ip}"));
        }
    }
}

// ─── free helpers ───────────────────────────────────────────────────────────

fn disko_profile(host_val: &HostEntry) -> Option<&str> {
    host_val.disko.as_ref().and_then(|d| d.profile.as_deref())
}

fn disko_devices(host_val: &HostEntry) -> HashMap<String, String> {
    host_val
        .disko
        .as_ref()
        .and_then(|d| d.devices.as_ref())
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// A static host's `mac:` is a single comma-separated string; the indexed form
/// is only meaningful on a `range:` group (resolved in `build_range_entry`).
fn mac_single(mac: &Option<Mac>) -> Option<String> {
    match mac {
        Some(Mac::Single(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Validate a `range: [from, to]` pair and return its bounds.
fn parse_range(range: &[i64]) -> Result<(i64, i64)> {
    if range.len() != 2 {
        return Err(NixError::validation("Bad range type"));
    }
    let (start, end) = (range[0], range[1]);
    let count = end - start;
    if !(0..=MAX_RANGE_BOUND).contains(&count) {
        return Err(NixError::validation(format!(
            "Range [{start}, {end}] out of bound"
        )));
    }
    Ok((start, end))
}

/// Convert a typed `services:` mapping into the internal `ServiceParams` map,
/// rejecting duplicate service names.
fn parse_services(
    services: Option<&IndexMap<String, Option<ServiceCfg>>>,
    hostname: &str,
) -> Result<IndexMap<String, ServiceParams>> {
    let mut result = IndexMap::new();
    let Some(services) = services else {
        return Ok(result);
    };
    for (name, cfg) in services {
        let params = cfg
            .as_ref()
            .map(|c| ServiceParams {
                title: c.title.clone(),
                description: c.description.clone(),
                domain: c.domain.clone(),
                icon: c.icon.clone(),
                global: c.global,
            })
            .unwrap_or_default();
        if result.contains_key(name) {
            return Err(NixError::validation(format!(
                "Service {hostname}:{name} already registered"
            )));
        }
        result.insert(name.clone(), params);
    }
    Ok(result)
}

/// Build the concrete host for index `i` of a `range:` group. The keys in
/// [`GROUP_INHERITED_KEYS`] are propagated from the group; the per-index MAC
/// address (if any) is resolved from the group's indexed `mac:` map.
fn build_range_entry(group: &HostEntry, i: i64) -> Result<HostEntry> {
    let hostname = group
        .hostname
        .as_deref()
        .map(|t| apply_template(t, i))
        .ok_or_else(|| NixError::validation("hostname template required in range"))?;
    let name = group
        .name
        .as_deref()
        .map(|t| apply_template(t, i))
        .unwrap_or_else(|| hostname.clone());
    let zone = group
        .zone
        .as_deref()
        .map(|t| apply_template(t, i))
        .ok_or_else(|| NixError::validation("zone required in range"))?;

    let mac = match &group.mac {
        Some(Mac::Indexed(map)) => map.get(&i).cloned().map(Mac::Single),
        _ => None,
    };

    Ok(HostEntry {
        hostname: Some(hostname),
        name: Some(name),
        zone: Some(zone),
        mac,
        // GROUP_INHERITED_KEYS
        profile: group.profile.clone(),
        users: group.users.clone(),
        groups: group.groups.clone(),
        features: group.features.clone(),
        tags: group.tags.clone(),
        disko: group.disko.clone(),
        // Not inherited.
        ipv4: None,
        arch: None,
        aliases: None,
        services: None,
        range: None,
        hosts: None,
    })
}

/// Build the concrete host for one member of a `hosts:` list group. The group's
/// `hostname`/`name` templates (literal `%s` placeholder) and the inherited keys
/// override the per-host config; everything else (`zone`, `mac`, …) comes from
/// the per-host entry.
fn build_list_entry(
    group: &HostEntry,
    host_cfg: &HostEntry,
    hostname: &str,
    host_name: &str,
) -> HostEntry {
    let tpl_hostname = group
        .hostname
        .as_deref()
        .map(|t| t.replace("%s", hostname))
        .unwrap_or_else(|| hostname.to_string());
    let tpl_name = group
        .name
        .as_deref()
        .map(|t| t.replace("%s", host_name))
        .unwrap_or_else(|| host_name.to_string());

    let mut entry = host_cfg.clone();
    entry.hostname = Some(tpl_hostname);
    entry.name = Some(tpl_name);
    // Group-level keys win over the per-host config.
    entry.profile = group.profile.clone();
    entry.users = group.users.clone();
    entry.groups = group.groups.clone();
    entry.features = group.features.clone();
    entry.tags = group.tags.clone();
    entry.disko = group.disko.clone();
    entry.range = None;
    entry.hosts = None;
    entry
}

/// Apply an sprintf-style template substitution for range host names.
/// Supported: `%'<pad><width>s` (e.g. `%'02s` → zero-padded), `%d`, `%s`.
fn apply_template(template: &str, value: i64) -> String {
    let s = value.to_string();
    let padded = regex::Regex::new(r"%'([^%])(\d+)s")
        .expect("valid regex")
        .replace_all(template, |caps: &regex::Captures| {
            let pad_char = caps[1].chars().next().unwrap_or('0');
            let width: usize = caps[2].parse().unwrap_or(0);
            if s.len() >= width {
                s.clone()
            } else {
                let padding: String = std::iter::repeat_n(pad_char, width - s.len()).collect();
                format!("{padding}{s}")
            }
        })
        .to_string();
    padded.replace("%d", &s).replace("%s", &s)
}
