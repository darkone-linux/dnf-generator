//! Steps 6 to 8 of `darkone add gateway`: `just clean`, the commit, `just
//! full-install` (recabling pause included), `just apply-local`, the checks.
//!
//! Every recipe runs on the terminal of `darkone`: its questions, `sudo` and
//! the pairing code are the operator's. A failure names what is left to run.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::plan::GatewayPlan;
use super::{log, Result, INSTALLER_USER};
use crate::installer::checks::internet_error;

/// `install` skips its "format disks" question: the recap of `darkone` got
/// the answer (`dnf/just/project/install.just`).
pub const ENV_CONFIRMED: &str = "DNF_INSTALL_CONFIRMED";

/// Host key the machine was paired and probed with: `install` refuses to go
/// on with another one.
pub const ENV_HOST_KEY: &str = "DNF_PAIR_HOSTKEY";

/// Never routed here: `ip route get` resolves a route, sends nothing.
const ROUTE_PROBE: &str = "1.1.1.1";

/// After `apply-local`: NetworkManager may still be renewing its lease.
const SETTLE: Duration = Duration::from_secs(30);
const RETRY_EVERY: Duration = Duration::from_secs(2);

/// One `getent`: a dead resolver must not hold the check.
const LOOKUP_LIMIT: Duration = Duration::from_secs(10);

/// `curl` bounds each try itself (20 s).
const INTERNET_TRIES: u32 = 2;

/// Steps of the chain, in order: where it stopped, what is left.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum Step {
    Clean,
    Commit,
    Install,
    ApplyLocal,
    Checks,
}

/// One line, closed type list of the framework, 80 characters at most.
pub fn commit_message(plan: &GatewayPlan) -> String {
    format!("chore(gateway): add {}", plan.hostname)
}

/// The commands left from `from` on, numbered: what the operator runs by hand
/// after a failure. Names and addresses of the plan only, all checked.
pub fn remaining(plan: &GatewayPlan, from: Step) -> String {
    let host = &plan.hostname;
    let steps = [
        (Step::Clean, "just clean".to_string()),
        (
            Step::Commit,
            format!("git add -A && git commit -m \"{}\"", commit_message(plan)),
        ),
        (
            Step::Install,
            format!(
                "just full-install {host} {INSTALLER_USER} {}   # {host} still on the installer",
                plan.target
            ),
        ),
        (
            Step::ApplyLocal,
            format!("just apply-local   # this machine takes {}", plan.admin.ip),
        ),
        (
            Step::Checks,
            format!(
                "ip route get {ROUTE_PROBE}; getent hosts {host}   # both {}, then Internet",
                plan.lan_ip
            ),
        ),
    ];
    steps
        .iter()
        .filter(|(step, _)| *step >= from)
        .enumerate()
        .map(|(i, (_, command))| format!("  {}. {command}", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The failure of `step`, with the way out. Committed: `darkone add gateway`
/// refuses a zone that has its gateway, the rest is run by hand.
pub fn failure(plan: &GatewayPlan, step: Step, why: &str) -> String {
    let host = &plan.hostname;
    let (state, undo) = match step {
        Step::Clean | Step::Commit => (
            "etc/config.yaml is written, nothing is committed, nothing is installed".to_string(),
            "\nOr undo, then run darkone add gateway again:\n  git stash --include-untracked",
        ),
        Step::Install => (
            format!(
                "{host} is declared and committed, so darkone add gateway will not run again \
                 (the zone has its gateway); the last message of the recipe tells how far the \
                 install went"
            ),
            "",
        ),
        Step::ApplyLocal | Step::Checks => (
            format!("{host} is installed, this machine is not applied yet"),
            "",
        ),
    };
    format!(
        "{why}: {state}. To go on, in the project:\n{}{undo}",
        remaining(plan, step)
    )
}

/// A recipe on this terminal: stdin, stdout and stderr inherited.
fn just(root: &Path, args: &[&str], env: &[(&str, &str)]) -> std::result::Result<(), String> {
    let status = Command::new("just")
        .args(args)
        .envs(env.iter().copied())
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run just: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`just {}` failed ({status})", args[0]))
    }
}

fn git(root: &Path, args: &[&str]) -> std::result::Result<(), String> {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`git {}` failed ({status})", args[0]))
    }
}

/// Steps 6 and 7, `etc/config.yaml` written. The recap of `darkone` was
/// accepted: `install` asks nothing more, and is held to the paired host key.
pub fn run(root: &Path, plan: &GatewayPlan, host_key: &str) -> Result<()> {
    let host = plan.hostname.as_str();
    let target = plan.target.to_string();
    let fail = |step: Step| move |why: String| failure(plan, step, &why);

    log("Checking and generating (just clean)...");
    just(root, &["clean"], &[]).map_err(fail(Step::Clean))?;
    log("Committing the declaration...");
    git(root, &["add", "-A"])
        .and_then(|()| git(root, &["commit", "-q", "-m", &commit_message(plan)]))
        .map_err(fail(Step::Commit))?;

    log(&format!(
        "Installing {host} on {target} (just full-install)..."
    ));
    just(
        root,
        &["full-install", host, INSTALLER_USER, &target],
        &[(ENV_CONFIRMED, "1"), (ENV_HOST_KEY, host_key)],
    )
    .map_err(fail(Step::Install))?;

    log("Applying this machine, behind its gateway (just apply-local)...");
    just(root, &["apply-local"], &[]).map_err(fail(Step::ApplyLocal))?;

    let failed = checks(plan);
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{host} is installed, but {} of 3 checks failed:\n  - {}\nStep 2 is done once \
             they pass:\n{}",
            failed.len(),
            failed.join("\n  - "),
            remaining(plan, Step::Checks)
        ))
    }
}

