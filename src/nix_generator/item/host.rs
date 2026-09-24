use std::collections::HashMap;
use std::path::Path;

use indexmap::IndexMap;

use crate::error::{NixError, Result};
use crate::nix_generator::validation::{
    assert_arch, assert_regex, RE_DEVICE, RE_IDENTIFIER, RE_LOGIN,
};

#[derive(Debug, Default)]
pub struct ServiceParams {
    pub title: Option<String>,
    pub description: Option<String>,
    pub domain: Option<String>,
    pub icon: Option<String>,
    pub global: bool,
}

#[derive(Debug)]
pub struct Host {
    pub hostname: String,
    pub name: String,
    pub zone: String,
    pub profile: String,
    pub ip: Option<String>,
    pub vpn_ip: Option<String>,
    /// Comma-separated MAC list of the LAN interfaces, when known. Not emitted
    /// in `hosts.nix`: it drives the DHCP reservations built per zone.
    pub mac: Option<String>,
    pub arch: Option<String>,
    pub zone_domain: String,
    pub network_domain: String,
    pub users: Vec<String>,
    pub groups: Vec<String>,
    pub features: IndexMap<String, String>,
    pub tags: Vec<String>,
    pub services: IndexMap<String, ServiceParams>,
    pub disko: DiskoConfig,
}

#[derive(Debug, Default)]
pub struct DiskoConfig {
    pub profile: Option<String>,
    pub devices: HashMap<String, String>,
}

impl Host {
    pub fn new(hostname: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
            name: String::new(),
            zone: String::new(),
            profile: String::new(),
            ip: None,
            vpn_ip: None,
            mac: None,
            arch: None,
            zone_domain: String::new(),
            network_domain: String::new(),
            users: vec![],
            groups: vec![],
            features: IndexMap::new(),
            tags: vec![],
            services: IndexMap::new(),
            disko: DiskoConfig::default(),
        }
    }

    /// Validate the optional `arch` against the whitelist, then store it.
    pub fn set_arch(&mut self, arch: Option<String>) -> Result<()> {
        if let Some(value) = &arch {
            assert_arch(value)?;
        }
        self.arch = arch;
        Ok(())
    }

    pub fn set_users(&mut self, users: Vec<String>) -> Result<()> {
        for login in &users {
            assert_regex(RE_LOGIN, login, &format!("Bad login '{login}'"))?;
        }
        self.users = users;
        Ok(())
    }

    pub fn set_features(&mut self, features: &[String]) {
        self.features = IndexMap::new();
        for feature in features {
            let mut parts = feature.splitn(2, ':');
            let key = parts.next().unwrap_or("").to_string();
            let value = parts.next().unwrap_or(&self.zone).to_string();
            self.features.insert(key, value);
        }
    }

    /// Resolve and validate disko config. Sets `disko.profile` to the relative path.
    /// A consumer profile (`usr/hosts/disko/`) shadows the framework one of the same name.
    pub fn set_disko(
        &mut self,
        profile_name: Option<&str>,
        devices: HashMap<String, String>,
        project_root: &Path,
    ) -> Result<()> {
        let Some(profile) = profile_name else {
            return Ok(());
        };
        assert_regex(RE_IDENTIFIER, profile, "Bad disko profile name")?;

        let usr_path = format!("usr/hosts/disko/{profile}.nix");
        let dnf_path = format!("dnf/hosts/disko/{profile}.nix");

        let resolved = if project_root.join(&usr_path).exists() {
            usr_path
        } else if project_root.join(&dnf_path).exists() {
            dnf_path
        } else {
            return Err(NixError::validation(format!(
                "Unknown disko profile \"{profile}.nix\" (not in dnf/hosts/disko or usr/hosts/disko)"
            )));
        };

        for (name, device) in &devices {
            assert_regex(RE_IDENTIFIER, name, "bad disko device identifier")?;
            assert_regex(RE_DEVICE, device, "bad disko device path")?;
        }

        self.disko = DiskoConfig {
            profile: Some(resolved),
            devices,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn set_users_valid() {
        let mut host = Host::new("myhost");
        host.zone = "lab".to_string();
        assert!(host
            .set_users(vec!["alice".to_string(), "bob".to_string()])
            .is_ok());
        assert_eq!(host.users.len(), 2);
    }

    #[test]
    fn set_users_invalid_login() {
        let mut host = Host::new("myhost");
        assert!(host.set_users(vec!["123invalid".to_string()]).is_err());
    }

    #[test]
    fn set_features_with_zone() {
        let mut host = Host::new("myhost");
        host.zone = "lab".to_string();
        host.set_features(&["vpn".to_string(), "dns:prod".to_string()]);
        assert_eq!(host.features.get("vpn").map(String::as_str), Some("lab"));
        assert_eq!(host.features.get("dns").map(String::as_str), Some("prod"));
    }

    #[test]
    fn set_disko_dnf_profile() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("dnf/hosts/disko")).unwrap();
        fs::write(dir.path().join("dnf/hosts/disko/nvme.nix"), "{}").unwrap();
        let mut host = Host::new("myhost");
        assert!(host
            .set_disko(Some("nvme"), HashMap::new(), dir.path())
            .is_ok());
        assert_eq!(
            host.disko.profile.as_deref(),
            Some("dnf/hosts/disko/nvme.nix")
        );
    }

    #[test]
    fn set_disko_usr_profile_shadows_dnf() {
        let dir = tempdir().unwrap();
        for side in ["dnf", "usr"] {
            fs::create_dir_all(dir.path().join(side).join("hosts/disko")).unwrap();
            fs::write(dir.path().join(side).join("hosts/disko/nvme.nix"), "{}").unwrap();
        }
        let mut host = Host::new("myhost");
        host.set_disko(Some("nvme"), HashMap::new(), dir.path())
            .unwrap();
        assert_eq!(
            host.disko.profile.as_deref(),
            Some("usr/hosts/disko/nvme.nix")
        );
    }

    #[test]
    fn set_disko_by_id_device() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("dnf/hosts/disko")).unwrap();
        fs::write(dir.path().join("dnf/hosts/disko/nvme.nix"), "{}").unwrap();
        let devices = HashMap::from([(
            "main".to_string(),
            "/dev/disk/by-id/nvme-WD_BLACK_SN850X_8000GB_25252R800194".to_string(),
        )]);
        let mut host = Host::new("myhost");
        assert!(host.set_disko(Some("nvme"), devices, dir.path()).is_ok());
    }

    #[test]
    fn set_disko_profile_not_found() {
        let dir = tempdir().unwrap();
        let mut host = Host::new("myhost");
        assert!(host
            .set_disko(Some("ghost"), HashMap::new(), dir.path())
            .is_err());
    }

    #[test]
    fn set_disko_none_profile_is_ok() {
        let dir = tempdir().unwrap();
        let mut host = Host::new("myhost");
        assert!(host.set_disko(None, HashMap::new(), dir.path()).is_ok());
    }
}
