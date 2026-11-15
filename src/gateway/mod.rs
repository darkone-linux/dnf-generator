//! `darkone add gateway`: the first gateway of the zone `darkone new` created,
//! installed from the admin host on a machine booted on the installer image.
//!
//! - `answers`: the answers file (`--answers`);
//! - `ask`: the questions for what the answers and the probe leave out;
//! - `plan`: proposals from the probe, the checked plan and its recap;
//! - `probe`: the inventory `just _probe` prints (schema 1);
//! - `project`: the project, its guards and what the plan reads of its config.
//!
//! What touches the system stays in the recipes: `just _pair`, `just _probe`.

pub mod answers;
pub mod ask;
pub mod plan;
pub mod probe;
pub mod project;

use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use answers::GatewayAnswers;
use plan::{propose, AdminMachine, GatewayPlan};
use probe::Probe;

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
    match (interactive, opts.dry_run, answers.confirm) {
        (true, true, _) => ask::show(&recap)?,
        (true, false, Some(true)) => {
            cliclack::note("Plan", &recap).map_err(crate::installer::ask::aborted)?
        }
        (true, false, _) => ask::confirm(&recap, &plan.hostname, &plan.disk.path)?,
        (false, false, Some(true)) | (false, true, _) => {
            log("Plan:");
            println!("    {}", recap.replace('\n', "\n    "));
        }
        (false, false, _) => {
            return Err(
                "--non-interactive needs `confirm: true` in the answers: the disk will be erased"
                    .to_string(),
            )
        }
    }
    if opts.dry_run {
        return Ok(());
    }
    apply(&root, &plan)
}

/// Extension point, spec 2b.5 steps 5 to 8, still to write:
/// 5. `yq` edit of `etc/config.yaml` from `plan`, `git diff`, confirmation;
/// 6. `just clean`, commit, `just full-install <hostname> nixos <target>`,
///    the confirmation of `install` passed by an environment variable;
/// 7. `just apply-local`, checks: default route, name of the gateway, Internet;
/// 8. "step 2 of 4", the next step.
fn apply(_root: &Path, plan: &GatewayPlan) -> Result<()> {
    Err(format!(
        "writing etc/config.yaml and installing {} are not implemented yet: nothing \
         changed (--dry-run shows the plan)",
        plan.hostname
    ))
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

/// `just _pair`: the code is typed on `/dev/tty`, never an answer or an
/// argument. stdout carries the host key alone, messages go to stderr.
fn pair(root: &Path, address: &str) -> Result<String> {
    log(&format!("Pairing with {address}..."));
    let key = just_stdout(root, &["_pair", address, INSTALLER_USER], Stdio::inherit())?;
    let key = key.trim();
    if key.starts_with("ssh-ed25519 ") {
        Ok(key.to_string())
    } else {
        Err(format!(
            "`just _pair` printed no ed25519 host key for {address}"
        ))
    }
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
