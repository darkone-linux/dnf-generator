//! Typed, **strict** schema for `config.yaml`.
//!
//! Every struct is annotated with `#[serde(deny_unknown_fields)]`: any key that
//! is not declared here is rejected at load time instead of being silently
//! ignored. This is the single source of truth for the shape of the
//! configuration; the loaders in `configuration.rs` consume these types rather
//! than navigating an untyped `serde_yaml::Value`.
//!
//! The merged config (`etc/config.yaml` overlaid with `var/generated/config.yaml`)
//! is deserialized into [`ConfigFile`] in `Configuration::load`, with
//! `serde_path_to_error` so violations report the exact path
//! (e.g. `hosts[2]: unknown field "hsotname"`).

use indexmap::IndexMap;
use serde::Deserialize;

/// Root of `config.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default)]
    pub network: Option<NetworkCfg>,
    #[serde(default)]
    pub zones: Option<IndexMap<String, ZoneCfg>>,
    /// Required: the deployment always declares at least an (empty) users map.
    pub users: IndexMap<String, UserCfg>,
    #[serde(default)]
    pub hosts: Vec<HostEntry>,
}

// ─── network ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkCfg {
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub default: Option<NetworkDefault>,
    #[serde(default)]
    pub coordination: Option<Coordination>,
    #[serde(default)]
    pub smtp: Option<Smtp>,
    #[serde(default)]
    pub matrix: Option<Matrix>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkDefault {
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    /// Injected by the framework into `var/generated/config.yaml`.
    #[serde(default, rename = "password-hash")]
    pub password_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coordination {
    #[serde(default)]
    pub enable: Option<bool>,
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Smtp {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub tls: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    #[serde(default)]
    pub admin: Option<String>,
}

// ─── zones ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneCfg {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default, rename = "ipPrefix")]
    pub ip_prefix: Option<String>,
    #[serde(default)]
    pub gateway: Option<Gateway>,
    #[serde(default, rename = "extraHosts")]
    pub extra_hosts: Option<IndexMap<String, ExtraHost>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gateway {
    #[serde(default)]
    pub wan: Option<GatewayWan>,
    #[serde(default)]
    pub lan: Option<GatewayLan>,
    #[serde(default)]
    pub vpn: Option<GatewayVpn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayWan {
    #[serde(default)]
    pub interface: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayLan {
    #[serde(default)]
    pub interfaces: Option<Vec<String>>,
    #[serde(default, rename = "dhcp-range")]
    pub dhcp_range: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayVpn {
    #[serde(default)]
    pub ipv4: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraHost {
    pub ip: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mac: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub services: Option<IndexMap<String, Option<ServiceCfg>>>,
}

// ─── users ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserCfg {
    pub uid: u32,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    /// When `true`, the account is emitted as `disabled = true;` in `users.nix`;
    /// `false` (the default) is omitted entirely.
    #[serde(default)]
    pub disabled: bool,
}

// ─── hosts ───────────────────────────────────────────────────────────────────

/// A single entry of the top-level `hosts:` list.
///
/// The three flavours (static / `range:` / `hosts:` list) share one closed
/// struct; the dispatch is done in `configuration.rs` by checking which of
/// `range` / `hosts` is present. All fields are optional so any flavour
/// validates, while `deny_unknown_fields` still rejects typos.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub zone: Option<String>,
    #[serde(default)]
    pub ipv4: Option<Ipv4Cfg>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub groups: Option<Vec<String>>,
    #[serde(default)]
    pub users: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub features: Option<Vec<String>>,
    #[serde(default)]
    pub mac: Option<Mac>,
    #[serde(default)]
    pub services: Option<IndexMap<String, Option<ServiceCfg>>>,
    #[serde(default)]
    pub disko: Option<DiskoCfg>,
    /// `range:` flavour — `[from, to]`.
    #[serde(default)]
    pub range: Option<Vec<i64>>,
    /// `hosts:` list flavour — one named sub-host per entry.
    #[serde(default)]
    pub hosts: Option<IndexMap<String, HostEntry>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ipv4Cfg {
    #[serde(default)]
    pub external: Option<String>,
    #[serde(default)]
    pub internal: Option<String>,
}

/// `mac:` is either a single comma-separated string (static hosts) or, for
/// `range:` groups, a map of per-index addresses (`{ 1: "..", 2: ".." }`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Mac {
    Single(String),
    Indexed(IndexMap<i64, String>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceCfg {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub global: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskoCfg {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub devices: Option<IndexMap<String, String>>,
}
