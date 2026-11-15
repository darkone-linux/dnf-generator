//! Answers of `darkone add gateway` (`--answers FILE`). A key the file gives
//! skips its question; what the probe tells (route card, single disk) is
//! detected, not asked.
//!
//! The pairing code is never an answer: `just _pair` reads it on `/dev/tty`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Result;
use crate::installer::answers::load_yaml;

/// Format of the answers file, migrated by `darkone upgrade` like `config.yaml`.
pub const GATEWAY_ANSWERS_VERSION: u32 = 1;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayAnswers {
    pub version: Option<u32>,

    /// IPv4 address the machine shows on its screen (installer image).
    pub address: Option<String>,
    pub host: Host,
    pub nics: Nics,
    pub disk: Disk,

    /// The recap is accepted: config written, disk erased. Scripted runs need
    /// it, `--dry-run` aside.
    pub confirm: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Host {
    /// Default `gw`.
    pub hostname: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Nics {
    /// Uplink card, to the box. Default: the card of the default route.
    pub wan: Option<String>,

    /// Cards of the zone, bridged. No default: the proposal is confirmed.
    pub lan: Option<Vec<String>>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Disk {
    /// `path`, `by_id` or kernel name of a probed disk. Optional when only one
    /// disk is large enough.
    pub device: Option<String>,
}

impl GatewayAnswers {
    pub fn load(path: &Path) -> Result<Self> {
        load_yaml(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented example (spec, notes 2b.5): every key.
    pub const EXAMPLE: &str = r#"
version: 1
address: "192.168.1.50"
host:
  hostname: "gw"
nics:
  wan: "enp1s0"
  lan: ["enp2s0", "enp3s0"]
disk:
  device: "/dev/disk/by-id/ata-EXAMPLE_SSD_0001"
confirm: true
"#;

    #[test]
    fn example_parses() {
        let a: GatewayAnswers = serde_yaml::from_str(EXAMPLE).unwrap();
        assert_eq!(a.version, Some(1));
        assert_eq!(a.address.as_deref(), Some("192.168.1.50"));
        assert_eq!(a.host.hostname.as_deref(), Some("gw"));
        assert_eq!(a.nics.wan.as_deref(), Some("enp1s0"));
        assert_eq!(
            a.nics.lan,
            Some(vec!["enp2s0".to_string(), "enp3s0".to_string()])
        );
        assert_eq!(
            a.disk.device.as_deref(),
            Some("/dev/disk/by-id/ata-EXAMPLE_SSD_0001")
        );
        assert_eq!(a.confirm, Some(true));
    }

    #[test]
    fn every_key_is_optional() {
        let a: GatewayAnswers = serde_yaml::from_str("{}").unwrap();
        assert_eq!(a, GatewayAnswers::default());
        let a: GatewayAnswers = serde_yaml::from_str("address: 10.5.0.3\n").unwrap();
        assert_eq!(a.address.as_deref(), Some("10.5.0.3"));
    }

    #[test]
    fn unknown_keys_and_a_pairing_code_are_refused() {
        assert!(serde_yaml::from_str::<GatewayAnswers>("code: 123456-7890\n").is_err());
        assert!(serde_yaml::from_str::<GatewayAnswers>("nics:\n  wlan: wlp1s0\n").is_err());
        assert!(serde_yaml::from_str::<GatewayAnswers>("disk:\n  encrypt: true\n").is_err());
    }

    #[test]
    fn loaded_from_a_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("answers.yaml");
        std::fs::write(&path, EXAMPLE).unwrap();
        assert_eq!(GatewayAnswers::load(&path).unwrap().confirm, Some(true));
        assert!(GatewayAnswers::load(&dir.path().join("none"))
            .unwrap_err()
            .contains("cannot read"));
    }
}
