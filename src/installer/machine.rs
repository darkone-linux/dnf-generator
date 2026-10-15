//! What `darkone new` detects instead of asking: firmware, battery, disks.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// UEFI variable `SecureBoot`: 4 bytes of attributes, then the state.
const SECURE_BOOT_VAR: &str =
    "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// A disk the system can be installed on.
#[derive(Debug, Clone, PartialEq)]
pub struct Disk {
    /// Kernel name, `/dev/sda`.
    pub dev: String,

    /// Stable name (`/dev/disk/by-id/…`) when the disk has one, else `dev`:
    /// `sda` may become `sdb` once a USB key is plugged in.
    pub path: String,
    pub size: u64,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub uefi: bool,
    pub secure_boot: bool,
    pub battery: bool,

    /// Install candidates only: the installer medium is left out.
    pub disks: Vec<Disk>,
}

impl Machine {
    pub fn detect() -> Self {
        Self {
            uefi: Path::new("/sys/firmware/efi").exists(),
            secure_boot: fs::read(SECURE_BOOT_VAR).is_ok_and(|v| v.get(4) == Some(&1)),
            battery: has_battery(),
            disks: candidate_disks(),
        }
    }
}

/// HID peripherals (wireless mouse, keyboard) report a battery too, with the
/// `Device` scope: only the system battery makes a laptop.
fn has_battery() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    entries.flatten().any(|e| {
        let read = |f: &str| fs::read_to_string(e.path().join(f)).unwrap_or_default();
        read("type").trim() == "Battery" && read("scope").trim() != "Device"
    })
}

fn candidate_disks() -> Vec<Disk> {
    let busy = busy_disks();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return Vec::new();
    };
    let mut disks: Vec<Disk> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let sys = e.path();
            let read = |f: &str| fs::read_to_string(sys.join(f)).unwrap_or_default();
            let virtual_dev = ["loop", "ram", "zram", "sr", "dm-", "md", "nbd", "fd"]
                .iter()
                .any(|p| name.starts_with(p));
            let size = read("size").trim().parse::<u64>().unwrap_or(0) * 512;
            if virtual_dev || size == 0 || read("ro").trim() == "1" || busy.contains(&name) {
                return None;
            }
            let dev = format!("/dev/{name}");
            Some(Disk {
                path: stable_path(&dev).unwrap_or_else(|| dev.clone()),
                dev,
                size,
                model: read("device/model").trim().to_string(),
            })
        })
        .collect();
    disks.sort_by(|a, b| a.dev.cmp(&b.dev));
    disks
}

/// Disks holding a mounted filesystem or an active swap: the installer medium
/// (`/iso`), or a disk the user is working with.
fn busy_disks() -> BTreeSet<String> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    mounts
        .lines()
        .chain(swaps.lines())
        .filter_map(|l| l.split_whitespace().next())
        .filter(|src| src.starts_with("/dev/"))
        .filter_map(|src| fs::canonicalize(src).ok())
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter_map(|part| parent_disk(&part))
        .collect()
}

/// `sdb1` → `sdb`, `nvme0n1p2` → `nvme0n1`, `sdb` → `sdb`.
fn parent_disk(name: &str) -> Option<String> {
    if Path::new("/sys/block").join(name).exists() {
        return Some(name.to_string());
    }
    let link = fs::canonicalize(Path::new("/sys/class/block").join(name)).ok()?;
    let parent = link.parent()?.file_name()?.to_string_lossy().into_owned();
    Path::new("/sys/block")
        .join(&parent)
        .exists()
        .then_some(parent)
}

/// First `/dev/disk/by-id` name of the whole disk, in name order. `wwn-` and
/// `nvme-eui.` names come last: they say nothing to a human reading the recap.
fn stable_path(dev: &str) -> Option<String> {
    let target = fs::canonicalize(dev).ok()?;
    let mut names: Vec<PathBuf> = fs::read_dir("/dev/disk/by-id")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| fs::canonicalize(p).is_ok_and(|t| t == target))
        .collect();
    names.sort_by_key(|p| {
        let n = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        (n.starts_with("wwn-") || n.starts_with("nvme-eui."), n)
    });
    names.first().map(|p| p.to_string_lossy().into_owned())
}

/// `512110190592` → `512 GB`: the size a disk is sold with.
pub fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else {
        format!("{gb:.0} GB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_rounds_like_a_label() {
        assert_eq!(human_size(512_110_190_592), "512 GB");
        assert_eq!(human_size(2_000_398_934_016), "2.0 TB");
        assert_eq!(human_size(34_359_738_368), "34 GB");
    }
}
