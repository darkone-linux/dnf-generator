use std::collections::{BTreeMap, HashMap, HashSet};

use serde_yaml::Value;

use crate::error::{NixError, Result};
use crate::nix_generator::schema::{Gateway, ZoneCfg};
use crate::nix_generator::validation::{
    assert_regex, assert_tailscale_ip, ipv4_to_u32, RE_MAC_ADDRESS,
};

const DEFAULT_LAN_IP_PREFIX: &str = "10.1";
const LAN_PREFIX_LENGTH: u8 = 16;
pub const EXTERNAL_ZONE_KEY: &str = "www";

/// Third octet reserved, in every zone, for hosts whose home is another zone
/// (`<ipPrefix>.6.<n>`). The zone addressing convention keeps 1..5 and 11..99
/// for native hosts, so this block is free everywhere.
pub const ROAMING_SUBNET: u8 = 6;

#[derive(Debug)]
pub struct NixZone {
    pub name: String,
    /// MAC -> "mac,ip" entry for dnsmasq dhcp-host
    mac_addresses: HashMap<String, String>,
    /// All MAC addresses registered globally (for duplicate detection)
    all_macs: HashSet<String>,
    /// hostname -> list of aliases
    aliases: HashMap<String, Vec<String>>,
    /// All aliases registered (for duplicate detection)
    all_aliases: HashSet<String>,
    /// hostname -> `Option<ip>`
    hosts: HashMap<String, Option<String>>,
    /// Hosts native to another zone, reserved here: hostname -> (macs, ip)
    roaming: BTreeMap<String, (String, String)>,
    pub dhcp_range: Vec<String>,
    pub config: HashMap<String, serde_yaml::Value>,
}

impl NixZone {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mac_addresses: HashMap::new(),
            all_macs: HashSet::new(),
            aliases: HashMap::new(),
            all_aliases: HashSet::new(),
            hosts: HashMap::new(),
            roaming: BTreeMap::new(),
            dhcp_range: vec![],
            config: HashMap::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_external(&self) -> bool {
        self.name == EXTERNAL_ZONE_KEY
    }

    pub fn domain(&self) -> &str {
        self.config
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    }

