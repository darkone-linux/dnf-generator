//! The project `darkone add gateway` runs in, and its guards: found from the
//! working directory, off the installer image, git clean, one local zone
//! without a gateway yet.
//!
//! The configuration is read by the generator itself (`Configuration::load`):
//! the same zone, prefix and gateway detection (`<zone>:1.1`) as `just generate`.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::plan::{AdminMac, GatewayPlan, DISKO_DISK, GATEWAY_PROFILE};
use super::Result;
use crate::installer::INSTALLER_MARKER;
use crate::nix_generator::configuration::Configuration;
use crate::nix_generator::nix_service::ServiceRegistry;

/// What the plan needs of the project.
#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    /// The only local zone, created by `darkone new`.
    pub zone: String,

    /// `/16` prefix of the zone, `10.1`.
    pub ip_prefix: String,
    pub hosts: Vec<KnownHost>,
}

/// A declared host, ranges and lists expanded.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownHost {
    pub hostname: String,
    pub zone: String,

    /// Fixed address, `10.1.2.1`; `None`: a DHCP lease.
    pub ip: Option<String>,

    /// Comma-separated MACs, as `mac:` takes them.
    pub mac: Option<String>,
}

impl Network {
    pub fn host(&self, hostname: &str) -> Option<&KnownHost> {
        self.hosts.iter().find(|h| h.hostname == hostname)
    }

    /// The host holding `<prefix>.<suffix>` in the zone.
    pub fn holder(&self, suffix: &str) -> Option<&KnownHost> {
        let ip = format!("{}.{suffix}", self.ip_prefix);
        self.hosts
            .iter()
            .find(|h| h.zone == self.zone && h.ip.as_deref() == Some(ip.as_str()))
    }
}

/// First directory up from `start` holding `etc/config.yaml` and a `Justfile`:
/// where `just` finds the project recipes.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|d| d.join("etc/config.yaml").is_file() && d.join("Justfile").is_file())
        .map(Path::to_path_buf)
}

/// Every guard reads its facts here; the verdicts below stay pure.
pub fn on_installer() -> bool {
    Path::new(INSTALLER_MARKER).exists()
}

