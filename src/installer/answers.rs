//! Answers of `darkone new`, and the install plan resolved from them.
//!
//! The answers file is the contract of `.specs/dnf/projet-procedure-demarrage-dnf.md`
//! (§ Fichier de réponses): a key it gives skips its question. What a machine
//! tells by itself (firmware, battery, single disk) is detected, not asked.

use std::path::Path;

use regex::Regex;
use serde::Deserialize;

use super::catalog::Catalog;
use super::machine::{human_size, Machine};
use super::Result;
use crate::nix_generator::validation::{RE_DEVICE, RE_TIMEZONE};
pub use crate::nix_generator::validation::{RE_FQDN, RE_HOSTNAME, RE_LOGIN, RE_NAME};

/// Format of the answers file, migrated by `darkone upgrade` like `config.yaml`.
pub const ANSWERS_VERSION: u32 = 1;

/// Crypt hash (`mkpasswd`): yescrypt, sha-512, bcrypt. No quote, space or
/// colon: the hash travels inside a single-quoted shell word.
pub const RE_PASSWORD_HASH: &str = r"^\$[0-9a-z]+\$[./0-9A-Za-z$=,+-]+$";
pub const RE_EMAIL: &str = r"^[^@\s]+@[^@\s]+\.[^@\s]+$";

/// Logins of system accounts, never an admin's.
pub const RESERVED_LOGINS: [&str; 2] = ["nix", "root"];