    pub fn ip_prefix(&self) -> &str {
        self.config
            .get("ipPrefix")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_LAN_IP_PREFIX)
    }

    pub fn gateway_lan_ip(&self) -> Option<&str> {
        self.config.get("gateway")?.get("lan")?.get("ip")?.as_str()
    }

    pub fn gateway_vpn_ipv4(&self) -> Option<&str> {
        self.config
            .get("gateway")?
            .get("vpn")?
            .get("ipv4")?
            .as_str()
    }

    pub fn hosts(&self) -> &HashMap<String, Option<String>> {
        &self.hosts
    }

    pub fn mac_addresses(&self) -> &HashMap<String, String> {
        &self.mac_addresses
    }

    pub fn aliases(&self) -> &HashMap<String, Vec<String>> {
        &self.aliases
    }

    pub fn roaming(&self) -> &BTreeMap<String, (String, String)> {
        &self.roaming
    }

    /// Reserve `ip` in this zone for `host`, whose home is another zone, so a
    /// machine plugged here gets a predictable lease instead of a dynamic one.
    ///
    /// A host is either native or roaming, never both: a MAC already declared
    /// here means the fleet describes the same machine twice.
    pub fn register_roaming_host(&mut self, host: &str, mac: &str, ip: &str) -> Result<()> {
        for part in mac.split(',') {
            if self.all_macs.contains(part) {
                return Err(NixError::validation(format!(
                    "Mac address {part} of roaming host {host} is already declared in zone {}",
                    self.name
                )));
            }
        }
        self.roaming
            .insert(host.to_string(), (mac.to_string(), ip.to_string()));
        Ok(())
    }

    /// The roaming block must stay free of native hosts: allocation there is
    /// computed, not declared, so any overlap is a config error.
    pub fn assert_roaming_block_free(&self) -> Result<()> {
        let block = format!("{}.{}.", self.ip_prefix(), ROAMING_SUBNET);
        for (host, ip) in &self.hosts {
            if ip.as_deref().is_some_and(|i| i.starts_with(&block)) {
                return Err(NixError::validation(format!(
                    "Host \"{host}\" uses {block}0/24, reserved for roaming hosts in zone {}",
                    self.name
                )));
            }
        }
        Ok(())
    }

    /// A declared fixed address must sit outside the dynamic pool.
    ///
    /// dnsmasq honours a `dhcp-host` reservation only while the address has
    /// not already been leased from `dhcp-range`; an overlap therefore renumbers
    /// the host silently at its first boot — precisely the address the install
    /// tooling was told to come back to.
    pub fn assert_hosts_outside_dhcp_range(&self) -> Result<()> {
        for range in &self.dhcp_range {
            let mut bounds = range.split(',');
            let (Some(start), Some(end)) = (
                bounds.next().and_then(ipv4_to_u32),
                bounds.next().and_then(ipv4_to_u32),
            ) else {
                continue;
            };
            // Sorted: a HashMap would report a different offender on every
            // run when several hosts overlap.
            let mut declared: Vec<(&String, &Option<String>)> = self.hosts.iter().collect();
            declared.sort_by(|a, b| a.0.cmp(b.0));
            for (host, ip) in declared {
                let Some(addr) = ip.as_deref().and_then(ipv4_to_u32) else {
                    continue;
                };
                if (start..=end).contains(&addr) {
                    return Err(NixError::validation(format!(
                        "Host \"{host}\" has fixed ip {} inside the dhcp-range {range} of zone {}: \
                         pick an address outside the dynamic pool",
                        ip.as_deref().unwrap_or_default(),
                        self.name
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn register_host(&mut self, host: &str, ip: Option<&str>, force: bool) -> Result<()> {
        if host.is_empty() {
            return Ok(());
        }
        if !force && self.hosts.contains_key(host) {
            return Err(NixError::validation(format!(
                "Hostname {host} already declared"
            )));
        }
        if let Some(ip_str) = ip {
            if self.hosts.values().any(|v| v.as_deref() == Some(ip_str)) {
                return Err(NixError::validation(format!(
                    "Ip address {ip_str} assigned to more than one host"
                )));
            }
        }
        self.hosts
            .entry(host.to_string())
            .and_modify(|v| {
                if ip.is_some() {
                    *v = ip.map(str::to_string);
                }
            })
            .or_insert_with(|| ip.map(str::to_string));
        Ok(())
    }

    pub fn register_mac_addresses(&mut self, mac: &str, ip: &str) -> Result<()> {
        for part in mac.split(',') {
            assert_regex(
                RE_MAC_ADDRESS,
                part,
                &format!("Bad mac address syntax \"{mac}\""),
            )?;
            if self.all_macs.contains(part) {
                return Err(NixError::validation(format!(
                    "Mac address {part} duplicated"
                )));
            }
            self.all_macs.insert(part.to_string());
        }
        if self.mac_addresses.contains_key(ip) {
            return Err(NixError::validation(format!(
                "Ip address {ip} conflict (mac: {mac} vs {})",
                self.mac_addresses[ip]
            )));
        }
        self.mac_addresses
            .insert(ip.to_string(), format!("{mac},{ip}"));
        Ok(())
    }

    pub fn register_aliases(&mut self, host: &str, aliases: &[String]) -> Result<()> {
        for alias in aliases {
            if self.aliases.contains_key(alias) {
                return Err(NixError::validation(format!(
                    "Alias name {alias} already declared in main hosts"
                )));
            }
            if self.hosts.contains_key(alias) {
                return Err(NixError::validation(format!(
                    "Name {alias} cannot be alias and main host name"
                )));
            }
            if self.all_aliases.contains(alias) {
                return Err(NixError::validation(format!("Duplicated alias {alias}")));
            }
            self.all_aliases.insert(alias.clone());
        }
        self.aliases
            .entry(host.to_string())
            .or_default()
            .extend(aliases.iter().cloned());
        Ok(())
    }

    pub fn set_gateway_lan_ip(&mut self, ip: &str) {
        self.set_gateway_field(&["lan", "ip"], ip.into());
    }

    pub fn set_gateway_vpn_ipv4(&mut self, ip: &str) {
        self.set_gateway_field(&["vpn", "ipv4"], ip.into());
    }

    pub fn set_gateway_hostname(&mut self, hostname: &str) -> Result<()> {
        if let Some(existing) = self
            .config
            .get("gateway")
            .and_then(|g| g.get("hostname"))
            .and_then(serde_yaml::Value::as_str)
        {
            return Err(NixError::validation(format!(
                "Zone \"{}\" already has a gateway \"{existing}\", cannot set \"{hostname}\"",
                self.name
            )));
        }
        self.set_gateway_field(&["hostname"], hostname.into());
        Ok(())
    }

    /// Insert `value` at `gateway.<path…>` inside `self.config`, creating any
    /// intermediate mappings as needed.
    fn set_gateway_field(&mut self, path: &[&str], value: serde_yaml::Value) {
        let gw = self
            .config
            .entry("gateway".to_string())
            .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
        if !matches!(gw, serde_yaml::Value::Mapping(_)) {
            *gw = serde_yaml::Value::Mapping(Default::default());
        }
        if let serde_yaml::Value::Mapping(map) = gw {
            insert_nested(map, path, value);
        }
    }

    /// Validate the typed zone config and materialise the internal `config`
    /// map that drives `network.nix` emission, applying defaults and the
    /// computed keys (`lang`, `domain`, `networkIp`, `prefixLength`).
    ///
    /// `extraHosts` and `gateway.lan.dhcp-range` are intentionally *not* placed
    /// in `config`: the former is expanded into synthetic hosts by the loader,
    /// the latter is consumed here to compute `dhcp_range`.
    pub fn register_zone_config(
        &mut self,
        cfg: Option<&ZoneCfg>,
        default_locale: &str,
        default_timezone: &str,
        network_domain: &str,
    ) -> Result<()> {
        let mut config: HashMap<String, Value> = HashMap::new();

        let locale = cfg
            .and_then(|c| c.locale.as_deref())
            .unwrap_or(default_locale)
            .to_string();
        let timezone = cfg
            .and_then(|c| c.timezone.as_deref())
            .unwrap_or(default_timezone)
            .to_string();
        let description = cfg
            .and_then(|c| c.description.clone())
            .unwrap_or_else(|| format!("{} network zone", self.name));

        config.insert("lang".to_string(), locale[..2].into());
        config.insert("locale".to_string(), locale.into());
        config.insert("timezone".to_string(), timezone.into());
        config.insert("description".to_string(), description.into());

        let domain = if self.is_external() {
            network_domain.to_string()
        } else {
            format!("{}.{network_domain}", self.name)
        };
        config.insert("domain".to_string(), domain.into());

        if !self.is_external() {
            let ip_prefix = cfg
                .and_then(|c| c.ip_prefix.as_deref())
                .unwrap_or(DEFAULT_LAN_IP_PREFIX)
                .to_string();
            config.insert("networkIp".to_string(), format!("{ip_prefix}.0.0").into());
            config.insert(
                "prefixLength".to_string(),
                (LAN_PREFIX_LENGTH as i64).into(),
            );

            let default_range = format!("{ip_prefix}.3.200,{ip_prefix}.3.249,24h");
            if let Some(gw) = cfg.and_then(|c| c.gateway.as_ref()) {
                assert_gw_cfg(gw)?;
                self.dhcp_range = gw
                    .lan
                    .as_ref()
                    .and_then(|l| l.dhcp_range.clone())
                    .unwrap_or_else(|| vec![default_range]);
                config.insert("gateway".to_string(), gateway_to_value(gw));
            } else {
                self.dhcp_range = vec![default_range];
            }

            config.insert("ipPrefix".to_string(), ip_prefix.into());
        }

        self.config = config;
        Ok(())
    }
}

/// Validate a gateway block: a WAN interface and at least one LAN interface are
/// mandatory; a VPN address, if given, must be in the tailnet range.
fn assert_gw_cfg(gw: &Gateway) -> Result<()> {
    if gw
        .wan
        .as_ref()
        .and_then(|w| w.interface.as_deref())
        .is_none()
    {
        return Err(NixError::validation("A WAN interface is required"));
    }
    let lan_ifaces = gw.lan.as_ref().and_then(|l| l.interfaces.as_ref());
    if lan_ifaces.is_none_or(|s| s.is_empty()) {
        return Err(NixError::validation("Valid LAN interfaces are required"));
    }
    if let Some(vpn_ip) = gw.vpn.as_ref().and_then(|v| v.ipv4.as_deref()) {
        assert_tailscale_ip(vpn_ip)?;
    }
    Ok(())
}

/// Build the `gateway` attribute mapping emitted in `network.nix`, in
/// `wan` → `lan` → `vpn` order. `dhcp-range` is deliberately excluded (it is
/// consumed to compute the DHCP range, not emitted). The result stays a
/// `Value::Mapping` so `set_gateway_*` can later inject the gateway hostname
/// and resolved IPs.
fn gateway_to_value(gw: &Gateway) -> Value {
    let mut map = serde_yaml::Mapping::new();
    if let Some(wan) = &gw.wan {
        if let Some(interface) = &wan.interface {
            let mut inner = serde_yaml::Mapping::new();
            inner.insert("interface".into(), interface.clone().into());
            map.insert("wan".into(), Value::Mapping(inner));
        }
    }
    if let Some(lan) = &gw.lan {
        if let Some(interfaces) = &lan.interfaces {
            let mut inner = serde_yaml::Mapping::new();
            let seq: Vec<Value> = interfaces.iter().map(|i| i.clone().into()).collect();
            inner.insert("interfaces".into(), Value::Sequence(seq));
            map.insert("lan".into(), Value::Mapping(inner));
        }
    }
    if let Some(vpn) = &gw.vpn {
        if let Some(ipv4) = &vpn.ipv4 {
            let mut inner = serde_yaml::Mapping::new();
            inner.insert("ipv4".into(), ipv4.clone().into());
            map.insert("vpn".into(), Value::Mapping(inner));
        }
    }
    Value::Mapping(map)
}

/// Walk `path` inside `root`, creating intermediate mappings, and insert `value`
/// at the final key. Used by `set_gateway_*` to nest gateway state.
fn insert_nested(root: &mut serde_yaml::Mapping, path: &[&str], value: serde_yaml::Value) {
    let Some((last, prefix)) = path.split_last() else {
        return;
    };
    let mut cursor = root;
    for segment in prefix {
        let entry = cursor
            .entry(serde_yaml::Value::String((*segment).to_string()))
            .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
        let serde_yaml::Value::Mapping(map) = entry else {
            *entry = serde_yaml::Value::Mapping(Default::default());
            let serde_yaml::Value::Mapping(map) = entry else {
                unreachable!()
            };
            cursor = map;
            continue;
        };
        cursor = map;
    }
    cursor.insert(serde_yaml::Value::String((*last).to_string()), value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_host_basic() {
        let mut z = NixZone::new("lab");
        assert!(z.register_host("server1", Some("10.1.2.1"), false).is_ok());
        assert_eq!(
            z.hosts().get("server1").and_then(Option::as_deref),
            Some("10.1.2.1")
        );
    }

    #[test]
    fn register_host_duplicate_fails() {
        let mut z = NixZone::new("lab");
        z.register_host("server1", Some("10.1.2.1"), false).unwrap();
        assert!(z.register_host("server1", Some("10.1.2.2"), false).is_err());
    }

    #[test]
    fn register_host_ip_conflict_fails() {
        let mut z = NixZone::new("lab");
        z.register_host("srv1", Some("10.1.2.1"), false).unwrap();
        assert!(z.register_host("srv2", Some("10.1.2.1"), false).is_err());
    }

    #[test]
    fn register_mac_valid() {
        let mut z = NixZone::new("lab");
        assert!(z
            .register_mac_addresses("aa:bb:cc:dd:ee:ff", "10.1.2.1")
            .is_ok());
        assert!(z.mac_addresses().contains_key("10.1.2.1"));
    }

    #[test]
    fn register_mac_duplicate_fails() {
        let mut z = NixZone::new("lab");
        z.register_mac_addresses("aa:bb:cc:dd:ee:ff", "10.1.2.1")
            .unwrap();
        assert!(z
            .register_mac_addresses("aa:bb:cc:dd:ee:ff", "10.1.2.2")
            .is_err());
    }

    #[test]
    fn register_aliases_valid() {
        let mut z = NixZone::new("lab");
        z.register_host("server1", Some("10.1.2.1"), false).unwrap();
        assert!(z
            .register_aliases("server1", &["srv1".to_string(), "s1".to_string()])
            .is_ok());
    }

    #[test]
    fn register_alias_duplicate_fails() {
        let mut z = NixZone::new("lab");
        z.register_host("a", Some("10.1.2.1"), false).unwrap();
        z.register_host("b", Some("10.1.2.2"), false).unwrap();
        z.register_aliases("a", &["alias1".to_string()]).unwrap();
        assert!(z.register_aliases("b", &["alias1".to_string()]).is_err());
    }

    #[test]
    fn register_roaming_host_valid() {
        let mut z = NixZone::new("beta");
        z.register_roaming_host("laptop", "aa:bb:cc:dd:ee:ff", "10.8.6.3")
            .unwrap();
        assert_eq!(
            z.roaming().get("laptop"),
            Some(&("aa:bb:cc:dd:ee:ff".to_string(), "10.8.6.3".to_string()))
        );
    }

    #[test]
    fn register_roaming_host_native_mac_fails() {
        let mut z = NixZone::new("beta");
        z.register_mac_addresses("aa:bb:cc:dd:ee:ff", "10.8.2.1")
            .unwrap();
        assert!(z
            .register_roaming_host("laptop", "aa:bb:cc:dd:ee:ff", "10.8.6.3")
            .is_err());
    }

    #[test]
    fn roaming_block_free_by_default() {
        let mut z = NixZone::new("beta");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("server1", Some("10.1.2.1"), false).unwrap();
        assert!(z.assert_roaming_block_free().is_ok());
    }

    #[test]
    fn roaming_block_used_by_native_host_fails() {
        let mut z = NixZone::new("beta");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("intruder", Some("10.1.6.1"), false)
            .unwrap();
        assert!(z.assert_roaming_block_free().is_err());
    }

    #[test]
    fn dhcp_range_default() {
        let mut z = NixZone::new("lab");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        assert_eq!(z.dhcp_range, vec!["10.1.3.200,10.1.3.249,24h"]);
    }

    #[test]
    fn fixed_ip_outside_dhcp_range_ok() {
        let mut z = NixZone::new("lab");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("server1", Some("10.1.2.1"), false).unwrap();
        assert!(z.assert_hosts_outside_dhcp_range().is_ok());
    }

    #[test]
    fn fixed_ip_inside_dhcp_range_fails() {
        let mut z = NixZone::new("lab");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("laptop", Some("10.1.3.238"), false)
            .unwrap();
        let err = z.assert_hosts_outside_dhcp_range().unwrap_err().to_string();
        assert!(err.contains("laptop"), "{err}");
        assert!(err.contains("10.1.3.238"), "{err}");
    }

    /// Boundaries are part of the pool: dnsmasq leases them like any other.
    #[test]
    fn dhcp_range_bounds_are_inclusive() {
        let mut z = NixZone::new("lab");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("edge", Some("10.1.3.200"), false).unwrap();
        assert!(z.assert_hosts_outside_dhcp_range().is_err());
    }

    /// A host with no declared address (DHCP-only) is not a conflict.
    #[test]
    fn host_without_ip_ignored() {
        let mut z = NixZone::new("lab");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("nomad", None, false).unwrap();
        assert!(z.assert_hosts_outside_dhcp_range().is_ok());
    }

    #[test]
    fn several_dhcp_ranges_all_checked() {
        let mut z = NixZone::new("lab");
        let cfg: ZoneCfg = serde_yaml::from_str(
            "ipPrefix: \"10.4\"\n\
             gateway:\n\
             \x20 wan: { interface: eth0 }\n\
             \x20 lan:\n\
             \x20   interfaces: [eth1]\n\
             \x20   dhcp-range:\n\
             \x20     - \"10.4.3.10,10.4.3.20,24h\"\n\
             \x20     - \"10.4.7.10,10.4.7.20,24h\"\n",
        )
        .unwrap();
        z.register_zone_config(Some(&cfg), "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("second", Some("10.4.7.15"), false).unwrap();
        assert!(z.assert_hosts_outside_dhcp_range().is_err());
    }

    /// The external zone has no pool at all: nothing to compare against.
    #[test]
    fn external_zone_has_no_range_to_check() {
        let mut z = NixZone::new(EXTERNAL_ZONE_KEY);
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        z.register_host("hcs", Some("10.1.3.210"), false).unwrap();
        assert!(z.assert_hosts_outside_dhcp_range().is_ok());
    }

    #[test]
    fn external_zone_no_dhcp() {
        let mut z = NixZone::new(EXTERNAL_ZONE_KEY);
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        assert!(z.dhcp_range.is_empty());
        assert_eq!(z.domain(), "darkone.lan");
    }

    #[test]
    fn local_zone_domain() {
        let mut z = NixZone::new("prod");
        z.register_zone_config(None, "fr_FR.UTF-8", "Europe/Paris", "darkone.lan")
            .unwrap();
        assert_eq!(z.domain(), "prod.darkone.lan");
    }
}
