//! What `darkone new` detects instead of asking: firmware, battery, disks,
//! and the USB sticks the backup of the admin keys can go on.

use std::collections::BTreeSet;
use std::fs;
use std::net::Ipv4Addr;
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

/// An IPv4 network the machine is connected to: `192.168.1.0/24`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ipv4Net {
    /// Address of the interface, host bits included.
    pub addr: Ipv4Addr,
    pub prefix_len: u8,
}

impl Ipv4Net {
    /// The network and the `base/bits` one share an address: only the shorter
    /// prefix is compared, `10.0.0.0/8` covers every `10.x/16`.
    pub fn overlaps(&self, base: Ipv4Addr, bits: u8) -> bool {
        let len = u32::from(self.prefix_len.min(bits).min(32));
        let mask = u32::MAX.checked_shl(32 - len).unwrap_or(0);
        u32::from(self.addr) & mask == u32::from(base) & mask
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Machine {
    pub uefi: bool,
    pub secure_boot: bool,
    pub battery: bool,

    /// Install candidates only: the installer medium is left out.
    pub disks: Vec<Disk>,

    /// IPv4 networks of the interfaces, loopback included: the prefix of the
    /// new zone must overlap none of them.
    pub networks: Vec<Ipv4Net>,

    /// MACs of the physical network cards, wired first: the fixed address a
    /// gateway hands out later is reserved on them.
    pub macs: Vec<String>,
}

impl Machine {
    /// `reclaimed`: the target root of a previous install of this session,
    /// whose disk counts as free to be resumed or erased again.
    pub fn detect(reclaimed: Option<&str>) -> Self {
        Self {
            uefi: Path::new("/sys/firmware/efi").exists(),
            secure_boot: fs::read(SECURE_BOOT_VAR).is_ok_and(|v| v.get(4) == Some(&1)),
            battery: has_battery(),
            disks: candidate_disks(reclaimed),
            networks: connected_networks(),
            macs: physical_macs(&read_nics()),
        }
    }
}

/// What `/sys/class/net/<if>` says of an interface.
#[derive(Debug, Clone, PartialEq)]
struct Nic {
    name: String,

    /// Backed by hardware: lo, bridges, veth, tun and bonds have no `device`.
    physical: bool,
    wireless: bool,
    mac: String,
}

fn read_nics() -> Vec<Nic> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| {
            let sys = e.path();
            Nic {
                name: e.file_name().to_string_lossy().into_owned(),
                physical: sys.join("device").exists(),
                wireless: sys.join("wireless").exists() || sys.join("phy80211").exists(),
                mac: fs::read_to_string(sys.join("address")).unwrap_or_default(),
            }
        })
        .collect()
}

/// Wired before wireless, then by interface name: the first MAC stays the same
/// from one run to the next. Lowercase, as `config.yaml` requires; an
/// unreadable, all-zero or repeated address is dropped.
fn physical_macs(nics: &[Nic]) -> Vec<String> {
    let mut nics: Vec<&Nic> = nics.iter().filter(|n| n.physical).collect();
    nics.sort_by(|a, b| (a.wireless, &a.name).cmp(&(b.wireless, &b.name)));
    let mut macs: Vec<String> = Vec::new();
    for nic in nics {
        let mac = nic.mac.trim().to_lowercase();
        let octets: Vec<&str> = mac.split(':').collect();
        let valid = octets.len() == 6
            && octets
                .iter()
                .all(|o| o.len() == 2 && o.bytes().all(|b| b.is_ascii_hexdigit()));
        if valid && octets.iter().any(|o| *o != "00") && !macs.contains(&mac) {
            macs.push(mac);
        }
    }
    macs
}

