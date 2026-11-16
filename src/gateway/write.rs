//! Step 5 of `darkone add gateway`: `etc/config.yaml` edited by `yq` (yq-go,
//! mikefarah), which keeps the comments a YAML library would drop.
//!
//! - Values reach `yq` through `strenv()` only: card names and disk paths
//!   come from the probed machine, none is ever part of the expression.
//! - Blank lines: `yq` drops them, so they travel as marker comments.
//! - The result must be the very document the same edit gives on
//!   `serde_yaml`: anything else `yq` did is refused, nothing written.

use std::io::Write as _;
use std::process::{Command, Stdio};

use regex::Regex;
use serde_yaml::{Mapping, Value};

use super::plan::{AdminMac, GatewayPlan, DISKO_DISK, GATEWAY_PROFILE};
use super::Result;
use crate::nix_generator::validation::{RE_DEVICE, RE_HOSTNAME};

/// Linux interface name (`IFNAMSIZ`), nothing a shell or YAML reads twice.
pub const RE_NIC: &str = r"^[A-Za-z0-9_.:-]{1,15}$";

/// `name:` of the gateway entry, required by the generator.
pub const GATEWAY_NAME: &str = "Gateway";

/// Comment a blank line becomes while `yq` holds the file.
const BLANK: &str = "dnf-blank-line";

/// Every `yq` major from 4 on reads these expressions (`strenv`, `style=`).
const YQ_MAJOR: u32 = 4;

pub fn valid_nic(name: &str) -> bool {
    Regex::new(RE_NIC).unwrap().is_match(name)
}

pub fn valid_device(path: &str) -> bool {
    Regex::new(RE_DEVICE).unwrap().is_match(path)
}

/// What the edit writes: strings only, each checked or of this project.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub zone: String,
    pub wan: String,
    pub lan: Vec<String>,
    pub hostname: String,

    /// `<zone>:1.1`.
    pub zone_spec: String,
    pub disko_profile: String,
    pub disk: String,
    pub admin: String,

    /// `<zone>:2.1`; `None`: its fixed address is declared, left as is.
    pub admin_zone: Option<String>,

    /// `None`: its `mac:` is declared, or unknown; left as is.
    pub admin_mac: Option<String>,
}

impl Edit {
    /// Refuses what the probed machine may not have shaped: a card name or a
    /// disk path that is more than a name.
    pub fn from_plan(plan: &GatewayPlan) -> Result<Edit> {
        let mut bad = Vec::new();
        for nic in std::iter::once(&plan.wan).chain(&plan.lan) {
            if !valid_nic(&nic.name) {
                bad.push(format!("card name {:?}", nic.name));
            }
        }
        if !valid_device(&plan.disk.path) {
            bad.push(format!("disk path {:?}", plan.disk.path));
        }
        if !Regex::new(RE_HOSTNAME).unwrap().is_match(&plan.hostname) {
            bad.push(format!("host name {:?}", plan.hostname));
        }
        if !bad.is_empty() {
            return Err(format!(
                "not written to etc/config.yaml, unexpected characters: {}",
                bad.join(", ")
            ));
        }
        Ok(Edit {
            zone: plan.zone.clone(),
            wan: plan.wan.name.clone(),
            lan: plan.lan.iter().map(|n| n.name.clone()).collect(),
            hostname: plan.hostname.clone(),
            zone_spec: plan.zone_spec.clone(),
            disko_profile: plan.disko_profile.clone(),
            disk: plan.disk.path.clone(),
            admin: plan.admin.hostname.clone(),
            admin_zone: plan.admin.new_address.then(|| plan.admin.zone_spec.clone()),
            admin_mac: match &plan.admin.mac {
                AdminMac::Add(mac) => Some(mac.clone()),
                AdminMac::Declared(_) | AdminMac::Unknown => None,
            },
        })
    }

