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
//!
//! The same types also derive the JSON Schema of `dnf-generator schema`
//! ([`json_schema`]): `///` comments are the descriptions an editor shows on
//! hover, so they address the user; developer notes use `//`. Patterns reuse
//! the `validation` constants, and only where the generator checks the raw
//! value: a templated field (`range:` hostnames) carries none.

use indexmap::IndexMap;
use schemars::generate::SchemaSettings;
use schemars::transform::RecursiveTransform;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use serde_json::Value;

use super::validation::{
    ALLOWED_ARCH, RE_AI_MODEL, RE_DEVICE, RE_FQDN, RE_HOSTNAME, RE_IDENTIFIER, RE_LOCALE,
    RE_MAC_ADDRESS, RE_SMTP_PROTOCOL, RE_TIMEZONE, TIMEOUT_KEYS,
};

/// JSON Schema of `etc/config.yaml`, pretty-printed (`dnf-generator schema`).
pub fn json_schema() -> String {
    let schema = SchemaSettings::draft2020_12()
        .with_transform(RecursiveTransform(tidy))
        .into_generator()
        .into_root_schema_for::<ConfigFile>();
    serde_json::to_string_pretty(&schema).expect("a JSON Schema always serializes")
}

// Doc comments are wrapped at 80 columns: an editor would keep those breaks.
// `default: null` would make it complete a key as `key: null`.
fn tidy(schema: &mut Schema) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    if let Some(Value::String(text)) = obj.get_mut("description") {
        *text = text
            .split("\n\n")
            .map(|paragraph| paragraph.replace('\n', " "))
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    if obj.get("default") == Some(&Value::Null) {
        obj.remove("default");
    }
}

/// DNF deployment configuration (`etc/config.yaml`). Run `just generate` after
/// every edit.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "DNF configuration")]
pub struct ConfigFile {
    /// Global parameters: domain, default locale, coordination, mail, Matrix,
    /// fleet-update.
    #[serde(default)]
    pub network: Option<NetworkCfg>,

    /// Sub-networks (home LAN, guest LAN, remote site…), by zone name. A host
    /// joins one with `zone:`. An implicit `www` zone holds the hosts declared
    /// with a public `ipv4`.
    #[serde(default)]
    pub zones: Option<IndexMap<String, ZoneCfg>>,

    // Required: the deployment always declares at least an (empty) users map.
    /// Accounts common to all zones, by login. Each login needs a
    /// `usr/users/<login>/default.nix`, even empty. The `nix` deploy account is
    /// implicit: do not declare it.
    pub users: IndexMap<String, UserCfg>,

    /// Machines to deploy: a single host, a `range:` of identical hosts, or a
    /// `hosts:` map of near-identical ones.
    #[serde(default)]
    pub hosts: Vec<HostEntry>,
}

// ─── network ─────────────────────────────────────────────────────────────────

/// Global network parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkCfg {
    /// Top-level DNS domain; each zone gets `<zone>.<domain>`. Choose it once:
    /// Matrix identities, Kanidm and YubiKey origins derive from it.
    #[serde(default)]
    pub domain: Option<String>,

    /// Locale and timezone of every host, unless its zone overrides them.
    #[serde(default)]
    pub default: Option<NetworkDefault>,

    /// Headscale coordination server, meshing remote hosts and zones (tailnet).
    #[serde(default)]
    pub coordination: Option<Coordination>,

    /// Outgoing mail relay of the hosts running `postfix` (system mail, alert
    /// digests). The password lives in sops, key `smtp/password`.
    #[serde(default)]
    pub smtp: Option<Smtp>,

    /// Matrix administration.
    #[serde(default)]
    pub matrix: Option<Matrix>,

    /// Defaults of `fleet-update` (`just fleet-update`, daily timer). Its
    /// command-line options override them.
    #[serde(default, rename = "fleetUpdate")]
    pub fleet_update: Option<FleetUpdate>,
}

