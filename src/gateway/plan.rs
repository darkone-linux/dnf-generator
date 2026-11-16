//! Plan of `darkone add gateway`: answers, probe and project checked as a
//! whole, every problem at once, before anything is written.
//!
//! [`GatewayPlan`] holds all the writing step needs: the `yq` edit of
//! `etc/config.yaml` and the `just full-install` chain (spec 2b.5, steps 5-8).

use std::net::Ipv4Addr;

use regex::Regex;

use super::answers::{GatewayAnswers, GATEWAY_ANSWERS_VERSION};
use super::probe::{Probe, ProbeDisk, ProbeNic};
use super::project::Network;
use super::write::{valid_device, valid_nic};
use crate::installer::machine::{human_size, Ipv4Net, MIN_INSTALL_SIZE};
use crate::nix_generator::validation::RE_HOSTNAME;

/// Proposed name of the first gateway.
pub const DEFAULT_HOSTNAME: &str = "gw";

/// Host profile of the gateway (`dnf/modules/host/gateway.nix`).
pub const GATEWAY_PROFILE: &str = "gateway";

/// Address of a zone gateway in its zone: the generator detects it by it.
pub const GATEWAY_SUFFIX: &str = "1.1";

/// Fixed address of the admin host, reserved on its MAC by the gateway.
pub const ADMIN_SUFFIX: &str = "2.1";

/// UEFI, btrfs, 16 GB swap: one disk, `disko.devices.main`.
pub const DISKO_PROFILE: &str = "server-1-disk";
pub const DISKO_DISK: &str = "main";

/// Width of a zone network (`LAN_PREFIX_LENGTH` of the generator).
const ZONE_PREFIX_LEN: u8 = 16;

/// What the probe suggests, before any answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposals {
    /// Wired card of the default route; `None`: no route, or through Wi-Fi.
    pub wan: Option<String>,

    /// Wi-Fi card holding the default route: never a WAN, said so.
    pub route_on_wifi: Option<String>,

    /// Disks of 20 GB at least, the only install targets.
    pub disks: Vec<ProbeDisk>,
}

pub fn propose(probe: &Probe) -> Proposals {
    Proposals {
        wan: probe.wired().find(|n| n.route).map(|n| n.name.clone()),
        route_on_wifi: probe
            .nics
            .iter()
            .find(|n| n.route && n.wifi)
            .map(|n| n.name.clone()),
        disks: probe
            .disks
            .iter()
            .filter(|d| d.size_bytes >= MIN_INSTALL_SIZE)
            .cloned()
            .collect(),
    }
}

/// LAN proposal once the WAN is known: every other wired card.
pub fn lan_proposal(probe: &Probe, wan: Option<&str>) -> Vec<String> {
    probe
        .wired()
        .filter(|n| Some(n.name.as_str()) != wan)
        .map(|n| n.name.clone())
        .collect()
}

/// The machine `darkone` runs on, the admin host of the network.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminMachine {
    /// Short hostname, the name of its `hosts` entry.
    pub hostname: String,

    /// MACs of its physical cards, wired first.
    pub macs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanNic {
    pub name: String,
    pub mac: String,
    pub link: bool,

    /// Carries the default route of the installer image.
    pub route: bool,
    pub ipv4: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanDisk {
    /// `disko.devices.main`: `by_id`, else the kernel name.
    pub path: String,
    pub dev: String,

    /// `path` is a `/dev/disk/by-id` link, stable across reboots.
    pub stable: bool,
    pub label: String,
}

/// The `mac:` key of the admin host.
#[derive(Debug, Clone, PartialEq)]
pub enum AdminMac {
    /// Already in `etc/config.yaml` (point 1.10): left as is.
    Declared(String),

    /// Read on this machine, to be written: comma-separated, wired first.
    Add(String),

    /// None declared, none readable: no reservation, a random lease.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminPlan {
    pub hostname: String,

    /// `zone:` value of its entry, `main:2.1`.
    pub zone_spec: String,

    /// `10.1.2.1`.
    pub ip: String,

    /// `zone:` to rewrite: it had no fixed address yet.
    pub new_address: bool,
    pub mac: AdminMac,
}

/// Everything the writing and the install need, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayPlan {
    pub zone: String,
    pub ip_prefix: String,
    pub hostname: String,

    /// `zone:` of the gateway entry, `main:1.1`.
    pub zone_spec: String,

    /// LAN address, `10.1.1.1`: the gateway the zone hosts get.
    pub lan_ip: String,

    /// Current address on the installer image, box side.
    pub target: Ipv4Addr,

    /// ed25519 host key `_pair` went through (`ssh-ed25519 AAAA…`); `None`
    /// with `--probe`.
    pub host_key: Option<String>,

    /// `zones.<zone>.gateway.wan.interface`.
    pub wan: PlanNic,

    /// `zones.<zone>.gateway.lan.interfaces`, bridged.
    pub lan: Vec<PlanNic>,
    pub disk: PlanDisk,

    /// `disko.profile`, the disk under `disko.devices.<DISKO_DISK>`.
    pub disko_profile: String,
    pub admin: AdminPlan,

    /// From the probe, shown in the recap.
    pub virt: String,
    pub ram_bytes: u64,

    /// Not blocking, shown with the recap.
    pub warnings: Vec<String>,
}