/// Default route, name of the gateway, Internet: each said as it ends, the
/// failures returned.
fn checks(plan: &GatewayPlan) -> Vec<String> {
    let host = plan.hostname.as_str();
    let lan_ip = plan.lan_ip.as_str();
    log("Checking the network behind the gateway...");
    let mut failed = Vec::new();
    let mut report = |result: std::result::Result<String, String>| match result {
        Ok(fine) => log(&format!("\x1b[32mOK\x1b[0m {fine}")),
        Err(why) => {
            log(&format!("\x1b[1;31mKO\x1b[0m {why}"));
            failed.push(why);
        }
    };
    report(retry(SETTLE, RETRY_EVERY, || {
        route_verdict(route_via().as_deref(), host, lan_ip)
    }));
    report(retry(SETTLE, RETRY_EVERY, || {
        name_verdict(resolve(host).as_deref(), host, lan_ip)
    }));
    report(internet(host));
    failed
}

/// `check` again every `pause` until it passes or `limit` is over: its last
/// failure.
fn retry<T>(
    limit: Duration,
    pause: Duration,
    mut check: impl FnMut() -> std::result::Result<T, String>,
) -> std::result::Result<T, String> {
    let deadline = Instant::now() + limit;
    loop {
        match check() {
            Ok(fine) => return Ok(fine),
            Err(why) if Instant::now() >= deadline => return Err(why),
            Err(_) => sleep(pause),
        }
    }
}