/// Default locale and timezone.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkDefault {
    /// Default locale, `xx_XX.UTF-8` (e.g. `en_US.UTF-8`).
    #[serde(default)]
    #[schemars(pattern(RE_LOCALE))]
    pub locale: Option<String>,

    /// Default timezone, `Area/City` (e.g. `Europe/Paris`); UTC is `Etc/UTC`.
    #[serde(default)]
    #[schemars(pattern(RE_TIMEZONE))]
    pub timezone: Option<String>,
}

/// Headscale coordination server.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Coordination {
    /// Use a coordination server. `false`: zones are only deployed locally.
    #[serde(default)]
    pub enable: Option<bool>,

    /// Hostname of the host running `headscale`.
    #[serde(default)]
    #[schemars(pattern(RE_HOSTNAME))]
    pub hostname: Option<String>,

    /// Subdomain of the coordination server: `<domain>.<network.domain>`.
    #[serde(default)]
    #[schemars(pattern(RE_HOSTNAME))]
    pub domain: Option<String>,
}

/// Outgoing mail relay.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Smtp {
    /// `http`, `https`, `submission` or `submissions`.
    #[serde(default)]
    #[schemars(pattern(RE_SMTP_PROTOCOL))]
    pub protocol: Option<String>,

    /// Relay server name (FQDN).
    #[serde(default)]
    #[schemars(pattern(RE_FQDN))]
    pub server: Option<String>,

    /// Relay port (e.g. `465` for `submissions`).
    #[serde(default)]
    pub port: Option<u16>,

    /// Relay login, an email address.
    #[serde(default)]
    pub username: Option<String>,

    /// Sender address of outgoing mail.
    #[serde(default)]
    pub sender: Option<String>,

    /// Use TLS.
    #[serde(default)]
    pub tls: Option<bool>,
}

/// Matrix administration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    /// Local parts of the administrators (MXID `<admin>@<network.domain>`):
    /// bridges and server admin, invited to the alert rooms.
    #[serde(default)]
    pub admins: Option<Vec<String>>,
}

// Same syntax as the `fleet-update` options (`--deployment-order`,
// `--critical-profiles`), which override them.
/// Defaults of `fleet-update`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetUpdate {
    /// Test waves by host profile, `:`-separated; `[others]` stands for the
    /// profiles not listed. Default: `hcs:gateway:server:[others]:laptop`.
    #[serde(default, rename = "deploymentOrder")]
    pub deployment_order: Option<String>,

    /// Host profiles whose failures also reach the incidents room,
    /// `:`-separated. Default: `hcs:gateway:server`.
    #[serde(default, rename = "criticalProfiles")]
    pub critical_profiles: Option<String>,

    // Spec § Délais. Keys checked against `TIMEOUT_KEYS`: a typo would fall
    // back to a default in silence.
    /// Longest run of an operation, in seconds, by operation.
    #[serde(default)]
    #[schemars(schema_with = "timeouts_schema")]
    pub timeouts: Option<IndexMap<String, i64>>,

    // Spec § Présence.
    /// Seconds between two pings of the hosts being watched.
    #[serde(default, rename = "pingInterval")]
    #[schemars(range(min = 1))]
    pub ping_interval: Option<i64>,

    // Spec § Intégration IA. Syntax only: the tool reports a model or an
    // effort it does not know.
    /// AI of `--ai-model`: `<tool>[:<model>][@<effort>]`, tool `claude` or
    /// `opencode`.
    #[serde(default, rename = "aiModel")]
    #[schemars(pattern(RE_AI_MODEL))]
    pub ai_model: Option<String>,
}

// ─── zones ───────────────────────────────────────────────────────────────────

