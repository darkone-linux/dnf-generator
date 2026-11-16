//! `darkone add gateway`: the first gateway of the zone `darkone new` created,
//! installed from the admin host on a machine booted on the installer image.
//!
//! - `answers`: the answers file (`--answers`);
//! - `ask`: the questions for what the answers and the probe leave out;
//! - `plan`: proposals from the probe, the checked plan and its recap;
//! - `probe`: the inventory `just _probe` prints (schema 1);
//! - `project`: the project, its guards and what the plan reads of its config;
//! - `write`: `etc/config.yaml` edited by `yq` from the plan;
//! - `chain`: commit, `just full-install`, `just apply-local`, the checks.
//!
//! What touches the system stays in the recipes: `just _pair`, `just _probe`,
//! `just clean`, `just full-install`, `just apply-local`.

pub mod answers;
pub mod ask;
pub mod chain;
pub mod plan;
pub mod probe;
pub mod project;
pub mod write;

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use regex::Regex;

use answers::GatewayAnswers;
use plan::{propose, AdminMachine, GatewayPlan};
use probe::Probe;
use write::Edit;

use crate::installer::machine::local_macs;
use crate::installer::project::log_as;
pub use crate::installer::Result;

/// Account of the installer image, the one `_pair` puts the deploy key in.
pub const INSTALLER_USER: &str = "nixos";

pub struct Options {
    pub answers: Option<PathBuf>,
    pub non_interactive: bool,
    pub dry_run: bool,

    /// Probe read from a file instead of `just _probe`: `--dry-run` only, a
    /// plan without a machine.
    pub probe: Option<PathBuf>,
}

pub fn log(msg: &str) {
    log_as("GATEWAY", msg);
}

pub fn run(opts: Options) -> Result<()> {
    if project::on_installer() {
        return Err(
            "this is the installer image: run darkone add gateway on the admin host, \
             in its project (/etc/nixos)"
                .to_string(),
        );
    }
    let cwd = std::env::current_dir().map_err(|e| format!("working directory: {e}"))?;
    let root = project::find_root(&cwd).ok_or_else(|| {
        format!(
            "{} is not in a DNF project (no etc/config.yaml and Justfile above): cd /etc/nixos",
            cwd.display()
        )
    })?;
    let interactive = !opts.non_interactive;
    if interactive && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err(
            "no terminal to ask on: give every answer with --answers --non-interactive".to_string(),
        );
    }

    // A dry run writes nothing: a pending change is only worth a warning.
    let pending = project::dirty(&project::git_status(&root)?);
    match pending {
        Some(why) if !opts.dry_run => return Err(why),
        Some(why) => log(&format!("dry run, {why}")),
        None => {}
    }
    let network = project::load_network(&root)?;

    // Needed at step 5 only: said before the pairing, not after the recap.
    write::check_yq()?;
    let mut answers = match &opts.answers {
        Some(path) => GatewayAnswers::load(path)?,
        None => GatewayAnswers::default(),
    };
    let admin = AdminMachine {
        hostname: short_hostname()?,
        macs: local_macs(),
    };
    if interactive {
        ask::intro()?;
        ask::address(&mut answers)?;
    }

    let (probe, host_key) = match &opts.probe {
        Some(file) => {
            let json = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
            (Probe::parse(&json)?, None)
        }
        None => {
            let address = checked_address(answers.address.as_deref())?;
            let key = pair(&root, address)?;
            (Probe::parse(&probe_json(&root, address)?)?, Some(key))
        }
    };
    if interactive {
        ask::complete(&mut answers, &probe, &propose(&probe))?;
    }

    // Box / zone check inside: refused before anything is written (2a.4).
    let plan =
        GatewayPlan::resolve(&answers, &probe, &network, &admin, host_key).map_err(|errors| {
            format!(
                "the answers do not allow the gateway:\n  - {}",
                errors.join("\n  - ")
            )
        })?;
    let recap = plan.recap();
    let mode = Mode {
        interactive,
        dry_run: opts.dry_run,
        confirmed: answers.confirm == Some(true),
    };
    match (interactive, mode.dry_run || mode.confirmed) {
        (true, true) => cliclack::note("Plan", &recap).map_err(crate::installer::ask::aborted)?,
        (true, false) => ask::confirm(&recap)?,
        (false, true) => {
            log("Plan:");
            println!("    {}", recap.replace('\n', "\n    "));
        }
        (false, false) => {
            return Err(
                "--non-interactive needs `confirm: true` in the answers: the disk will be erased"
                    .to_string(),
            )
        }
    }
    apply(&root, &plan, &mode)
}

/// How this run talks and what it may change.
struct Mode {
    interactive: bool,
    dry_run: bool,

    /// `confirm: true` of the answers: no question left.
    confirmed: bool,
}

