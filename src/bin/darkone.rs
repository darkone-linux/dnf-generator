//! `darkone`: the DNF assistant. `darkone new` installs the first host of a
//! network, its admin host, from the DNF installer image.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use dnf_generator::installer::answers::{Answers, Plan};
use dnf_generator::installer::ask;
use dnf_generator::installer::catalog::Catalog;
use dnf_generator::installer::machine::Machine;
use dnf_generator::installer::project::{self, log, Options, Passphrase, IP_PREFIX, ZONE};
use dnf_generator::installer::{Result, INSTALLER_MARKER};

#[derive(Parser)]
#[command(name = "darkone", version, about = "Darkone NixOS Framework assistant")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install this machine as the first host of a new DNF network, its admin
    /// host. Erases the chosen disk.
    New(NewArgs),
}

#[derive(Args)]
struct NewArgs {
    /// Answers file (YAML): each answer it gives skips its question.
    #[arg(long, value_name = "FILE")]
    answers: Option<PathBuf>,

    /// Fail on any missing answer instead of asking.
    #[arg(long)]
    non_interactive: bool,

    /// Erase the disk without asking for a confirmation.
    #[arg(long)]
    yes: bool,

    /// Show the plan and change nothing; runs on any machine.
    #[arg(long)]
    dry_run: bool,

    /// Passphrase of an encrypted disk, never in the answers file, which
    /// circulates. Without it, the passphrase is asked.
    #[arg(long, value_name = "FILE")]
    luks_passphrase_file: Option<PathBuf>,

    /// Where the project is written before it moves to /etc/nixos of the new
    /// system.
    #[arg(long, value_name = "DIR", default_value = "/tmp/dnf-project")]
    project: PathBuf,

    /// Framework tree holding the project template (set by the installer image).
    #[arg(long, value_name = "DIR", env = "DNF_SOURCE")]
    dnf_source: PathBuf,

    /// Framework flake URL of the project. Default: the template's, the release
    /// of this installer image.
    #[arg(long, value_name = "URL")]
    dnf_url: Option<String>,
}

fn main() -> ExitCode {
    let Command::New(args) = Cli::parse().command;
    match new(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[ \x1b[1;36mDNF\x1b[0m ] \x1b[1;31mERR\x1b[0m • {e}");
            ExitCode::FAILURE
        }
    }
}

fn new(args: NewArgs) -> Result<()> {
    if !args.dry_run && !Path::new(INSTALLER_MARKER).exists() {
        return Err(
            "not on the DNF installer image, whose disk this command erases: boot it first \
             (--dry-run shows the plan anywhere)"
                .to_string(),
        );
    }
    let mut answers = match &args.answers {
        Some(path) => Answers::load(path)?,
        None => Answers::default(),
    };
    let catalog = Catalog::load(&args.dnf_source)?;
    let machine = Machine::detect();
    if !args.non_interactive {
        if !std::io::stdin().is_terminal() {
            return Err(
                "no terminal to ask on: give every answer with --answers --non-interactive"
                    .to_string(),
            );
        }
        ask::complete(&mut answers, &machine, &catalog)?;
    }
    let plan = Plan::resolve(&answers, &machine, &catalog).map_err(|errors| {
        format!(
            "the answers do not allow an install:\n  - {}",
            errors.join("\n  - ")
        )
    })?;

    // Before the recap: everything is typed before the confirmation.
    let luks = match (&args.luks_passphrase_file, plan.encrypt) {
        (_, false) => None,
        (Some(file), true) => Some(Passphrase::from_file(file, ask::MIN_PASSWORD)?),
        (None, true) if args.non_interactive => {
            return Err(
                "disk.encrypt: give the disk passphrase with --luks-passphrase-file".to_string(),
            )
        }
        (None, true) if args.dry_run => None,
        (None, true) => Some(Passphrase::write(&ask::luks_passphrase()?)?),
    };
    let opts = Options {
        dnf_url: Some(match args.dnf_url {
            Some(url) => url,
            None => project::template_dnf_url(&args.dnf_source)?,
        }),
        project: args.project,
        dnf_source: args.dnf_source,
    };
    let recap = recap(&plan, &opts);
    match (args.non_interactive, args.dry_run, args.yes) {
        (false, true, _) => ask::show(&recap)?,
        (false, false, false) => ask::confirm(&recap, &plan.device)?,
        (true, false, false) => {
            return Err("--non-interactive needs --yes: the disk will be erased".to_string())
        }
        _ => {
            log("Plan:");
            println!("    {}", recap.replace('\n', "\n    "));
        }
    }
    if args.dry_run {
        return Ok(());
    }
    project::create(&plan, &opts)?;
    project::install(&plan, &opts, luks.as_ref())?;
    log("Done: remove the installation media, then reboot.");
    Ok(())
}

/// Definitive choices are flagged: nothing changes them after the install (D5).
fn recap(plan: &Plan, opts: &Options) -> String {
    let email = plan
        .email
        .as_ref()
        .map(|e| format!(", {e}"))
        .unwrap_or_default();
    let keyboard = plan.keyboard.as_ref().map(|k| format!("keyboard {k}"));
    let locale = [plan.locale.clone(), keyboard, plan.timezone.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    [
        (
            "Admin",
            format!("{} ({}{email}), definitive login", plan.login, plan.name),
        ),
        ("Network", format!("{}, definitive", plan.domain)),
        ("Zone", format!("{ZONE}, {IP_PREFIX}.0.0/16, definitive")),
        ("Host", format!("{}, {}", plan.hostname, plan.profile)),
        (
            "Disk",
            format!(
                "{} ({}), ERASED, layout {}, {}",
                plan.device,
                plan.device_label,
                plan.disko_profile,
                if plan.encrypt {
                    "encrypted (passphrase at each boot)"
                } else {
                    "not encrypted"
                }
            ),
        ),
        (
            "Locale",
            if locale.is_empty() {
                "framework defaults".to_string()
            } else {
                locale
            },
        ),
        ("Framework", opts.dnf_url.clone().unwrap_or_default()),
    ]
    .iter()
    .map(|(key, value)| format!("{key:<10} {value}"))
    .collect::<Vec<_>>()
    .join("\n")
}