/// A sub-network.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoneCfg {
    /// Human-readable description.
    #[serde(default)]
    pub description: Option<String>,

    /// Locale of the zone hosts; defaults to `network.default.locale`.
    #[serde(default)]
    pub locale: Option<String>,

    /// Timezone of the zone hosts; defaults to `network.default.timezone`.
    #[serde(default)]
    pub timezone: Option<String>,

    /// First two octets of the zone's RFC 1918 /16 (e.g. `10.0` for
    /// 10.0.0.0/16).
    #[serde(default, rename = "ipPrefix")]
    pub ip_prefix: Option<String>,

    /// Turns one host of the zone into its router: `wan` and at least one
    /// `lan` interface are required.
    #[serde(default)]
    pub gateway: Option<Gateway>,

    /// Non-NixOS devices reachable by name (phones, printers, IoT…): fixed IP,
    /// DHCP reservation, DNS record. By device name.
    #[serde(default, rename = "extraHosts")]
    pub extra_hosts: Option<IndexMap<String, ExtraHost>>,
}

/// Router interfaces of a zone.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Gateway {
    /// Uplink side.
    #[serde(default)]
    pub wan: Option<GatewayWan>,

    /// Internal side.
    #[serde(default)]
    pub lan: Option<GatewayLan>,

    /// Tailnet side.
    #[serde(default)]
    pub vpn: Option<GatewayVpn>,
}

/// Uplink side of the gateway.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GatewayWan {
    /// Uplink NIC of the gateway host (e.g. `enp1s0`).
    #[serde(default)]
    pub interface: Option<String>,
}

/// Internal side of the gateway.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GatewayLan {
    /// Internal NIC(s), bridged together.
    #[serde(default)]
    pub interfaces: Option<Vec<String>>,

    /// DHCP ranges, `start,end,lease`. Default:
    /// `<ipPrefix>.3.200,<ipPrefix>.3.249,24h`.
    #[serde(default, rename = "dhcp-range")]
    pub dhcp_range: Option<Vec<String>>,
}

/// Tailnet side of the gateway.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GatewayVpn {
    /// Tailnet address of the gateway (100.64.0.0/10).
    #[serde(default)]
    pub ipv4: Option<String>,
}

/// A non-NixOS device of the zone.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtraHost {
    /// Suffix in the zone: the full IP is `<ipPrefix>.<ip>` (e.g. `5.1`).
    pub ip: String,

    /// Human-readable name.
    #[serde(default)]
    pub name: Option<String>,

    /// MAC address, lowercase hex; required for the DHCP reservation.
    #[serde(default)]
    #[schemars(pattern(RE_MAC_ADDRESS))]
    pub mac: Option<String>,

    /// Extra DNS names.
    #[serde(default)]
    pub aliases: Option<Vec<String>>,

    /// Services this device provides, by name; the optional value customizes
    /// the portal entry and DNS.
    #[serde(default)]
    pub services: Option<IndexMap<String, Option<ServiceCfg>>>,
}

// ─── users ───────────────────────────────────────────────────────────────────

/// A user account.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserCfg {
    /// Unique uid across the deployment, 1000 to 64999.
    #[schemars(range(min = 1000, max = 64999))]
    pub uid: u32,

    /// Full name.
    pub name: String,

    /// Email address.
    #[serde(default)]
    pub email: Option<String>,

    /// Home profile: built-in (`nix-admin`, `admin`, `advanced`, `normal`,
    /// `gamer`, `student`, `teenager`, `child`, `baby`, `minimal`) or yours,
    /// under `usr/home/profiles/<name>/`.
    #[serde(default)]
    pub profile: Option<String>,

    /// Groups: a host authorizes the users sharing one of its groups.
    #[serde(default)]
    pub groups: Vec<String>,

    // `true` is emitted as `disabled = true;` in `users.nix`; `false` (the
    // default) is omitted entirely.
    /// Keep the user declared, but lock the account.
    #[serde(default)]
    pub disabled: bool,
}

// ─── hosts ───────────────────────────────────────────────────────────────────