    /// Constant text: values are named, never pasted. New nodes take the
    /// style of the installer file, double-quoted values and flow lists.
    pub fn expression(&self) -> String {
        let quoted = r#"(.. | select(tag == "!!str")) style="double""#;
        let mut expr = format!(
            r#".zones[strenv(DNF_ZONE)].gateway = ({{"wan": {{"interface": strenv(DNF_WAN)}}, "lan": {{"interfaces": (strenv(DNF_LAN) | split(",") | . style="flow")}}}} | {quoted})
| .hosts += [{{"hostname": strenv(DNF_HOST), "name": strenv(DNF_NAME), "zone": strenv(DNF_HOST_ZONE), "profile": strenv(DNF_PROFILE), "disko": {{"profile": strenv(DNF_DISKO), "devices": {{strenv(DNF_DISKO_DISK): strenv(DNF_DISK)}}}}}} | {quoted}]"#
        );
        let admin = "(.hosts[] | select(.hostname == strenv(DNF_ADMIN))";
        if self.admin_zone.is_some() {
            expr.push_str(&format!("\n| {admin} | .zone) = strenv(DNF_ADMIN_ZONE)"));
        }
        if self.admin_mac.is_some() {
            expr.push_str(&format!(
                "\n| {admin} | .mac) = (strenv(DNF_ADMIN_MAC) | . style=\"double\")"
            ));
        }
        expr
    }