/// No `ip`, or an unreadable answer: no network known, the zone gets `10.0`.
fn connected_networks() -> Vec<Ipv4Net> {
    match Command::new("ip").args(["-j", "-4", "addr"]).output() {
        Ok(o) if o.status.success() => parse_networks(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// `ip -j -4 addr`: one entry per interface, each with its `addr_info` list.
fn parse_networks(json: &str) -> Vec<Ipv4Net> {
    let Ok(links) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    links
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|link| link["addr_info"].as_array().into_iter().flatten())
        .filter_map(|info| {
            Some(Ipv4Net {
                addr: info["local"].as_str()?.parse().ok()?,
                prefix_len: u8::try_from(info["prefixlen"].as_u64()?)
                    .ok()
                    .filter(|l| *l <= 32)?,
            })
        })
        .collect()
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

fn candidate_disks(reclaimed: Option<&str>) -> Vec<Disk> {
    let busy = busy_disks(reclaimed);
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
fn busy_disks(reclaimed: Option<&str>) -> BTreeSet<String> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
    busy_sources(&mounts, &swaps, reclaimed)
        .into_iter()
        .filter_map(|src| fs::canonicalize(src).ok())
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter_map(|part| parent_disk(&part))
        .collect()
}

/// Devices of `/proc/mounts` and `/proc/swaps`, except the mounts on the
/// `reclaimed` root or below it. A swap is never reclaimed.
fn busy_sources<'a>(mounts: &'a str, swaps: &'a str, reclaimed: Option<&str>) -> Vec<&'a str> {
    let kept = |l: &&str| {
        let target = l.split_whitespace().nth(1).unwrap_or_default();
        !reclaimed.is_some_and(|root| under(target, root))
    };
    mounts
        .lines()
        .filter(kept)
        .chain(swaps.lines())
        .filter_map(|l| l.split_whitespace().next())
        .filter(|src| src.starts_with("/dev/"))
        .collect()
}

/// `/mnt` and `/mnt/boot` are under `/mnt`, `/mntx` is not.
fn under(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `root` is a mount point (`/proc/mounts`), as `mountpoint -q` tells.
pub fn mounted(root: &str) -> bool {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    is_mount_point(&mounts, root)
}

fn is_mount_point(mounts: &str, root: &str) -> bool {
    mounts
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some(root))
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
            parse_backup_targets(&String::from_utf8_lossy(&o.stdout), &busy_disks(None))
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
    fn networks_are_read_from_ip_json() {
        let json = r#"[
          {"ifindex": 1, "ifname": "lo", "flags": ["LOOPBACK", "UP"],
           "addr_info": [{"family": "inet", "local": "127.0.0.1", "prefixlen": 8,
             "scope": "host", "label": "lo"}]},
          {"ifindex": 2, "ifname": "enp1s0", "flags": ["BROADCAST", "UP"],
           "addr_info": [
             {"family": "inet", "local": "10.0.2.15", "prefixlen": 24,
              "broadcast": "10.0.2.255", "scope": "global", "dynamic": true},
             {"family": "inet", "local": "192.168.1.20", "prefixlen": 24,
              "scope": "global"}]},
          {"ifindex": 3, "ifname": "wlp2s0", "flags": ["BROADCAST"]}
        ]"#;
        let net = |a: &str, prefix_len| Ipv4Net {
            addr: a.parse().unwrap(),
            prefix_len,
        };
        assert_eq!(
            parse_networks(json),
            vec![
                net("127.0.0.1", 8),
                net("10.0.2.15", 24),
                net("192.168.1.20", 24)
            ]
        );
        assert!(parse_networks("not json").is_empty());
        assert!(parse_networks("[]").is_empty());
    }

    #[test]
    fn macs_are_wired_first_then_wireless_by_name() {
        let nic = |name: &str, physical, wireless, mac: &str| Nic {
            name: name.to_string(),
            physical,
            wireless,
            mac: format!("{mac}\n"),
        };
        let nics = [
            nic("wlp3s0", true, true, "AA:AA:AA:00:00:03"),
            nic("lo", false, false, "00:00:00:00:00:00"),
            nic("enp5s0", true, false, "aa:aa:aa:00:00:02"),
            nic("br0", false, false, "aa:aa:aa:00:00:09"),
            nic("veth1", false, false, "aa:aa:aa:00:00:08"),
            nic("enp1s0", true, false, "aa:aa:aa:00:00:01"),
            nic("wlan0", true, true, "aa:aa:aa:00:00:04"),
        ];
        assert_eq!(
            physical_macs(&nics),
            [
                "aa:aa:aa:00:00:01",
                "aa:aa:aa:00:00:02",
                "aa:aa:aa:00:00:04",
                "aa:aa:aa:00:00:03"
            ]
        );
    }

    #[test]
    fn macs_skip_zero_malformed_and_repeated_addresses() {
        let nic = |name: &str, mac: &str| Nic {
            name: name.to_string(),
            physical: true,
            wireless: false,
            mac: mac.to_string(),
        };
        let nics = [
            nic("eth0", "00:00:00:00:00:00"),
            nic("eth1", "aa:bb:cc:dd:ee"),
            nic("eth2", ""),
            nic("eth3", "zz:bb:cc:dd:ee:ff"),
            nic("eth4", "AA:BB:CC:DD:EE:FF"),
            nic("eth5", "aa:bb:cc:dd:ee:ff"),
        ];
        assert_eq!(physical_macs(&nics), ["aa:bb:cc:dd:ee:ff"]);
        assert!(physical_macs(&[]).is_empty());
    }

    #[test]
    fn overlap_compares_the_shorter_prefix() {
        let net = |a: &str, prefix_len| Ipv4Net {
            addr: a.parse().unwrap(),
            prefix_len,
        };
        let base = |a: &str| a.parse().unwrap();
        assert!(net("10.0.2.15", 24).overlaps(base("10.0.0.0"), 16));
        assert!(!net("10.0.2.15", 24).overlaps(base("10.1.0.0"), 16));
        assert!(net("10.7.7.7", 8).overlaps(base("10.200.0.0"), 16));
        assert!(net("172.20.0.1", 12).overlaps(base("172.16.0.0"), 16));
        assert!(net("1.2.3.4", 0).overlaps(base("10.0.0.0"), 16));
        assert!(net("192.168.1.1", 32).overlaps(base("192.168.0.0"), 16));
    }

    const MOUNTS: &str = "/dev/sr0 /iso iso9660 ro 0 0\n\
                          /dev/mapper/crypted /mnt btrfs rw 0 0\n\
                          /dev/nvme0n1p1 /mnt/boot vfat rw 0 0\n\
                          /dev/sdb1 /mntx ext4 rw 0 0\n\
                          tmpfs /tmp tmpfs rw 0 0\n";
    const SWAPS: &str = "Filename Type Size Used Priority\n/dev/sdc2 partition 8 0 -2\n";

    #[test]
    fn a_reclaimed_root_frees_its_disk_only() {
        let all = [
            "/dev/sr0",
            "/dev/mapper/crypted",
            "/dev/nvme0n1p1",
            "/dev/sdb1",
            "/dev/sdc2",
        ];
        assert_eq!(busy_sources(MOUNTS, SWAPS, None), all);
        assert_eq!(
            busy_sources(MOUNTS, SWAPS, Some("/mnt")),
            ["/dev/sr0", "/dev/sdb1", "/dev/sdc2"]
        );
    }

    #[test]
    fn mount_point_is_an_exact_target() {
        assert!(is_mount_point(MOUNTS, "/mnt"));
        assert!(is_mount_point(MOUNTS, "/mnt/boot"));
        assert!(!is_mount_point(MOUNTS, "/mn"));
        assert!(!is_mount_point("", "/mnt"));
    }

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