type Errors = Vec<String>;

impl GatewayPlan {
    pub fn resolve(
        answers: &GatewayAnswers,
        probe: &Probe,
        network: &Network,
        admin: &AdminMachine,
        host_key: Option<String>,
    ) -> std::result::Result<GatewayPlan, Errors> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let proposals = propose(probe);

        match answers.version {
            None => errors.push(format!(
                "missing answer: version ({GATEWAY_ANSWERS_VERSION})"
            )),
            Some(GATEWAY_ANSWERS_VERSION) => {}
            Some(v) => errors.push(format!(
                "version {v} is not supported, expected {GATEWAY_ANSWERS_VERSION}"
            )),
        }
        let target = match answers.address.as_deref().map(str::parse::<Ipv4Addr>) {
            None => {
                errors.push("missing answer: address, the IPv4 its screen shows".to_string());
                None
            }
            Some(Err(_)) => {
                errors.push(format!(
                    "address: '{}' is not an IPv4 address",
                    answers.address.as_deref().unwrap_or_default()
                ));
                None
            }
            Some(Ok(ip)) => Some(ip),
        };

        let hostname = answers
            .host
            .hostname
            .clone()
            .unwrap_or_else(|| DEFAULT_HOSTNAME.to_string());
        if !Regex::new(RE_HOSTNAME).unwrap().is_match(&hostname) {
            errors.push(format!("host.hostname: invalid value '{hostname}'"));
        } else if network.host(&hostname).is_some() {
            errors.push(format!(
                "host.hostname: '{hostname}' is already a host of etc/config.yaml"
            ));
        }

        // `server-1-disk` boots from an EFI partition.
        if !probe.uefi {
            errors.push(format!(
                "the machine boots in BIOS mode, layout {DISKO_PROFILE} needs UEFI: \
                 switch its firmware to UEFI, then boot the installer again"
            ));
        }

        let nics = nics(answers, probe, &proposals, &mut errors, &mut warnings);
        let disk = disk(answers, probe, &proposals, &mut errors, &mut warnings);
        if let (Some(target), Some((wan, _))) = (target, &nics) {
            box_overlap(target, wan, network, &mut errors);
        }
        let admin = admin_plan(network, admin, &mut errors, &mut warnings);