/// Host profiles of a first workstation; a virtual machine stays a desktop.
const WORKSTATION_PROFILES: [&str; 2] = ["laptop", "desktop"];

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Answers {
    pub version: Option<u32>,
    pub admin: Admin,
    pub network: Network,
    pub host: Host,
    pub disk: Disk,
    pub locale: Locale,
    pub backup: Backup,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Admin {
    pub login: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub password_hash: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Network {
    pub domain: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Host {
    pub hostname: Option<String>,
    pub profile: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Disk {
    pub device: Option<String>,
    pub encrypt: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Locale {
    pub language: Option<String>,
    pub keyboard: Option<String>,
    pub timezone: Option<String>,
}

// The backup step (admin age key on removable media) is not implemented yet.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Backup {
    pub device: Option<String>,
}

impl Answers {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Everything the install needs, checked: what the recap shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub login: String,
    pub name: String,
    pub email: Option<String>,
    pub password_hash: String,
    pub domain: String,
    pub hostname: String,
    pub profile: String,
    pub device: String,
    pub device_label: String,
    pub disko_profile: String,
    pub locale: Option<String>,

    /// XKB layout of the locale, shown in the recap.
    pub keyboard: Option<String>,
    pub timezone: Option<String>,
}

impl Plan {
    /// Every problem at once: a scripted install is fixed in a single pass.
    pub fn resolve(
        answers: &Answers,
        machine: &Machine,
        catalog: &Catalog,
    ) -> std::result::Result<Plan, Vec<String>> {
        let mut errors = Vec::new();
        let mut check = |key: &str, value: &Option<String>, pattern: &str| match value {
            None => {
                errors.push(format!("missing answer: {key}"));
                String::new()
            }
            Some(v) if !Regex::new(pattern).unwrap().is_match(v) => {
                errors.push(format!("{key}: invalid value '{v}'"));
                String::new()
            }
            Some(v) => v.clone(),
        };

        let login = check("admin.login", &answers.admin.login, RE_LOGIN);
        let name = check("admin.name", &answers.admin.name, RE_NAME);
        let password_hash = check(
            "admin.passwordHash",
            &answers.admin.password_hash,
            RE_PASSWORD_HASH,
        );
        let domain = check("network.domain", &answers.network.domain, RE_FQDN);
        let hostname = check("host.hostname", &answers.host.hostname, RE_HOSTNAME);
        let email = answers
            .admin
            .email
            .as_ref()
            .map(|_| check("admin.email", &answers.admin.email, RE_EMAIL));
        let timezone = answers
            .locale
            .timezone
            .as_ref()
            .map(|_| check("locale.timezone", &answers.locale.timezone, RE_TIMEZONE));

        match answers.version {
            None => errors.push(format!("missing answer: version ({ANSWERS_VERSION})")),
            Some(ANSWERS_VERSION) => {}
            Some(v) => errors.push(format!(
                "version {v} is not supported, expected {ANSWERS_VERSION}"
            )),
        }
        if RESERVED_LOGINS.contains(&login.as_str()) {
            errors.push(format!("admin.login: '{login}' is a system account"));
        }

        // Only what the framework knows the keyboard of (`config/locales.json`).
        // No language: the framework defaults.
        let entry = answers.locale.language.as_ref().and_then(|l| {
            let entry = catalog.locale(l);
            if entry.is_none() {
                let offered: Vec<&str> =
                    catalog.locales.iter().map(|e| e.locale.as_str()).collect();
                errors.push(format!(
                    "locale.language: '{l}' is not offered, one of {}",
                    offered.join(", ")
                ));
            }
            entry
        });
        match (&answers.locale.keyboard, entry) {
            (Some(k), Some(e)) if *k != e.xkb && *k != e.console => errors.push(format!(
                "locale.keyboard: '{k}' cannot be set apart, the keyboard follows the locale ({})",
                e.xkb
            )),
            (Some(_), None) if answers.locale.language.is_none() => errors
                .push("locale.keyboard: give locale.language, the keyboard follows it".to_string()),
            _ => {}
        }

        // The main timezone of the country, unless the answers name one.
        let timezone = timezone
            .or_else(|| entry.and_then(|e| catalog.timezones(e.country()).into_iter().next()));

        let profile = match &answers.host.profile {
            None if machine.battery => "laptop".to_string(),
            None => "desktop".to_string(),
            Some(p) if WORKSTATION_PROFILES.contains(&p.as_str()) => p.clone(),
            Some(p) => {
                errors.push(format!(
                    "host.profile: '{p}' is not a workstation profile ({})",
                    WORKSTATION_PROFILES.join(", ")
                ));
                String::new()
            }
        };

        match answers.disk.encrypt {
            None => errors.push("missing answer: disk.encrypt".to_string()),
            Some(true) => errors.push(
                "disk.encrypt: encryption is not available yet in the installer, set it to false"
                    .to_string(),
            ),
            Some(false) => {}
        }
        if machine.secure_boot {
            errors.push(
                "Secure Boot is on: DNF does not sign its boot loader. Turn it off in the firmware settings."
                    .to_string(),
            );
        }

        let disk = pick_disk(answers.disk.device.as_deref(), machine).unwrap_or_else(|e| {
            errors.push(e);
            None
        });
        let disko_profile = if machine.uefi {
            "btrfs-1-disk"
        } else {
            "ext4-1-disk-legacy"
        };

        if !errors.is_empty() {
            return Err(errors);
        }
        let disk = disk.expect("checked above");
        Ok(Plan {
            login,
            name,
            email,
            password_hash,
            domain,
            hostname,
            profile,
            device: disk.path.clone(),
            device_label: format!("{} {}", human_size(disk.size), disk.model)
                .trim()
                .to_string(),
            disko_profile: disko_profile.to_string(),
            locale: entry.map(|e| e.locale.clone()),
            keyboard: entry.map(|e| e.xkb.clone()),
            timezone,
        })
    }
}

/// The disk named by the answers, or the only candidate of the machine.
fn pick_disk<'a>(
    device: Option<&str>,
    machine: &'a Machine,
) -> std::result::Result<Option<&'a super::machine::Disk>, String> {
    let list = || {
        machine
            .disks
            .iter()
            .map(|d| format!("{} ({})", d.path, human_size(d.size)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match device {
        Some(d) if !Regex::new(RE_DEVICE).unwrap().is_match(d) => {
            Err(format!("disk.device: invalid value '{d}'"))
        }
        Some(d) => machine
            .disks
            .iter()
            .find(|c| c.path == d || c.dev == d)
            .map(Some)
            .ok_or_else(|| {
                format!("disk.device: '{d}' is not an installable disk here: {}", list())
            }),
        None => match machine.disks.as_slice() {
            [only] => Ok(Some(only)),
            [] => Err("no disk to install on (disks in use, the installer medium among them, are left out)".to_string()),
            _ => Err(format!("missing answer: disk.device, one of {}", list())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::catalog::tests::catalog;
    use super::super::machine::Disk as MachineDisk;
    use super::*;

    const CONTRACT: &str = r#"
version: 1
admin:
  login: "alice"
  name: "Alice Martin"
  email: "alice@example.org"
  passwordHash: "$y$j9T$abcdefghijklmnop$ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijk01"
network:
  domain: "example.org"
host:
  hostname: "poste"
  profile: "laptop"
disk:
  device: "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001"
  encrypt: false
locale:
  language: "fr_FR.UTF-8"
  keyboard: "fr"
  timezone: "Europe/Paris"
backup:
  device: "/dev/disk/by-label/SECOURS"
"#;

    fn disk(name: &str, path: &str) -> MachineDisk {
        MachineDisk {
            dev: format!("/dev/{name}"),
            path: path.to_string(),
            size: 512_110_190_592,
            model: "EXAMPLE SSD".to_string(),
        }
    }

    fn machine() -> Machine {
        Machine {
            uefi: true,
            secure_boot: false,
            battery: false,
            disks: vec![disk("nvme0n1", "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001")],
        }
    }

    fn answers(yaml: &str) -> Answers {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn errors(yaml: &str, machine: &Machine) -> Vec<String> {
        Plan::resolve(&answers(yaml), machine, &catalog()).unwrap_err()
    }

    #[test]
    fn contract_example_resolves() {
        let plan = Plan::resolve(&answers(CONTRACT), &machine(), &catalog()).unwrap();
        assert_eq!(plan.login, "alice");
        assert_eq!(plan.email.as_deref(), Some("alice@example.org"));
        assert_eq!(plan.profile, "laptop");
        assert_eq!(plan.device, "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001");
        assert_eq!(plan.device_label, "512 GB EXAMPLE SSD");
        assert_eq!(plan.disko_profile, "btrfs-1-disk");
        assert_eq!(plan.locale.as_deref(), Some("fr_FR.UTF-8"));
    }

    #[test]
    fn detection_fills_profile_disk_and_layout() {
        let yaml = CONTRACT
            .replace("  profile: \"laptop\"\n", "")
            .replace("  device: \"/dev/disk/by-id/nvme-EXAMPLE_SSD_0001\"\n", "");
        let mut m = machine();
        m.battery = true;
        m.uefi = false;
        let plan = Plan::resolve(&answers(&yaml), &m, &catalog()).unwrap();
        assert_eq!(plan.profile, "laptop");
        assert_eq!(plan.device, "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001");
        assert_eq!(plan.disko_profile, "ext4-1-disk-legacy");
    }

    #[test]
    fn kernel_name_selects_the_stable_path() {
        let yaml = CONTRACT.replace("/dev/disk/by-id/nvme-EXAMPLE_SSD_0001", "/dev/nvme0n1");
        let plan = Plan::resolve(&answers(&yaml), &machine(), &catalog()).unwrap();
        assert_eq!(plan.device, "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001");
    }

    #[test]
    fn every_missing_answer_is_reported_at_once() {
        let errs = errors("{}", &machine());
        for key in [
            "version",
            "admin.login",
            "admin.name",
            "admin.passwordHash",
            "network.domain",
            "host.hostname",
            "disk.encrypt",
        ] {
            assert!(
                errs.iter()
                    .any(|e| e.starts_with(&format!("missing answer: {key}"))),
                "{key} not reported in {errs:?}"
            );
        }
    }

    #[test]
    fn invalid_values_are_refused() {
        let yaml = CONTRACT
            .replace("\"alice\"", "\"nix\"")
            .replace("$y$j9T$", "clear text")
            .replace("example.org\"\nhost", "example\"\nhost")
            .replace("\"fr\"", "\"us\"")
            .replace("encrypt: false", "encrypt: true")
            .replace("profile: \"laptop\"", "profile: \"server\"");
        let errs = errors(&yaml, &machine());
        for fragment in [
            "'nix' is a system account",
            "admin.passwordHash: invalid",
            "network.domain: invalid",
            "locale.keyboard",
            "encryption is not available yet",
            "not a workstation profile",
        ] {
            assert!(
                errs.iter().any(|e| e.contains(fragment)),
                "{fragment} not reported in {errs:?}"
            );
        }
    }

    #[test]
    fn disk_choice_needs_an_installable_disk() {
        let mut m = machine();
        m.disks.push(disk("sda", "/dev/disk/by-id/ata-OTHER"));
        let yaml = CONTRACT.replace("  device: \"/dev/disk/by-id/nvme-EXAMPLE_SSD_0001\"\n", "");
        assert!(errors(&yaml, &m)
            .iter()
            .any(|e| e.starts_with("missing answer: disk.device, one of")));

        let yaml = CONTRACT.replace("nvme-EXAMPLE_SSD_0001", "usb-INSTALLER");
        assert!(errors(&yaml, &machine())
            .iter()
            .any(|e| e.contains("not an installable disk")));
    }

    #[test]
    fn locale_brings_keyboard_and_main_timezone() {
        let yaml = CONTRACT
            .replace("fr_FR.UTF-8", "es_ES.UTF-8")
            .replace("keyboard: \"fr\"", "keyboard: \"es\"")
            .replace("  timezone: \"Europe/Paris\"\n", "");
        let plan = Plan::resolve(&answers(&yaml), &machine(), &catalog()).unwrap();
        assert_eq!(plan.keyboard.as_deref(), Some("es"));
        assert_eq!(plan.timezone.as_deref(), Some("Europe/Madrid"));

        // The console name of the keyboard is accepted too: `uk` is `gb`.
        let yaml = CONTRACT
            .replace("fr_FR.UTF-8", "en_GB.UTF-8")
            .replace("keyboard: \"fr\"", "keyboard: \"uk\"");
        let plan = Plan::resolve(&answers(&yaml), &machine(), &catalog()).unwrap();
        assert_eq!(plan.keyboard.as_deref(), Some("gb"));
    }

    #[test]
    fn locale_outside_the_framework_table_is_refused() {
        let yaml = CONTRACT.replace("fr_FR.UTF-8", "ru_RU.UTF-8");
        assert!(errors(&yaml, &machine())
            .iter()
            .any(|e| e.contains("'ru_RU.UTF-8' is not offered")));
    }

    #[test]
    fn secure_boot_blocks() {
        let mut m = machine();
        m.secure_boot = true;
        assert!(errors(CONTRACT, &m)
            .iter()
            .any(|e| e.contains("Secure Boot")));
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(serde_yaml::from_str::<Answers>("admin:\n  passwd: x\n").is_err());
    }
}
