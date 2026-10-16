//! What `darkone new` detects instead of asking: firmware, battery, disks,
//! and the USB sticks the backup of the admin keys can go on.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

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

/// An admin host takes about 15 GiB: a smaller disk is a USB stick, maybe the
/// one of the backup, never an install target.
const MIN_INSTALL_SIZE: u64 = 20_000_000_000;

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
            if virtual_dev
                || size < MIN_INSTALL_SIZE
                || read("ro").trim() == "1"
                || busy.contains(&name)
            {
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

/// A filesystem of a USB stick or a memory card, the backup goes on it.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupTarget {
    /// `/dev/disk/by-uuid/…` when the filesystem has one: the same name once
    /// the stick is plugged back in.
    pub path: String,
    pub label: String,
    pub fstype: String,
    pub size: u64,
    pub model: String,
}

/// Filesystems the kernel of the installer writes, none of them formatted.
const BACKUP_FSTYPES: [&str; 8] = [
    "vfat", "exfat", "ntfs", "ext4", "ext3", "ext2", "btrfs", "xfs",
];

/// Removable media only: the installed disk and the installer medium are
/// mounted, so left out.
pub fn backup_targets() -> Vec<BackupTarget> {
    let out = Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "NAME,PATH,TYPE,FSTYPE,LABEL,UUID,SIZE,RM,TRAN,MODEL",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            parse_backup_targets(&String::from_utf8_lossy(&o.stdout), &busy_disks())
        }
        _ => Vec::new(),
    }
}

fn parse_backup_targets(json: &str, busy: &BTreeSet<String>) -> Vec<BackupTarget> {
    let Ok(tree) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().trim().to_string();

    // `rm` and `size` are strings in older lsblk releases.
    let removable = |d: &Value| {
        d["rm"].as_bool().unwrap_or(false)
            || d["rm"].as_str() == Some("1")
            || ["usb", "mmc"].contains(&text(d, "tran").as_str())
    };
    let size = |v: &Value| {
        v["size"]
            .as_u64()
            .or_else(|| v["size"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0)
    };
    let mut targets = Vec::new();
    for disk in tree["blockdevices"].as_array().into_iter().flatten() {
        if text(disk, "type") != "disk" || !removable(disk) || busy.contains(&text(disk, "name")) {
            continue;
        }

        // A stick formatted without a partition table holds its filesystem
        // on the whole disk.
        let children = disk["children"].as_array().cloned().unwrap_or_default();
        for fs in std::iter::once(disk).chain(children.iter()) {
            let fstype = text(fs, "fstype");
            if !BACKUP_FSTYPES.contains(&fstype.as_str()) {
                continue;
            }
            let uuid = text(fs, "uuid");
            targets.push(BackupTarget {
                path: if uuid.is_empty() {
                    text(fs, "path")
                } else {
                    format!("/dev/disk/by-uuid/{uuid}")
                },
                label: text(fs, "label"),
                fstype,
                size: size(fs),
                model: text(disk, "model"),
            });
        }
    }
    targets
}

/// Whole disk holding a device: `/dev/disk/by-uuid/…` → `sdb`.
pub fn disk_of(path: &str) -> Option<String> {
    let dev = fs::canonicalize(path).ok()?;
    parent_disk(&dev.file_name()?.to_string_lossy())
}

/// `512110190592` → `512 GB`: the size a disk is sold with.
pub fn human_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else if gb < 1.0 {
        format!("{:.0} MB", gb * 1000.0)
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
        assert_eq!(human_size(66_060_288), "66 MB");
    }

    #[test]
    fn backup_targets_are_removable_filesystems() {
        let json = r#"{"blockdevices": [
          {"name": "nvme0n1", "path": "/dev/nvme0n1", "type": "disk", "fstype": null,
           "label": null, "uuid": null, "size": 512110190592, "rm": false,
           "tran": "nvme", "model": "SSD",
           "children": [{"name": "nvme0n1p1", "path": "/dev/nvme0n1p1", "type": "part",
             "fstype": "vfat", "label": null, "uuid": "AAAA-0001", "size": 1, "rm": false,
             "tran": null, "model": null}]},
          {"name": "sdb", "path": "/dev/sdb", "type": "disk", "fstype": null,
           "label": null, "uuid": null, "size": "8053063680", "rm": "1",
           "tran": "usb", "model": "Flash Disk ",
           "children": [
             {"name": "sdb1", "path": "/dev/sdb1", "type": "part", "fstype": "vfat",
              "label": "SECOURS", "uuid": "1234-ABCD", "size": "8052000000", "rm": "1",
              "tran": null, "model": null},
             {"name": "sdb2", "path": "/dev/sdb2", "type": "part", "fstype": "swap",
              "label": null, "uuid": "5678", "size": "1000", "rm": "1",
              "tran": null, "model": null}]},
          {"name": "sdc", "path": "/dev/sdc", "type": "disk", "fstype": "exfat",
           "label": "BIG", "uuid": null, "size": 64000000000, "rm": false,
           "tran": "usb", "model": "Stick"},
          {"name": "sdd", "path": "/dev/sdd", "type": "disk", "fstype": "iso9660",
           "label": "nixos", "uuid": "x", "size": 1, "rm": true, "tran": "usb",
           "model": "Installer",
           "children": [{"name": "sdd2", "path": "/dev/sdd2", "type": "part",
             "fstype": "vfat", "label": "EFIBOOT", "uuid": "BBBB-0002", "size": 1,
             "rm": true, "tran": null, "model": null}]}
        ]}"#;
        let busy = BTreeSet::from(["sdd".to_string()]);
        let targets = parse_backup_targets(json, &busy);
        assert_eq!(
            targets,
            vec![
                BackupTarget {
                    path: "/dev/disk/by-uuid/1234-ABCD".to_string(),
                    label: "SECOURS".to_string(),
                    fstype: "vfat".to_string(),
                    size: 8_052_000_000,
                    model: "Flash Disk".to_string(),
                },
                BackupTarget {
                    path: "/dev/sdc".to_string(),
                    label: "BIG".to_string(),
                    fstype: "exfat".to_string(),
                    size: 64_000_000_000,
                    model: "Stick".to_string(),
                },
            ]
        );
        assert!(parse_backup_targets("not json", &busy).is_empty());
    }
}
