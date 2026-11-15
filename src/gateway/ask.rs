//! Questions of `darkone add gateway`: what the answers file leaves out and
//! the probe cannot tell. Each answer lands in `GatewayAnswers`, then the same
//! `GatewayPlan::resolve` as a scripted run checks the whole.

use std::net::Ipv4Addr;

use super::answers::{GatewayAnswers, GATEWAY_ANSWERS_VERSION};
use super::plan::{lan_proposal, Proposals, DEFAULT_HOSTNAME};
use super::probe::{Probe, ProbeNic};
use super::Result;
use crate::installer::ask::{aborted, rule};
use crate::nix_generator::validation::RE_HOSTNAME;

pub fn intro() -> Result<()> {
    cliclack::intro("Darkone NixOS Framework: the gateway of the zone").map_err(aborted)
}

/// Read on the screen of the machine, booted on the installer image.
pub fn address(answers: &mut GatewayAnswers) -> Result<()> {
    answers.version.get_or_insert(GATEWAY_ANSWERS_VERSION);
    if answers.address.is_some() {
        return Ok(());
    }
    cliclack::note(
        "The machine to install",
        "Plug its WAN card into the box, boot it on the DNF installer, then\n\
         choose entry 2 of its menu. Its screen shows its address and a\n\
         pairing code, asked next.",
    )
    .map_err(aborted)?;
    let address: String = cliclack::input("Address shown on its screen")
        .placeholder("192.168.1.50")
        .validate(|v: &String| match v.parse::<Ipv4Addr>() {
            Ok(_) => Ok(()),
            Err(_) => Err("an IPv4 address, like 192.168.1.50"),
        })
        .interact()
        .map_err(aborted)?;
    answers.address = Some(address);
    Ok(())
}

/// `enp1s0  aa:bb:… · link · 192.168.1.50/24`.
fn hint(n: &ProbeNic) -> String {
    let mut parts = vec![
        n.mac.clone(),
        (if n.link { "link" } else { "no link" }).to_string(),
    ];
    parts.extend(n.ipv4.iter().cloned());
    parts.join(" · ")
}

/// After the probe: name, WAN when no route tells it, LAN, disk.
pub fn complete(answers: &mut GatewayAnswers, probe: &Probe, proposals: &Proposals) -> Result<()> {
    if answers.host.hostname.is_none() {
        let name: String = cliclack::input("Name of the gateway")
            .default_input(DEFAULT_HOSTNAME)
            .validate(rule(RE_HOSTNAME, "a letter, then letters, digits, _ or -"))
            .interact()
            .map_err(aborted)?;
        answers.host.hostname = Some(name);
    }

    // Fewer than two wired cards: no question, the plan says why.
    let wired: Vec<&ProbeNic> = probe.wired().collect();
    if wired.len() < 2 {
        return disk(answers, proposals);
    }
    if answers.nics.wan.is_none() && proposals.wan.is_none() {
        cliclack::log::warning(
            "No wired card carries the default route: which one goes to the box?",
        )
        .ok();
        let mut select = cliclack::select("WAN card, to the box");
        for n in &wired {
            select = select.item(n.name.clone(), &n.name, hint(n));
        }
        answers.nics.wan = Some(select.interact().map_err(aborted)?);
    }
    let wan = answers.nics.wan.clone().or(proposals.wan.clone());
    if answers.nics.lan.is_none() {
        let proposed = lan_proposal(probe, wan.as_deref());
        let mut select = cliclack::multiselect(format!(
            "LAN cards, bridged for the zone (WAN: {})",
            wan.as_deref().unwrap_or("?")
        ))
        .initial_values(proposed.clone())
        .required(true);
        for name in &proposed {
            let n = probe.nic(name).expect("proposed from the probe");
            select = select.item(name.clone(), name, hint(n));
        }
        answers.nics.lan = Some(select.interact().map_err(aborted)?);
    }
    disk(answers, proposals)
}

fn disk(answers: &mut GatewayAnswers, proposals: &Proposals) -> Result<()> {
    if answers.disk.device.is_some() || proposals.disks.len() < 2 {
        return Ok(());
    }
    let mut select = cliclack::select("Disk of the gateway, erased");
    for d in &proposals.disks {
        let stable = if d.by_id.is_some() {
            ""
        } else {
            " · no stable name"
        };
        select = select.item(d.path.clone(), &d.path, format!("{}{stable}", d.label()));
    }
    answers.disk.device = Some(select.interact().map_err(aborted)?);
    Ok(())
}

/// The plan of a dry run, which ends the session.
pub fn show(recap: &str) -> Result<()> {
    cliclack::note("Plan", recap).map_err(aborted)?;
    cliclack::outro("Dry run: nothing changed").map_err(aborted)
}

/// The plan, then a confirmation that defaults to no.
pub fn confirm(recap: &str, hostname: &str, disk: &str) -> Result<()> {
    cliclack::note("Plan", recap).map_err(aborted)?;
    let go = cliclack::confirm(format!(
        "Write etc/config.yaml, erase {disk} and install {hostname}?"
    ))
    .initial_value(false)
    .interact()
    .map_err(aborted)?;
    if go {
        Ok(())
    } else {
        cliclack::outro_cancel("Nothing changed").ok();
        Err("aborted, nothing changed".to_string())
    }
}