// The three flavours (static / `range:` / `hosts:` list) share one closed
// struct; the dispatch is done in `configuration.rs` by checking which of
// `range` / `hosts` is present. All fields are optional so any flavour
// validates, while `deny_unknown_fields` still rejects typos.
/// A machine to deploy, or a group of identical ones (`range:`, `hosts:`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    /// Required. DNS-friendly identifier: a letter, then letters, digits, `_`
    /// or `-`. Templated with `%d` or `%'02s` for `range:`, `%s` for `hosts:`.
    #[serde(default)]
    pub hostname: Option<String>,

    /// Human-readable description.
    #[serde(default)]
    pub name: Option<String>,

    /// Required. Host profile: built-in (`minimal`, `server`, `gateway`,
    /// `hcs`, `desktop`, `laptop`, `portable`, `vm`) or yours.
    #[serde(default)]
    pub profile: Option<String>,

    /// `<zone>` for DHCP, or `<zone>:<X.Y>` for the fixed IP
    /// `<ipPrefix>.<X.Y>`. Exclusive with `ipv4`.
    #[serde(default)]
    pub zone: Option<String>,

    /// Host outside your zones (e.g. a VPS), in the implicit `www` zone.
    /// Exclusive with `zone`.
    #[serde(default)]
    pub ipv4: Option<Ipv4Cfg>,

    /// `x86_64` (default), or `aarch64:rpi5` / `rpi4` / `rpi3` / `rpi02` for a
    /// Raspberry Pi.
    #[serde(default)]
    #[schemars(schema_with = "arch_schema")]
    pub arch: Option<String>,

    /// Extra DNS names in the host's zone.
    #[serde(default)]
    pub aliases: Option<Vec<String>>,

    /// Host groups: users sharing one of them are authorized on this host.
    #[serde(default)]
    pub groups: Option<Vec<String>>,

    /// Logins authorized on this host.
    #[serde(default)]
    pub users: Option<Vec<String>>,

    /// Colmena tags, for a selective deployment.
    #[serde(default)]
    pub tags: Option<Vec<String>>,

    /// Non-service flags, `name` or `name:value` (value defaults to the host's
    /// zone): `monitoring-node`, `nfs-client`, `build-farm`, `alert-critical`,
    /// `alert-non-critical`, `alert-disabled`…
    #[serde(default)]
    pub features: Option<Vec<String>>,

    /// MAC address(es) of the internal NIC(s).
    #[serde(default)]
    pub mac: Option<Mac>,

    /// Services enabled on this host, by name (e.g. `nextcloud:`); the optional
    /// value customizes the portal entry and DNS.
    #[serde(default)]
    pub services: Option<IndexMap<String, Option<ServiceCfg>>>,

    /// Disk layout, read at install time only, frozen afterwards.
    #[serde(default)]
    pub disko: Option<DiskoCfg>,

    /// `[from, to]`: identical hosts, numbered in `hostname`, `name` and
    /// `zone` (`%d`, `%'02s`).
    #[serde(default)]
    #[schemars(length(equal = 2))]
    pub range: Option<Vec<i64>>,

    /// Near-identical hosts by name (`%s` in `hostname`), each overriding
    /// fields of this entry.
    #[serde(default)]
    pub hosts: Option<IndexMap<String, HostEntry>>,
}

/// Host outside the local zones.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ipv4Cfg {
    /// Public IPv4 address.
    #[serde(default)]
    pub external: Option<String>,

    /// Tailnet address (100.64.0.0/10).
    #[serde(default)]
    pub internal: Option<String>,
}

// Single string for static hosts, per-index map for `range:` groups.
/// One address, or several comma-separated; for a `range:` group, a map from
/// index to address.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Mac {
    Single(String),
    Indexed(IndexMap<i64, String>),
}

/// Portal entry and DNS name of a service.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceCfg {
    /// Title of the portal (homepage) entry.
    #[serde(default)]
    pub title: Option<String>,

    /// Description of the portal entry.
    #[serde(default)]
    pub description: Option<String>,

    /// Subdomain label; defaults to the service name.
    #[serde(default)]
    pub domain: Option<String>,

    /// Icon of the portal entry.
    #[serde(default)]
    pub icon: Option<String>,

    /// Publish as `<domain>.<network.domain>` instead of
    /// `<domain>.<zone>.<network.domain>`.
    #[serde(default)]
    pub global: bool,
}