/// Steps 5 to 8 of the spec (2b.5), the plan accepted. Nothing is written
/// before the diff is accepted and the machine found again.
fn apply(root: &Path, plan: &GatewayPlan, mode: &Mode) -> Result<()> {
    let config = root.join(project::CONFIG);
    let original = fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    let edited = Edit::from_plan(plan)?.apply(&original)?;
    project::check_edit(root, &edited, plan)?;
    log("Changes to etc/config.yaml, not written yet:");
    show_diff(root, &edited)?;
    if mode.dry_run {
        log(&format!(
            "Then, on a real run:\n{}",
            chain::remaining(plan, chain::Step::Clean)
        ));
        return if mode.interactive {
            ask::dry_run_done()
        } else {
            log("Dry run: nothing changed.");
            Ok(())
        };
    }
    if mode.interactive && !mode.confirmed {
        ask::confirm_diff(&plan.hostname)?;
    }

    // The recap and the diff took their time: another machine may hold the
    // address now. `install` checks the same key again, at its own pairing.
    let host_key = plan
        .host_key
        .as_deref()
        .ok_or("no host key from the pairing: nothing changed")?;
    let address = plan.target.to_string();
    if pair(root, &address)? != host_key {
        return Err(format!(
            "{address} no longer shows the host key it was probed with: another machine, or \
             it rebooted. Nothing changed: check its screen, then run darkone add gateway again"
        ));
    }
    fs::write(&config, &edited).map_err(|e| format!("{}: {e}", config.display()))?;
    chain::run(root, plan, host_key)?;

    let done = format!(
        "Step 2 of 4 done: {} is the gateway of zone {}, this machine is {} behind it.",
        plan.hostname, plan.zone, plan.admin.ip
    );
    if mode.interactive {
        ask::done(&done, NEXT_STEP);
    } else {
        log(&done);
        log(&format!("Next: {}", NEXT_STEP.replace('\n', " ")));
    }
    Ok(())
}

/// Step 3 of the plan; its command is not released with this one.
const NEXT_STEP: &str = "Step 3: workstations, servers and services.\n\
     Plug each machine into the LAN of the gateway, then boot it on the\n\
     DNF installer. `darkone add workstation` comes with step 3; until\n\
     then: declare the host in etc/config.yaml, then run\n\
     `just full-install <host>`.";

/// `git diff` of the file against its edit, read on stdin: nothing is written
/// to show it. `--no-ext-diff`: a patch, whatever diff tool the admin set.
fn show_diff(root: &Path, edited: &str) -> Result<()> {
    use std::io::Write as _;
    let mut child = Command::new("git")
        .args(["--no-pager", "diff", "--no-index", "--no-ext-diff", "--"])
        .args([project::CONFIG, "-"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let written = child
        .stdin
        .take()
        .expect("piped above")
        .write_all(edited.as_bytes());
    let status = child.wait().map_err(|e| format!("git diff: {e}"))?;

    // 1: the files differ, as they must.
    match (written, status.code()) {
        (Ok(()), Some(0 | 1)) => Ok(()),
        _ => Err(format!(
            "`git diff` of etc/config.yaml failed ({status}): nothing changed"
        )),
    }
}

/// An IPv4 literal before it reaches a recipe argument.
fn checked_address(address: Option<&str>) -> Result<&str> {
    match address {
        None => Err("missing answer: address, the IPv4 its screen shows".to_string()),
        Some(a) if a.parse::<Ipv4Addr>().is_ok() => Ok(a),
        Some(a) => Err(format!("address: '{a}' is not an IPv4 address")),
    }
}

/// The name of this machine's `hosts` entry.
fn short_hostname() -> Result<String> {
    let name = fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|e| format!("hostname of this machine: {e}"))?;
    let name = name
        .trim()
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        Err("this machine has no hostname".to_string())
    } else {
        Ok(name)
    }
}

/// The one line `_pair` prints, `ssh-ed25519 <base64>`: all a later run
/// compares. 51 bytes of key blob: 68 base64 characters, no padding.
fn host_key_of(output: &str) -> Option<String> {
    let key = Regex::new(r"^ssh-ed25519 [A-Za-z0-9+/]{68}$").unwrap();
    let line = output.trim();
    key.is_match(line).then(|| line.to_string())
}

/// `just _pair`: the code is typed on `/dev/tty`, never an answer or an
/// argument. stdout carries the host key alone, messages go to stderr. A
/// machine already paired is asked nothing.
fn pair(root: &Path, address: &str) -> Result<String> {
    log(&format!("Pairing with {address}..."));
    let out = just_stdout(root, &["_pair", address, INSTALLER_USER], Stdio::inherit())?;
    host_key_of(&out)
        .ok_or_else(|| format!("`just _pair` printed no ed25519 host key for {address}"))
}

/// `just _probe`: one JSON object on stdout, validated by `jq` there.
fn probe_json(root: &Path, address: &str) -> Result<String> {
    log(&format!("Probing {address}..."));
    just_stdout(root, &["_probe", address, INSTALLER_USER], Stdio::null())
}

fn just_stdout(root: &Path, args: &[&str], stdin: Stdio) -> Result<String> {
    let out = Command::new("just")
        .args(args)
        .current_dir(root)
        .stdin(stdin)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cannot run just: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("`just {}` failed ({})", args[0], out.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_is_one_ed25519_key_and_nothing_else() {
        let key = format!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI{}", "x".repeat(43));
        let key = key.as_str();
        assert_eq!(host_key_of(&format!("{key}\n")).as_deref(), Some(key));
        for bad in [
            "",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7",
            "ssh-ed25519 AAAA",
            &format!("{key} $(id)"),
            &format!("{key}\n{key}"),
            &format!("warning\n{key}"),
        ] {
            assert_eq!(host_key_of(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_an_ipv4_literal_reaches_a_recipe() {
        assert_eq!(checked_address(Some("192.168.1.50")), Ok("192.168.1.50"));
        assert!(checked_address(None)
            .unwrap_err()
            .starts_with("missing answer"));
        for bad in ["gw.lan", "192.168.1.50; rm -rf /", "10.0.0", ""] {
            assert!(checked_address(Some(bad)).is_err(), "{bad}");
        }
    }
}