    /// The variables `expression` names. `DNF_LAN`: no comma in a card name
    /// (`RE_NIC`), `split(",")` gives the cards back.
    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("DNF_ZONE", self.zone.clone()),
            ("DNF_WAN", self.wan.clone()),
            ("DNF_LAN", self.lan.join(",")),
            ("DNF_HOST", self.hostname.clone()),
            ("DNF_NAME", GATEWAY_NAME.to_string()),
            ("DNF_HOST_ZONE", self.zone_spec.clone()),
            ("DNF_PROFILE", GATEWAY_PROFILE.to_string()),
            ("DNF_DISKO", self.disko_profile.clone()),
            ("DNF_DISKO_DISK", DISKO_DISK.to_string()),
            ("DNF_DISK", self.disk.clone()),
            ("DNF_ADMIN", self.admin.clone()),
            (
                "DNF_ADMIN_ZONE",
                self.admin_zone.clone().unwrap_or_default(),
            ),
            ("DNF_ADMIN_MAC", self.admin_mac.clone().unwrap_or_default()),
        ]
    }

    /// The same edit on the parsed document: what `yq` must give, and the
    /// shapes it cannot edit, refused before it runs.
    pub fn expected(&self, original: &Value) -> Result<Value> {
        let mut doc = original.clone();
        let zone = doc
            .get_mut("zones")
            .and_then(|z| z.get_mut(self.zone.as_str()))
            .and_then(Value::as_mapping_mut)
            .ok_or_else(|| format!("etc/config.yaml: zones.{} is not a mapping", self.zone))?;
        if zone.contains_key("gateway") {
            return Err(format!(
                "etc/config.yaml: zones.{} already has a gateway block",
                self.zone
            ));
        }
        let lan: Vec<Value> = self.lan.iter().map(|n| n.as_str().into()).collect();
        zone.insert(
            "gateway".into(),
            map([
                ("wan", map([("interface", self.wan.as_str().into())])),
                ("lan", map([("interfaces", Value::Sequence(lan))])),
            ]),
        );

        let hosts = doc
            .get_mut("hosts")
            .and_then(Value::as_sequence_mut)
            .ok_or("etc/config.yaml: hosts is not a list")?;
        if self.admin_zone.is_some() || self.admin_mac.is_some() {
            let by_hand = format!(
                "give {} its `zone: {}`{} by hand in etc/config.yaml, commit, then run \
                 darkone add gateway again",
                self.admin,
                self.admin_zone.as_deref().unwrap_or("<zone>:<x.y>"),
                if self.admin_mac.is_some() {
                    " and its `mac:`"
                } else {
                    ""
                }
            );
            let mut entries: Vec<&mut Mapping> = hosts
                .iter_mut()
                .filter_map(Value::as_mapping_mut)
                .filter(|h| h.get("hostname").and_then(Value::as_str) == Some(self.admin.as_str()))
                .collect();

            // A `range:` or `hosts:` entry declares several machines at once.
            let entry = match entries.as_mut_slice() {
                [one] if !one.contains_key("range") && !one.contains_key("hosts") => one,
                _ => {
                    return Err(format!(
                        "admin host {} has no entry of its own in etc/config.yaml (range, \
                         list, or declared twice): {by_hand}",
                        self.admin
                    ))
                }
            };
            if let Some(zone) = &self.admin_zone {
                entry.insert("zone".into(), zone.as_str().into());
            }
            if let Some(mac) = &self.admin_mac {
                if entry.contains_key("mac") {
                    return Err(format!(
                        "admin host {} already has a `mac:` key: {by_hand}",
                        self.admin
                    ));
                }
                entry.insert("mac".into(), mac.as_str().into());
            }
        }
        hosts.push(map([
            ("hostname", self.hostname.as_str().into()),
            ("name", GATEWAY_NAME.into()),
            ("zone", self.zone_spec.as_str().into()),
            ("profile", GATEWAY_PROFILE.into()),
            (
                "disko",
                map([
                    ("profile", self.disko_profile.as_str().into()),
                    ("devices", map([(DISKO_DISK, self.disk.as_str().into())])),
                ]),
            ),
        ]));
        Ok(doc)
    }

    /// The edited text of `original`. Blank lines kept when the file allows
    /// it: a marker inside a block scalar breaks it, then they are dropped
    /// and the diff shows it.
    pub fn apply(&self, original: &str) -> Result<String> {
        let parsed: Value =
            serde_yaml::from_str(original).map_err(|e| format!("etc/config.yaml: {e}"))?;
        let expected = self.expected(&parsed)?;
        check_yq()?;
        let marked = !original.contains(BLANK);
        let kept = marked
            .then(|| self.run(original, &expected, true))
            .and_then(Result::ok);
        match kept {
            Some(text) => Ok(text),
            None => self.run(original, &expected, false),
        }
    }

    fn run(&self, original: &str, expected: &Value, blanks: bool) -> Result<String> {
        let input = if blanks {
            mark_blanks(original)
        } else {
            original.to_string()
        };
        let output = yq(&self.expression(), &self.env(), &input)?;
        let output = if blanks {
            space_before_entry(&unmark_blanks(&output), &self.hostname)
        } else {
            output
        };
        let got: Value = serde_yaml::from_str(&output)
            .map_err(|e| format!("yq gave a file that does not parse ({e}): nothing written"))?;
        if got != *expected {
            return Err(
                "yq did not give the expected etc/config.yaml (anchors, several documents, \
                 unusual layout?): nothing written, declare the gateway by hand"
                    .to_string(),
            );
        }
        Ok(output)
    }
}