/// stdout of a command that must end within `limit`; `None`: it failed, or
/// was killed. Small outputs only: read once the command is over.
fn output_within(mut command: Command, limit: Duration) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(100)),
            _ => {
                child.kill().ok();
                child.wait().ok();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `1.1.1.1 via 10.1.1.1 dev enp1s0 src …` → `10.1.1.1`; `None`: no route,
/// or a directly connected one.
fn via_of(route_get: &str) -> Option<String> {
    let mut words = route_get.split_whitespace();
    words.find(|w| *w == "via")?;
    words.next().map(str::to_string)
}

/// Next hop of the route to the Internet.
fn route_via() -> Option<String> {
    let mut ip = Command::new("ip");
    ip.args(["-4", "-o", "route", "get", ROUTE_PROBE]);
    via_of(&output_within(ip, LOOKUP_LIMIT)?)
}

fn route_verdict(
    via: Option<&str>,
    host: &str,
    lan_ip: &str,
) -> std::result::Result<String, String> {
    match via {
        Some(via) if via == lan_ip => Ok(format!("default route through {host} ({lan_ip})")),
        Some(via) => Err(format!(
            "the default route goes through {via}, not {host} ({lan_ip}): plug this machine \
             into a LAN port of {host} only (cable to the box unplugged, its Wi-Fi left)"
        )),
        None => Err(format!(
            "no default route: plug this machine into a LAN port of {host}, it gives the \
             address and the route (DHCP)"
        )),
    }
}

/// `10.1.1.1  STREAM gw` → `10.1.1.1`, first answer of `getent ahostsv4`.
fn first_address(ahosts: &str) -> Option<String> {
    ahosts.split_whitespace().next().map(str::to_string)
}

fn resolve(host: &str) -> Option<String> {
    let mut getent = Command::new("getent");
    getent.args(["ahostsv4", host]);
    first_address(&output_within(getent, LOOKUP_LIMIT)?)
}

fn name_verdict(
    address: Option<&str>,
    host: &str,
    lan_ip: &str,
) -> std::result::Result<String, String> {
    match address {
        Some(address) if address == lan_ip => Ok(format!("{host} resolves to {lan_ip}")),
        Some(address) => Err(format!(
            "{host} resolves to {address}, not {lan_ip}: the DNS in use is not the one of \
             {host} (another network still connected?)"
        )),
        None => Err(format!(
            "{host} does not resolve: the DNS of this machine is not {host} ({lan_ip}) yet; \
             unplug and plug the cable again, or reboot"
        )),
    }
}

fn internet(host: &str) -> std::result::Result<String, String> {
    let mut last = String::new();
    for _ in 0..INTERNET_TRIES {
        match internet_error() {
            None => return Ok("Internet answers (cache.nixos.org)".to_string()),
            Some(why) => last = why,
        }
    }
    Err(format!(
        "no Internet through {host}: {last}; its WAN card must be plugged into the box"
    ))
}

#[cfg(test)]
mod tests {
    use super::super::plan::{AdminMac, AdminPlan, PlanDisk, PlanNic};
    use super::*;

    fn nic(name: &str) -> PlanNic {
        PlanNic {
            name: name.to_string(),
            mac: "aa:bb:cc:00:10:01".to_string(),
            link: true,
            route: true,
            ipv4: vec![],
        }
    }

    fn plan() -> GatewayPlan {
        GatewayPlan {
            zone: "main".to_string(),
            ip_prefix: "10.1".to_string(),
            hostname: "gw".to_string(),
            zone_spec: "main:1.1".to_string(),
            lan_ip: "10.1.1.1".to_string(),
            target: "192.168.1.50".parse().unwrap(),
            host_key: Some("ssh-ed25519 AAAAexample".to_string()),
            wan: nic("enp1s0"),
            lan: vec![nic("enp2s0")],
            disk: PlanDisk {
                path: "/dev/sda".to_string(),
                dev: "/dev/sda".to_string(),
                stable: false,
                label: "256 GB".to_string(),
            },
            disko_profile: "server-1-disk".to_string(),
            admin: AdminPlan {
                hostname: "poste".to_string(),
                zone_spec: "main:2.1".to_string(),
                ip: "10.1.2.1".to_string(),
                new_address: true,
                mac: AdminMac::Unknown,
            },
            virt: "none".to_string(),
            ram_bytes: 0,
            warnings: vec![],
        }
    }

    #[test]
    fn commit_message_fits_the_framework_rule() {
        assert_eq!(commit_message(&plan()), "chore(gateway): add gw");
        let mut long = plan();
        long.hostname = format!("g{}", "w".repeat(59));
        assert!(commit_message(&long).len() <= 80);
    }

    #[test]
    fn a_failure_says_exactly_what_is_left() {
        let after_commit = failure(&plan(), Step::Install, "`just full-install` failed");
        assert!(
            after_commit.starts_with("`just full-install` failed: gw is declared and committed"),
            "{after_commit}"
        );
        assert!(after_commit.contains("darkone add gateway will not run again"));
        assert!(
            after_commit.contains("  1. just full-install gw nixos 192.168.1.50"),
            "{after_commit}"
        );
        assert!(
            after_commit.contains("  2. just apply-local"),
            "{after_commit}"
        );
        assert!(
            after_commit.contains("  3. ip route get 1.1.1.1; getent hosts gw   # both 10.1.1.1"),
            "{after_commit}"
        );
        assert!(!after_commit.contains("just clean"), "{after_commit}");
        assert!(!after_commit.contains("git stash"), "{after_commit}");

        // Before the commit: every step, and the way back.
        let before = failure(&plan(), Step::Clean, "`just clean` failed");
        assert!(before.contains("nothing is committed"), "{before}");
        assert!(before.contains("  1. just clean\n"), "{before}");
        assert!(
            before.contains("  2. git add -A && git commit -m \"chore(gateway): add gw\""),
            "{before}"
        );
        assert!(
            before.contains("  3. just full-install gw nixos"),
            "{before}"
        );
        assert!(before.contains("git stash --include-untracked"), "{before}");

        let installed = failure(&plan(), Step::ApplyLocal, "`just apply-local` failed");
        assert!(installed.contains("gw is installed"), "{installed}");
        assert!(installed.contains("  1. just apply-local"), "{installed}");
        assert!(!installed.contains("full-install"), "{installed}");
    }

    #[test]
    fn default_route_must_go_through_the_gateway() {
        assert_eq!(
            via_of("1.1.1.1 via 10.1.1.1 dev enp1s0 src 10.1.2.1 uid 1000 \\    cache"),
            Some("10.1.1.1".to_string())
        );
        assert_eq!(via_of("10.1.1.1 dev enp1s0 src 10.1.2.1 uid 1000"), None);
        assert_eq!(via_of(""), None);

        assert_eq!(
            route_verdict(Some("10.1.1.1"), "gw", "10.1.1.1").unwrap(),
            "default route through gw (10.1.1.1)"
        );
        let box_route = route_verdict(Some("192.168.1.1"), "gw", "10.1.1.1").unwrap_err();
        assert!(
            box_route.contains("goes through 192.168.1.1, not gw"),
            "{box_route}"
        );
        let none = route_verdict(None, "gw", "10.1.1.1").unwrap_err();
        assert!(none.contains("no default route"), "{none}");
    }

    #[test]
    fn gateway_name_must_resolve_to_its_lan_address() {
        assert_eq!(
            first_address("10.1.1.1       STREAM gw\n10.1.1.1       DGRAM  \n"),
            Some("10.1.1.1".to_string())
        );
        assert_eq!(first_address(""), None);
        assert!(name_verdict(Some("10.1.1.1"), "gw", "10.1.1.1").is_ok());

        // A provider DNS answering every unknown name.
        let hijacked = name_verdict(Some("203.0.113.9"), "gw", "10.1.1.1").unwrap_err();
        assert!(hijacked.contains("resolves to 203.0.113.9, not 10.1.1.1"));
        assert!(name_verdict(None, "gw", "10.1.1.1")
            .unwrap_err()
            .contains("does not resolve"));
    }

    #[test]
    fn a_check_is_retried_then_gives_its_last_failure() {
        let mut calls = 0;
        let passed = retry(Duration::from_secs(5), Duration::from_millis(10), || {
            calls += 1;
            if calls < 2 {
                Err("not yet".to_string())
            } else {
                Ok(calls)
            }
        });
        assert_eq!(passed, Ok(2));
        let failed: std::result::Result<(), String> =
            retry(
                Duration::ZERO,
                RETRY_EVERY,
                || Err("still down".to_string()),
            );
        assert_eq!(failed, Err("still down".to_string()));
    }

    #[test]
    fn a_command_is_bounded() {
        let mut quick = Command::new("sh");
        quick.args(["-c", "echo done"]);
        assert_eq!(
            output_within(quick, Duration::from_secs(5)),
            Some("done\n".to_string())
        );
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);
        let started = Instant::now();
        assert_eq!(output_within(slow, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
        let mut failing = Command::new("sh");
        failing.args(["-c", "exit 3"]);
        assert_eq!(output_within(failing, Duration::from_secs(5)), None);
        assert_eq!(
            output_within(Command::new("/nonexistent/command"), LOOKUP_LIMIT),
            None
        );
    }
}
