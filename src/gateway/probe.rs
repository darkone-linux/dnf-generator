//! Inventory of the machine to install, as `just _probe` prints it (schema 1,
//! `dnf/just/project/install.just`): cards, disks, firmware.
//!
//! Keys are only ever added to the schema: unknown ones are ignored here, a
//! new `schema` number is refused.

use serde::Deserialize;
use serde_json::Value;

use super::Result;
use crate::installer::machine::human_size;

/// The only probe schema this `darkone` reads.
pub const PROBE_SCHEMA: u64 = 1;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Probe {
    pub schema: u64,

    /// `systemd-detect-virt --vm`: `none`, `kvm`, `oracle`…
    pub virt: String,
    pub uefi: bool,
    pub ram_bytes: u64,
    pub nics: Vec<ProbeNic>,
    pub disks: Vec<ProbeDisk>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProbeNic {
    pub name: String,
    pub mac: String,

    /// Carrier up: a cable plugged in, and something at its other end.
    pub link: bool,

    /// Global addresses, `a.b.c.d/len`.
    pub ipv4: Vec<String>,
    pub wifi: bool,

    /// Carries the default route: the WAN candidate.
    pub route: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProbeDisk {
    /// Kernel name, `sda`.
    pub name: String,

    /// `/dev/sda`.
    pub dev: String,

    /// Most readable `/dev/disk/by-id` link; `None`: no stable name.
    pub by_id: Option<String>,

    /// `by_id`, else `dev`: what `disko.devices` takes.
    pub path: String,
    pub size_bytes: u64,

    /// `""` when the kernel gives none (virtio-blk).
    pub model: String,
}

impl Probe {
    /// `schema` read first: another number may mean other keys.
    pub fn parse(json: &str) -> Result<Self> {
        let value: Value =
            serde_json::from_str(json).map_err(|e| format!("probe: not JSON ({e})"))?;
        match value.get("schema").and_then(Value::as_u64) {
            Some(PROBE_SCHEMA) => {}
            Some(n) => {
                return Err(format!(
                    "probe schema {n} is not supported, this darkone reads {PROBE_SCHEMA}: \
                     update darkone, or the framework, so that both match"
                ))
            }
            None => return Err("probe: no `schema` number".to_string()),
        }
        let mut probe: Probe = serde_json::from_value(value).map_err(|e| format!("probe: {e}"))?;
        probe.keep_printable();
        Ok(probe)
    }

    /// Read over an ssh that checks no host key, then shown on this terminal:
    /// printable ASCII only, the filter of the remote script applied again.
    fn keep_printable(&mut self) {
        let clean = |s: &mut String| s.retain(|c| (' '..='~').contains(&c));
        clean(&mut self.virt);
        for nic in &mut self.nics {
            clean(&mut nic.name);
            clean(&mut nic.mac);
            nic.ipv4.iter_mut().for_each(clean);
        }
        for disk in &mut self.disks {
            clean(&mut disk.name);
            clean(&mut disk.dev);
            clean(&mut disk.path);
            clean(&mut disk.model);
            if let Some(by_id) = &mut disk.by_id {
                clean(by_id);
            }
        }
    }

    /// Wired cards, in the probe order: the only ones a gateway bridges or
    /// routes through.
    pub fn wired(&self) -> impl Iterator<Item = &ProbeNic> {
        self.nics.iter().filter(|n| !n.wifi)
    }

    pub fn nic(&self, name: &str) -> Option<&ProbeNic> {
        self.nics.iter().find(|n| n.name == name)
    }
}

impl ProbeDisk {
    /// `32 GB QEMU HARDDISK`, or the size alone.
    pub fn label(&self) -> String {
        format!("{} {}", human_size(self.size_bytes), self.model)
            .trim()
            .to_string()
    }

    /// The disk named by an answer: its `path`, `by_id` or kernel name.
    pub fn is(&self, device: &str) -> bool {
        self.path == device || self.dev == device || self.by_id.as_deref() == Some(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of the spec (2b.3 notes): QEMU, one SATA and one virtio disk.
    const SPEC_EXAMPLE: &str = r#"{"schema":1,"virt":"kvm","uefi":true,"ram_bytes":3041988608,
     "nics":[{"name":"enp0s2","mac":"52:54:00:12:34:56","link":true,
              "ipv4":["10.0.2.15/24"],"wifi":false,"route":true}],
     "disks":[{"name":"sda","dev":"/dev/sda",
               "by_id":"/dev/disk/by-id/ata-My_Disk_SATA1",
               "path":"/dev/disk/by-id/ata-My_Disk_SATA1",
               "size_bytes":1073741824,"model":"My Disk"},
              {"name":"vda","dev":"/dev/vda","by_id":null,
               "path":"/dev/vda","size_bytes":2147483648,
               "model":""}]}"#;

    #[test]
    fn spec_example_parses() {
        let p = Probe::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(p.virt, "kvm");
        assert!(p.uefi);
        assert_eq!(p.nics[0].ipv4, ["10.0.2.15/24"]);
        assert!(p.nics[0].route);
        assert_eq!(p.disks[1].by_id, None);
        assert_eq!(p.disks[0].label(), "1 GB My Disk");
        assert_eq!(p.disks[1].label(), "2 GB");
        assert!(p.disks[0].is("/dev/sda"));
        assert!(p.disks[0].is("/dev/disk/by-id/ata-My_Disk_SATA1"));
        assert!(!p.disks[0].is("/dev/vda"));
    }

    #[test]
    fn schema_is_read_first() {
        let newer = SPEC_EXAMPLE.replace(r#""schema":1"#, r#""schema":2"#);
        assert!(Probe::parse(&newer)
            .unwrap_err()
            .contains("schema 2 is not supported"));
        assert!(Probe::parse(r#"{"virt":"kvm"}"#)
            .unwrap_err()
            .contains("no `schema`"));
        assert!(Probe::parse("not json").unwrap_err().contains("not JSON"));

        // Schema 1, a key missing: the probe is broken, not newer.
        assert!(Probe::parse(r#"{"schema":1}"#).is_err());
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let hostile = SPEC_EXAMPLE
            .replace("My Disk", "My\\u001b[2JDisk\\u0007é")
            .replace("enp0s2", "enp\\u001b0s2");
        let p = Probe::parse(&hostile).unwrap();
        assert_eq!(p.disks[0].model, "My[2JDisk");
        assert_eq!(p.nics[0].name, "enp0s2");
    }

    #[test]
    fn keys_added_to_schema_1_are_ignored() {
        let more = SPEC_EXAMPLE.replace(r#""uefi":true"#, r#""uefi":true,"tpm":true"#);
        assert_eq!(
            Probe::parse(&more).unwrap(),
            Probe::parse(SPEC_EXAMPLE).unwrap()
        );
    }
}