fn map<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Mapping(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

/// Each blank line as a comment `yq` carries along.
fn mark_blanks(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.trim().is_empty() {
                format!("#{BLANK}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

/// Markers back to blank lines, wherever `yq` indented them.
fn unmark_blanks(text: &str) -> String {
    let marker = format!("#{BLANK}");
    text.lines()
        .map(|l| {
            if l.trim() == marker {
                "\n".to_string()
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

/// A blank line before the entry `yq` appended, as between two hosts. Its
/// first line is known: `- hostname: "<name>"`, the style of `expression`.
fn space_before_entry(text: &str, hostname: &str) -> String {
    let entry = format!("- hostname: \"{hostname}\"");
    let mut out = String::with_capacity(text.len() + 1);
    let mut after_blank = true;
    for line in text.lines() {
        if line.trim_start() == entry && !after_blank {
            out.push('\n');
        }
        after_blank = line.trim().is_empty();
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `yq` of mikefarah, 4 or later: the Python `yq` reads another language.
pub fn check_yq() -> Result<()> {
    let needed = "etc/config.yaml is edited by yq-go (mikefarah), which comes with the admin \
                  profile (nix-admin); else `nix shell nixpkgs#yq-go`";
    let out = Command::new("yq")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| format!("yq not found: {needed}"))?;
    let version = String::from_utf8_lossy(&out.stdout);
    match yq_major(&version) {
        Some(major) if major >= YQ_MAJOR => Ok(()),
        _ => Err(format!(
            "this yq ({}) is not yq-go {YQ_MAJOR} or later: {needed}",
            version.trim()
        )),
    }
}

/// `yq (https://github.com/mikefarah/yq/) version v4.54.1` → 4.
fn yq_major(version: &str) -> Option<u32> {
    if !version.contains("mikefarah") {
        return None;
    }
    let number = version.rsplit("version").next()?.trim();
    number
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// `yq` as a filter: file on stdin, result on stdout, values in its
/// environment only.
fn yq(expression: &str, env: &[(&str, String)], input: &str) -> Result<String> {
    let mut child = Command::new("yq")
        .args(["eval", "--no-colors", expression, "-"])
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run yq: {e}"))?;

    // Written from a thread: `yq` may fill the stdout pipe before it has
    // read everything.
    let mut stdin = child.stdin.take().expect("piped above");
    let text = input.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(text.as_bytes()));
    let out = child.wait_with_output().map_err(|e| format!("yq: {e}"))?;
    writer.join().ok();
    if out.status.success() {
        String::from_utf8(out.stdout).map_err(|_| "yq gave a file that is not UTF-8".to_string())
    } else {
        Err(format!(
            "yq failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_names_and_disk_paths_are_names_only() {
        for ok in ["enp1s0", "eth0", "wlp3s0", "lan-0", "eno1.100", "a:b"] {
            assert!(valid_nic(ok), "{ok}");
        }
        for bad in [
            "",
            "enp1s0,enp2s0",
            "a b",
            "$(id)",
            "x\"y",
            "enp1s0\n",
            "sixteen-chars-xx",
            "é",
        ] {
            assert!(!valid_nic(bad), "{bad:?}");
        }
        assert!(valid_device("/dev/disk/by-id/ata-EXAMPLE_SSD_0001"));
        assert!(valid_device("/dev/vda"));
        for bad in ["sda", "/dev/sd a", "/dev/$(id)", "/dev/x\"", "/etc/passwd"] {
            assert!(!valid_device(bad), "{bad:?}");
        }
    }

    #[test]
    fn blank_lines_travel_as_comments() {
        let text = "a: 1\n\n  \nb:\n  c: 2\n";
        let marked = mark_blanks(text);
        assert_eq!(
            marked,
            "a: 1\n#dnf-blank-line\n#dnf-blank-line\nb:\n  c: 2\n"
        );
        assert_eq!(unmark_blanks(&marked), "a: 1\n\n\nb:\n  c: 2\n");

        // As `yq` gives them back: indented like the node that follows.
        assert_eq!(
            unmark_blanks("a: 1\n  #dnf-blank-line\n# kept\n"),
            "a: 1\n\n# kept\n"
        );
    }

    #[test]
    fn appended_entry_gets_one_blank_line_before_it() {
        let glued = "hosts:\n  - hostname: \"poste\"\n    zone: \"main\"\n  - hostname: \"gw\"\n";
        let spaced =
            "hosts:\n  - hostname: \"poste\"\n    zone: \"main\"\n\n  - hostname: \"gw\"\n";
        assert_eq!(space_before_entry(glued, "gw"), spaced);
        assert_eq!(space_before_entry(spaced, "gw"), spaced);
        assert_eq!(space_before_entry(glued, "other"), glued);
    }

    #[test]
    fn only_yq_go_4_is_accepted() {
        assert_eq!(
            yq_major("yq (https://github.com/mikefarah/yq/) version v4.54.1\n"),
            Some(4)
        );
        assert_eq!(
            yq_major("yq (https://github.com/mikefarah/yq/) version 5.0.0"),
            Some(5)
        );

        // The Python wrapper around jq.
        assert_eq!(yq_major("yq 3.4.3"), None);
        assert_eq!(yq_major(""), None);
    }

    fn edit() -> Edit {
        Edit {
            zone: "main".into(),
            wan: "enp1s0".into(),
            lan: vec!["enp2s0".into()],
            hostname: "gw".into(),
            zone_spec: "main:1.1".into(),
            disko_profile: "server-1-disk".into(),
            disk: "/dev/sda".into(),
            admin: "poste".into(),
            admin_zone: Some("main:2.1".into()),
            admin_mac: Some("aa:bb:cc:00:00:01".into()),
        }
    }

    #[test]
    fn expression_names_its_values_and_never_holds_one() {
        let mut e = edit();
        e.wan = "WAN-CARD".into();
        e.disk = "/dev/DISK".into();
        e.hostname = "HOSTNAME".into();
        let full = e.expression();
        for value in ["WAN-CARD", "/dev/DISK", "HOSTNAME", "enp2s0", "poste"] {
            assert!(!full.contains(value), "{value} in {full}");
        }
        assert!(full.contains("strenv(DNF_ADMIN_ZONE)") && full.contains("strenv(DNF_ADMIN_MAC)"));

        // The admin entry is left alone when it needs nothing.
        e.admin_zone = None;
        e.admin_mac = None;
        let gateway_only = e.expression();
        assert!(!gateway_only.contains("DNF_ADMIN"), "{gateway_only}");
    }

    fn doc(yaml: &str) -> Value {
        serde_yaml::from_str(yaml).unwrap()
    }

    const BASE: &str =
        "zones:\n  main:\n    ipPrefix: \"10.1\"\nhosts:\n  - hostname: poste\n    zone: main\n";

    #[test]
    fn expected_document_holds_the_whole_edit() {
        let got = edit().expected(&doc(BASE)).unwrap();
        let want = doc("zones:\n  main:\n    ipPrefix: \"10.1\"\n    gateway:\n      wan:\n        interface: enp1s0\n      lan:\n        interfaces: [enp2s0]\n\
             hosts:\n  - hostname: poste\n    zone: main:2.1\n    mac: aa:bb:cc:00:00:01\n\
             \x20 - hostname: gw\n    name: Gateway\n    zone: main:1.1\n    profile: gateway\n    disko:\n      profile: server-1-disk\n      devices:\n        main: /dev/sda\n");
        assert_eq!(got, want);
    }

    #[test]
    fn shapes_yq_cannot_edit_are_refused_first() {
        let e = edit();
        let err = e
            .expected(&doc(&BASE.replace("ipPrefix", "gateway: {}\n    ipPrefix")))
            .unwrap_err();
        assert!(err.contains("already has a gateway block"), "{err}");
        let err = e.expected(&doc("zones: {}\nhosts: []\n")).unwrap_err();
        assert!(err.contains("zones.main is not a mapping"), "{err}");
        let err = e
            .expected(&doc("zones:\n  main: {}\nhosts: {}\n"))
            .unwrap_err();
        assert!(err.contains("hosts is not a list"), "{err}");

        // The admin host inside a list of hosts: no entry `yq` can select.
        let listed = "zones:\n  main: {}\nhosts:\n  - hostname: \"%s\"\n    zone: main\n    hosts:\n      poste: {}\n";
        let err = e.expected(&doc(listed)).unwrap_err();
        assert!(err.contains("no entry of its own"), "{err}");
        assert!(
            err.contains("`zone: main:2.1` and its `mac:` by hand"),
            "{err}"
        );

        // Nothing to change on it: the same file is fine.
        let mut gateway_only = edit();
        gateway_only.admin_zone = None;
        gateway_only.admin_mac = None;
        gateway_only.expected(&doc(listed)).unwrap();

        let err = e
            .expected(&doc(&format!("{BASE}    mac: \"aa:00\"\n")))
            .unwrap_err();
        assert!(err.contains("already has a `mac:` key"), "{err}");
    }
}