/// `git status --porcelain` of the project.
pub fn git_status(root: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{} is not a git repository: `git status` failed",
            root.display()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The commit of the gateway must hold its own change only.
pub fn dirty(porcelain: &str) -> Option<String> {
    let lines: Vec<&str> = porcelain.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let shown = lines
        .iter()
        .take(5)
        .copied()
        .collect::<Vec<_>>()
        .join("\n    ");
    let more = if lines.len() > 5 {
        format!("\n    … {} more", lines.len() - 5)
    } else {
        String::new()
    };
    Some(format!(
        "the project has uncommitted changes, commit or stash them first:\n    {shown}{more}"
    ))
}

/// Where the configuration lives in a project.
pub const CONFIG: &str = "etc/config.yaml";

/// Merged configuration, through the service registry of the linked framework
/// (`dnf/config/modules.nix`), as `just generate` reads it. `text`: the file,
/// or its edit before it is written.
pub fn load_config(root: &Path, text: &str) -> Result<Configuration> {
    let registry_path = root.join("dnf/config/modules.nix");
    let registry = ServiceRegistry::load(&registry_path).map_err(|e| {
        format!(
            "cannot load {}: {e}; is dnf/ linked (nix run .#init)?",
            registry_path.display()
        )
    })?;
    Configuration::load_text(
        text,
        &root.join("var/generated/config.yaml"),
        root,
        registry,
    )
    .map_err(|e| format!("{CONFIG}: {e}"))
}

pub fn load_network(root: &Path) -> Result<Network> {
    let config = root.join(CONFIG);
    let text =
        std::fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    network_of(&load_config(root, &text)?)
}

/// The edit, read by the generator before it is written: it must load, and
/// say what the plan says.
pub fn check_edit(root: &Path, edited: &str, plan: &GatewayPlan) -> Result<()> {
    let refused = |why: String| format!("the edited {CONFIG} is refused, nothing written: {why}");
    let cfg = load_config(root, edited).map_err(refused)?;
    let text = |v: Option<&serde_yaml::Value>| v.and_then(|v| v.as_str()).map(str::to_string);
    let gateway = cfg
        .network
        .zones
        .get(&plan.zone)
        .and_then(|z| z.config.get("gateway"));
    let part = |a: &str, b: &str| text(gateway.and_then(|g| g.get(a)).and_then(|g| g.get(b)));
    let host = cfg.hosts.get(&plan.hostname);
    let admin = cfg.hosts.get(&plan.admin.hostname);
    let lan: Vec<String> = gateway
        .and_then(|g| g.get("lan"))
        .and_then(|l| l.get("interfaces"))
        .and_then(|i| i.as_sequence())
        .map(|s| s.iter().filter_map(|v| text(Some(v))).collect())
        .unwrap_or_default();

    // The generator resolves the layout to `dnf/hosts/disko/<profile>.nix`.
    let layout = host
        .and_then(|h| h.disko.profile.as_deref())
        .and_then(|p| Path::new(p).file_stem())
        .map(|p| p.to_string_lossy().into_owned());
    let mut facts = vec![
        (
            "gateway of the zone",
            text(gateway.and_then(|g| g.get("hostname"))),
            Some(plan.hostname.clone()),
        ),
        (
            "its LAN address",
            part("lan", "ip"),
            Some(plan.lan_ip.clone()),
        ),
        (
            "its WAN card",
            part("wan", "interface"),
            Some(plan.wan.name.clone()),
        ),
        (
            "its LAN cards",
            Some(lan.join(",")),
            Some(
                plan.lan
                    .iter()
                    .map(|n| n.name.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        ),
        (
            "its address",
            host.and_then(|h| h.ip.clone()),
            Some(plan.lan_ip.clone()),
        ),
        (
            "its profile",
            host.map(|h| h.profile.clone()),
            Some(GATEWAY_PROFILE.to_string()),
        ),
        ("its disk layout", layout, Some(plan.disko_profile.clone())),
        (
            "its disk",
            host.and_then(|h| h.disko.devices.get(DISKO_DISK).cloned()),
            Some(plan.disk.path.clone()),
        ),
        (
            "address of the admin host",
            admin.and_then(|h| h.ip.clone()),
            Some(plan.admin.ip.clone()),
        ),
    ];
    match &plan.admin.mac {
        AdminMac::Declared(mac) | AdminMac::Add(mac) => facts.push((
            "MAC of the admin host",
            admin.and_then(|h| h.mac.clone()),
            Some(mac.clone()),
        )),
        AdminMac::Unknown => {}
    }
    let wrong: Vec<String> = facts
        .into_iter()
        .filter(|(_, got, want)| got != want)
        .map(|(what, got, want)| format!("{what}: {got:?}, expected {want:?}"))
        .collect();
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(refused(format!(
            "the generator does not read the plan back ({})",
            wrong.join("; ")
        )))
    }
}

/// The zone of `darkone new`, still without a gateway: the only case
/// `darkone add gateway` handles (other zones need a HCS, step 4).
pub fn network_of(cfg: &Configuration) -> Result<Network> {
    let mut zones: Vec<_> = cfg
        .network
        .zones
        .values()
        .filter(|z| !z.is_external())
        .collect();
    zones.sort_by(|a, b| a.name().cmp(b.name()));
    let zone = match zones.as_slice() {
        [only] => *only,
        [] => return Err("etc/config.yaml declares no zone: run darkone new first".to_string()),
        many => {
            let names: Vec<&str> = many.iter().map(|z| z.name()).collect();
            return Err(format!(
                "several zones ({}): darkone add gateway installs the first gateway of the \
                 zone darkone new created; declare the others by hand",
                names.join(", ")
            ));
        }
    };

    // `gateway` lands in the zone config from a declared block or from a host
    // at `.1.1`, the generator's convention.
    if let Some(gateway) = zone.config.get("gateway") {
        return Err(
            match gateway.get("hostname").and_then(serde_yaml::Value::as_str) {
                Some(host) => format!(
                    "zone '{}' already has a gateway, {host}: darkone add gateway installs \
                     the first one only (reinstall: just full-install {host})",
                    zone.name()
                ),
                None => format!(
                    "zone '{0}' declares zones.{0}.gateway, but no host at {1}.1.1: \
                     finish it by hand, or remove the block first",
                    zone.name(),
                    zone.ip_prefix()
                ),
            },
        );
    }
    let mut hosts: Vec<KnownHost> = cfg
        .hosts
        .values()
        .map(|h| KnownHost {
            hostname: h.hostname.clone(),
            zone: h.zone.clone(),
            ip: h.ip.clone(),
            mac: h.mac.clone(),
        })
        .collect();
    hosts.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Ok(Network {
        zone: zone.name().to_string(),
        ip_prefix: zone.ip_prefix().to_string(),
        hosts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_found_upwards() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::create_dir_all(root.join("usr/machines/gw")).unwrap();
        std::fs::write(root.join("etc/config.yaml"), "users: {}\n").unwrap();
        assert_eq!(find_root(&root.join("usr/machines/gw")), None);
        std::fs::write(root.join("Justfile"), "").unwrap();
        assert_eq!(
            find_root(&root.join("usr/machines/gw")),
            Some(root.to_path_buf())
        );
        assert_eq!(find_root(root), Some(root.to_path_buf()));
        assert_eq!(find_root(Path::new("/proc")), None);
    }

    #[test]
    fn clean_tree_only() {
        assert_eq!(dirty(""), None);
        assert_eq!(dirty("\n"), None);
        let one = dirty(" M etc/config.yaml\n").unwrap();
        assert!(one.contains("uncommitted changes"), "{one}");
        assert!(one.contains("etc/config.yaml"), "{one}");
        let many: String = (0..8).map(|i| format!("?? file{i}\n")).collect();
        let many = dirty(&many).unwrap();
        assert!(many.contains("file4") && !many.contains("file5"), "{many}");
        assert!(many.contains("3 more"), "{many}");
    }

    #[test]
    fn git_status_needs_a_repository() {
        let dir = tempfile::TempDir::new().unwrap();
        let Ok(out) = Command::new("git").arg("--version").output() else {
            return;
        };
        if !out.status.success() {
            return;
        }
        assert!(git_status(dir.path()).is_err());
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(init.success());
        assert_eq!(git_status(dir.path()).unwrap(), "");
        std::fs::write(dir.path().join("new"), "").unwrap();
        assert!(dirty(&git_status(dir.path()).unwrap()).is_some());
    }
}