/// Disk layout of a host.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiskoCfg {
    /// Layout of `usr/hosts/disko/` or `dnf/hosts/disko/`: `btrfs-1-disk`,
    /// `ext4-1-disk`, `ext4-1-disk-legacy`, `luks-btrfs-1-disk`,
    /// `server-1-disk`, `server-4-disks`.
    #[serde(default)]
    #[schemars(pattern(RE_IDENTIFIER))]
    pub profile: Option<String>,

    /// One block device per disk of the profile, by disk name
    /// (`/dev/disk/by-id/…` recommended).
    #[serde(default)]
    #[schemars(schema_with = "devices_schema")]
    pub devices: Option<IndexMap<String, String>>,
}

// ─── schema helpers ──────────────────────────────────────────────────────────

// Closed lists come from the `validation` constants, so the editor completes
// exactly what the generator accepts. `null` stays valid: a bare `key:` is.

fn arch_schema(_: &mut SchemaGenerator) -> Schema {
    let mut values: Vec<serde_json::Value> = ALLOWED_ARCH.iter().map(|a| (*a).into()).collect();
    values.push(serde_json::Value::Null);
    json_schema!({ "type": ["string", "null"], "enum": values })
}

fn timeouts_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["object", "null"],
        "propertyNames": { "enum": TIMEOUT_KEYS },
        "additionalProperties": { "type": "integer", "minimum": 1 }
    })
}

fn devices_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["object", "null"],
        "propertyNames": { "pattern": RE_IDENTIFIER },
        "additionalProperties": { "type": "string", "pattern": RE_DEVICE }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        serde_json::from_str(&json_schema()).unwrap()
    }

    /// Root object, then every definition: `(name, schema)`.
    fn objects(schema: &Value) -> Vec<(String, &Value)> {
        let mut objects = vec![("ConfigFile".to_string(), schema)];
        for (name, def) in schema["$defs"].as_object().unwrap() {
            objects.push((name.clone(), def));
        }
        objects
    }

    #[test]
    fn root_is_draft_2020_12_and_requires_users() {
        let schema = schema();
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(schema["required"], serde_json::json!(["users"]));
    }

    // An undocumented key shows nothing on hover in the editor.
    #[test]
    fn every_property_has_a_description() {
        let schema = schema();
        let mut missing = Vec::new();
        for (name, object) in objects(&schema) {
            for (key, property) in object["properties"].as_object().into_iter().flatten() {
                let described = property["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty());
                if !described {
                    missing.push(format!("{name}.{key}"));
                }
            }
        }
        assert!(missing.is_empty(), "undocumented: {missing:?}");
    }

    // `deny_unknown_fields` everywhere: the editor flags the typos the
    // generator rejects.
    #[test]
    fn every_struct_rejects_unknown_keys() {
        let schema = schema();
        for (name, object) in objects(&schema) {
            if object.get("properties").is_some() {
                assert_eq!(object["additionalProperties"], false, "{name}");
            }
        }
    }

    #[test]
    fn closed_lists_follow_the_validation_constants() {
        let schema = schema();
        let host = &schema["$defs"]["HostEntry"]["properties"];
        let mut arch: Vec<Value> = ALLOWED_ARCH.iter().map(|a| (*a).into()).collect();
        arch.push(Value::Null);
        assert_eq!(host["arch"]["enum"], Value::Array(arch));

        let timeouts = &schema["$defs"]["FleetUpdate"]["properties"]["timeouts"];
        assert_eq!(
            timeouts["propertyNames"]["enum"],
            serde_json::json!(TIMEOUT_KEYS)
        );
    }

    #[test]
    fn descriptions_are_unwrapped_and_null_defaults_dropped() {
        let text = json_schema();
        let schema: Value = serde_json::from_str(&text).unwrap();
        for (name, object) in objects(&schema) {
            for (key, property) in object["properties"].as_object().into_iter().flatten() {
                let description = property["description"].as_str().unwrap_or_default();
                assert!(
                    !description.replace("\n\n", "").contains('\n'),
                    "{name}.{key}"
                );
                assert_ne!(property.get("default"), Some(&Value::Null), "{name}.{key}");
            }
        }
    }
}