        if !errors.is_empty() {
            return Err(errors);
        }
        let (wan, lan) = nics.expect("checked above");
        Ok(GatewayPlan {
            zone: network.zone.clone(),
            ip_prefix: network.ip_prefix.clone(),
            zone_spec: format!("{}:{GATEWAY_SUFFIX}", network.zone),
            lan_ip: format!("{}.{GATEWAY_SUFFIX}", network.ip_prefix),
            hostname,
            target: target.expect("checked above"),
            host_key,
            wan,
            lan,
            disk: disk.expect("checked above"),
            disko_profile: DISKO_PROFILE.to_string(),
            admin: admin.expect("checked above"),
            virt: probe.virt.clone(),
            ram_bytes: probe.ram_bytes,
            warnings,
        })
    }

    /// Definitive choices are flagged: nothing changes them afterwards (D5).
    pub fn recap(&self) -> String {
        let names = |nics: &[PlanNic]| {
            nics.iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let machine = [
            Some(format!("{} now", self.target)),
            Some(match self.virt.as_str() {
                "none" => "physical".to_string(),
                v => format!("virtual ({v})"),
            }),
            (self.ram_bytes > 0).then(|| format!("{} RAM", human_size(self.ram_bytes))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        let admin = &self.admin;
        let address = if admin.new_address {
            format!("fixed address {} ({}), new", admin.ip, admin.zone_spec)
        } else {
            format!("fixed address {} ({}), kept", admin.ip, admin.zone_spec)
        };
        let mac = match &admin.mac {
            AdminMac::Declared(m) => format!("MAC {m}"),
            AdminMac::Add(m) => format!("MAC {m}, added"),
            AdminMac::Unknown => "no MAC: a random lease".to_string(),
        };
        let mut rows = vec![
            (
                "Zone",
                format!(
                    "{}, {}.0.0/{ZONE_PREFIX_LEN}, definitive",
                    self.zone, self.ip_prefix
                ),
            ),
            (
                "Gateway",
                format!(
                    "{}, {} ({}), profile {GATEWAY_PROFILE}",
                    self.hostname, self.lan_ip, self.zone_spec
                ),
            ),
            ("Machine", machine),
            (
                "WAN",
                format!(
                    "{} ({}), to the box{}",
                    self.wan.name,
                    self.wan.mac,
                    if self.wan.route {
                        ", default route"
                    } else {
                        ""
                    }
                ),
            ),
            (
                "LAN",
                format!(
                    "{}: bridged, {}, DHCP and DNS of the zone",
                    names(&self.lan),
                    self.lan_ip
                ),
            ),
            (
                "Disk",
                format!("{} ({}), ERASED", self.disk.path, self.disk.label),
            ),
            (
                "Layout",
                format!(
                    "{}: UEFI, btrfs, 16 GB swap, definitive",
                    self.disko_profile
                ),
            ),
            ("Admin", format!("{}: {address}, {mac}", admin.hostname)),
            (
                "Then",
                format!(
                    "rewire this machine to a LAN port of {} when asked",
                    self.hostname
                ),
            ),
        ];
        for w in &self.warnings {
            rows.push(("Warning", w.clone()));
        }
        rows.iter()
            .map(|(key, value)| format!("{key:<10} {value}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn plan_nic(n: &ProbeNic) -> PlanNic {
    PlanNic {
        name: n.name.clone(),
        mac: n.mac.clone(),
        link: n.link,
        route: n.route,
        ipv4: n.ipv4.clone(),
    }
}

/// `enp1s0 (aa:bb:…, link)`, for the error messages.
fn describe(n: &ProbeNic) -> String {
    let link = if n.link { "link" } else { "no link" };
    format!("{} ({}, {link})", n.name, n.mac)
}

fn wired_list(probe: &Probe) -> String {
    probe.wired().map(describe).collect::<Vec<_>>().join(", ")
}

/// WAN, then LAN; `None` once an error is recorded.
fn nics(
    answers: &GatewayAnswers,
    probe: &Probe,
    proposals: &Proposals,
    errors: &mut Errors,
    warnings: &mut Errors,
) -> Option<(PlanNic, Vec<PlanNic>)> {
    let wired: Vec<&ProbeNic> = probe.wired().collect();
    if wired.len() < 2 {
        let found = if wired.is_empty() {
            "no wired card".to_string()
        } else {
            format!("one wired card only, {}", wired_list(probe))
        };
        errors.push(format!(
            "{found}: a gateway needs two, one to the box (WAN) and one for the zone (LAN)"
        ));
        return None;
    }
    if let Some(wifi) = &proposals.route_on_wifi {
        warnings.push(format!(
            "the default route goes through Wi-Fi ({wifi}): never a WAN, a wired card is"
        ));
    }

    let wan = match (&answers.nics.wan, &proposals.wan) {
        (Some(w), _) => w.clone(),
        (None, Some(w)) => w.clone(),
        (None, None) => {
            errors.push(format!(
                "missing answer: nics.wan, no wired card carries the default route; one of {}",
                wired_list(probe)
            ));
            return None;
        }
    };
    let wan_nic = match probe.nic(&wan) {
        Some(n) if n.wifi => {
            errors.push(format!("nics.wan: '{wan}' is a Wi-Fi card, a WAN is wired"));
            None
        }
        Some(n) => Some(n),
        None => {
            errors.push(format!(
                "nics.wan: no card '{wan}' on the machine; one of {}",
                wired_list(probe)
            ));
            None
        }
    }?;
    match &proposals.wan {
        Some(route) if *route != wan => warnings.push(format!(
            "WAN {wan} does not carry the default route, {route} does: check the cables"
        )),
        None => warnings.push(format!(
            "no card carries the default route (bond, VLAN, no DHCP?): WAN {wan} as answered"
        )),
        _ => {}
    }
    if !wan_nic.link {
        warnings.push(format!("WAN {wan} has no link: plug it into the box"));
    }

    let Some(lan) = &answers.nics.lan else {
        errors.push(format!(
            "missing answer: nics.lan, proposed: {}",
            lan_proposal(probe, Some(&wan)).join(", ")
        ));
        return None;
    };
    if lan.is_empty() {
        errors.push("nics.lan: one card at least".to_string());
        return None;
    }
    let mut lan_nics = Vec::new();
    for (i, name) in lan.iter().enumerate() {
        match probe.nic(name) {
            _ if *name == wan => errors.push(format!("nics.lan: '{name}' is the WAN")),
            _ if lan[..i].contains(name) => errors.push(format!("nics.lan: '{name}' given twice")),
            Some(n) if n.wifi => errors.push(format!(
                "nics.lan: '{name}' is a Wi-Fi card, the bridge takes wired ones"
            )),
            Some(n) => lan_nics.push(plan_nic(n)),
            None => errors.push(format!(
                "nics.lan: no card '{name}' on the machine; one of {}",
                wired_list(probe)
            )),
        }
    }
    if lan_nics.len() != lan.len() {
        return None;
    }

    // Probed names end up in etc/config.yaml and in generated Nix.
    let odd: Vec<String> = std::iter::once(wan_nic.name.as_str())
        .chain(lan_nics.iter().map(|n| n.name.as_str()))
        .filter(|name| !valid_nic(name))
        .map(|name| format!("{name:?}"))
        .collect();
    if !odd.is_empty() {
        errors.push(format!(
            "card name with unexpected characters, not written to etc/config.yaml: {}",
            odd.join(", ")
        ));
        return None;
    }
    Some((plan_nic(wan_nic), lan_nics))
}

fn disk(
    answers: &GatewayAnswers,
    probe: &Probe,
    proposals: &Proposals,
    errors: &mut Errors,
    warnings: &mut Errors,
) -> Option<PlanDisk> {
    let list = || {
        proposals
            .disks
            .iter()
            .map(|d| format!("{} ({})", d.path, d.label()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let min = human_size(MIN_INSTALL_SIZE);
    let chosen = match answers.disk.device.as_deref() {
        Some(device) => match probe.disks.iter().find(|d| d.is(device)) {
            Some(d) if d.size_bytes < MIN_INSTALL_SIZE => {
                errors.push(format!(
                    "disk.device: '{device}' holds {}, {min} at least",
                    human_size(d.size_bytes)
                ));
                None
            }
            Some(d) => Some(d),
            None => {
                errors.push(format!(
                    "disk.device: '{device}' is not a disk of the machine; one of {}",
                    list()
                ));
                None
            }
        },
        None => match proposals.disks.as_slice() {
            [only] => Some(only),
            [] => {
                errors.push(format!(
                    "no disk of {min} at least on the machine (the installer medium is left out)"
                ));
                None
            }
            _ => {
                errors.push(format!("missing answer: disk.device, one of {}", list()));
                None
            }
        },
    }?;
    if !valid_device(&chosen.path) {
        errors.push(format!(
            "disk path with unexpected characters, not written to etc/config.yaml: {:?}",
            chosen.path
        ));
        return None;
    }
    if chosen.by_id.is_none() {
        warnings.push(format!(
            "{} has no /dev/disk/by-id name: the kernel name goes into disko.devices, \
             it may change when a disk is added",
            chosen.dev
        ));
    }
    Some(PlanDisk {
        path: chosen.path.clone(),
        dev: chosen.dev.clone(),
        stable: chosen.by_id.is_some(),
        label: chosen.label(),
    })
}

/// `a.b.c.d/len` of a probed address, the host bits cleared.
fn network_of(cidr: &str) -> Option<Ipv4Net> {
    let (addr, len) = cidr.split_once('/')?;
    let addr: Ipv4Addr = addr.parse().ok()?;
    let prefix_len: u8 = len.parse().ok().filter(|l| *l <= 32)?;
    let mask = u32::MAX
        .checked_shl(32 - u32::from(prefix_len))
        .unwrap_or(0);
    Some(Ipv4Net {
        addr: Ipv4Addr::from(u32::from(addr) & mask),
        prefix_len,
    })
}

/// Box network inside the zone: WAN and LAN would share one network (2a.4).
/// Same rule as `just install` for the target address, plus the networks of
/// the WAN card the probe gives. The zone prefix is final: the box moves.
fn box_overlap(target: Ipv4Addr, wan: &PlanNic, network: &Network, errors: &mut Errors) {
    let Ok(base) = format!("{}.0.0", network.ip_prefix).parse::<Ipv4Addr>() else {
        errors.push(format!(
            "zone '{}': ipPrefix '{}' is not a.b",
            network.zone, network.ip_prefix
        ));
        return;
    };
    let mut boxnets: Vec<String> = wan
        .ipv4
        .iter()
        .filter_map(|c| network_of(c))
        .filter(|n| n.overlaps(base, ZONE_PREFIX_LEN))
        .map(|n| format!("{}/{} on {}", n.addr, n.prefix_len, wan.name))
        .collect();
    let target_net = Ipv4Net {
        addr: target,
        prefix_len: 32,
    };
    if boxnets.is_empty() && target_net.overlaps(base, ZONE_PREFIX_LEN) {
        boxnets.push(format!("address {target}"));
    }
    if !boxnets.is_empty() {
        errors.push(format!(
            "the box network ({}) overlaps zone '{}' ({base}/{ZONE_PREFIX_LEN}): the WAN \
             and LAN of the gateway would share one network. The zone prefix is final: move \
             the box to another network (its LAN subnet or DHCP range), then boot the \
             machine again",
            boxnets.join(", "),
            network.zone
        ));
    }
}

/// Fixed address and MAC of the admin host: the gateway reserves one for it.
fn admin_plan(
    network: &Network,
    admin: &AdminMachine,
    errors: &mut Errors,
    warnings: &mut Errors,
) -> Option<AdminPlan> {
    let Some(host) = network.host(&admin.hostname) else {
        errors.push(format!(
            "this machine, {}, is not a host of etc/config.yaml: run darkone add gateway \
             on the admin host",
            admin.hostname
        ));
        return None;
    };
    if host.zone != network.zone {
        errors.push(format!(
            "admin host {} is in zone '{}', not '{}'",
            host.hostname, host.zone, network.zone
        ));
        return None;
    }
    let prefix = format!("{}.", network.ip_prefix);
    let (zone_spec, ip, new_address) = match &host.ip {
        Some(ip) => match ip.strip_prefix(&prefix) {
            Some(suffix) => (format!("{}:{suffix}", network.zone), ip.clone(), false),
            None => {
                errors.push(format!(
                    "admin host {}: address {ip} outside zone '{}'",
                    host.hostname, network.zone
                ));
                return None;
            }
        },
        None => {
            if let Some(other) = network.holder(ADMIN_SUFFIX) {
                errors.push(format!(
                    "{}:{ADMIN_SUFFIX} is taken by {}: give the admin host {} a fixed \
                     address by hand first",
                    network.zone, other.hostname, host.hostname
                ));
                return None;
            }
            (
                format!("{}:{ADMIN_SUFFIX}", network.zone),
                format!("{prefix}{ADMIN_SUFFIX}"),
                true,
            )
        }
    };
    let mac = match (&host.mac, admin.macs.is_empty()) {
        (Some(m), _) => AdminMac::Declared(m.clone()),
        (None, false) => AdminMac::Add(admin.macs.join(",")),
        (None, true) => {
            warnings.push(format!(
                "no MAC known for {}: no reservation, it takes a random lease behind \
                 the gateway; add mac: to its entry",
                host.hostname
            ));
            AdminMac::Unknown
        }
    };
    Some(AdminPlan {
        hostname: host.hostname.clone(),
        zone_spec,
        ip,
        new_address,
        mac,
    })
}

#[cfg(test)]
mod tests {
    use super::super::project::KnownHost;
    use super::*;

    #[test]
    fn probed_addresses_become_networks() {
        let n = network_of("192.168.1.50/24").unwrap();
        assert_eq!(n.addr, Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(n.prefix_len, 24);
        assert_eq!(
            network_of("10.7.7.7/8").unwrap().addr,
            Ipv4Addr::new(10, 0, 0, 0)
        );
        assert_eq!(network_of("0.0.0.0/0").unwrap().prefix_len, 0);
        assert!(network_of("10.0.0.1").is_none());
        assert!(network_of("10.0.0.1/33").is_none());
        assert!(network_of("host/24").is_none());
    }

    fn network(admin_ip: Option<&str>, admin_mac: Option<&str>) -> Network {
        Network {
            zone: "main".to_string(),
            ip_prefix: "10.1".to_string(),
            hosts: vec![KnownHost {
                hostname: "poste".to_string(),
                zone: "main".to_string(),
                ip: admin_ip.map(str::to_string),
                mac: admin_mac.map(str::to_string),
            }],
        }
    }

    fn machine(macs: &[&str]) -> AdminMachine {
        AdminMachine {
            hostname: "poste".to_string(),
            macs: macs.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn admin_gets_2_1_and_its_mac_unless_declared() {
        let (mut e, mut w) = (Vec::new(), Vec::new());
        let p = admin_plan(
            &network(None, None),
            &machine(&["aa:00", "aa:01"]),
            &mut e,
            &mut w,
        )
        .unwrap();
        assert_eq!(
            (p.zone_spec.as_str(), p.ip.as_str(), p.new_address),
            ("main:2.1", "10.1.2.1", true)
        );
        assert_eq!(p.mac, AdminMac::Add("aa:00,aa:01".to_string()));

        let p = admin_plan(
            &network(Some("10.1.4.7"), Some("bb:00")),
            &machine(&["aa:00"]),
            &mut e,
            &mut w,
        )
        .unwrap();
        assert_eq!(
            (p.zone_spec.as_str(), p.ip.as_str(), p.new_address),
            ("main:4.7", "10.1.4.7", false)
        );
        assert_eq!(p.mac, AdminMac::Declared("bb:00".to_string()));
        assert!(e.is_empty() && w.is_empty(), "{e:?} {w:?}");

        let p = admin_plan(&network(None, None), &machine(&[]), &mut e, &mut w).unwrap();
        assert_eq!(p.mac, AdminMac::Unknown);
        assert!(w[0].contains("no MAC known"), "{w:?}");
    }

    #[test]
    fn admin_must_be_declared_and_2_1_free() {
        let (mut e, mut w) = (Vec::new(), Vec::new());
        let mut other = machine(&[]);
        other.hostname = "laptop".to_string();
        assert!(admin_plan(&network(None, None), &other, &mut e, &mut w).is_none());
        assert!(e[0].contains("not a host of etc/config.yaml"), "{e:?}");

        let mut n = network(None, None);
        n.hosts.push(KnownHost {
            hostname: "nas".to_string(),
            zone: "main".to_string(),
            ip: Some("10.1.2.1".to_string()),
            mac: None,
        });
        e.clear();
        assert!(admin_plan(&n, &machine(&["aa:00"]), &mut e, &mut w).is_none());
        assert!(e[0].contains("main:2.1 is taken by nas"), "{e:?}");
    }
}
