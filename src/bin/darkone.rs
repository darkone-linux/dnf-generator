//! `darkone`: the DNF assistant. `darkone new` installs the first host of a
//! network, its admin host, from the DNF installer image.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use dnf_generator::installer::answers::{Answers, Plan};
use dnf_generator::installer::ask;
use dnf_generator::installer::catalog::Catalog;
use dnf_generator::installer::machine::{self, Machine};
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

    /// Passphrase of the backup of the admin keys, for an unencrypted disk; an
    /// encrypted one opens its backup with the disk passphrase.
    #[arg(long, value_name = "FILE")]
    backup_passphrase_file: Option<PathBuf>,

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

    // Checked before anything is typed or erased.
    match &plan.backup_device {
        Some(device) if !args.dry_run => check_backup_device(device, &plan.device)?,
        None if args.non_interactive => {
            return Err(
                "missing answer: backup.device, the USB stick of the key backup".to_string(),
            )
        }
        _ => {}
    }

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
        (None, true) => Some(Passphrase::write("luks", &ask::luks_passphrase()?)?),
    };
    let backup_passphrase = match (&args.backup_passphrase_file, plan.encrypt) {
        (Some(_), true) => return Err(
            "--backup-passphrase-file: the disk passphrase opens the backup of an encrypted host"
                .to_string(),
        ),
        (None, true) => None,
        (Some(file), false) => Some(Passphrase::from_file(file, ask::MIN_PASSWORD)?),
        (None, false) if args.non_interactive => {
            return Err(
                "give the passphrase of the key backup with --backup-passphrase-file".to_string(),
            )
        }
        (None, false) if args.dry_run => None,
        (None, false) => Some(Passphrase::write("backup", &ask::backup_passphrase()?)?),
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
    let passphrase = backup_passphrase
        .as_ref()
        .or(luks.as_ref())
        .expect("asked or given above");
    if backup(&plan, &opts, passphrase, !args.non_interactive)? {
        log("Done: keep the USB stick away from this machine, remove the installation media, then reboot.");
    } else {
        log(&format!(
            "Done, keys NOT backed up: {BACKUP_LATER}. Remove the installation media, then reboot."
        ));
    }
    Ok(())
}

/// The stick named by the answers, checked before the disk is erased.
fn check_backup_device(device: &str, install_device: &str) -> Result<()> {
    let disk = machine::disk_of(device)
        .ok_or_else(|| format!("backup.device: {device} not found, plug the USB stick in"))?;
    if machine::disk_of(install_device).as_deref() == Some(disk.as_str()) {
        return Err(format!(
            "backup.device: {device} lies on the disk the install erases"
        ));
    }
    Ok(())
}

/// How a skipped or failed backup is done from the installed system.
const BACKUP_LATER: &str = "once booted, cd /etc/nixos && just backup-admin <device>";

/// Written, then read back: the install is not over without a backup that
/// opens. Interactive, a failure leads back to the choice of the stick, which
/// may skip it: `false`.
fn backup(plan: &Plan, opts: &Options, passphrase: &Passphrase, interactive: bool) -> Result<bool> {
    let not_backed_up = |why: &str| {
        format!("{why}: the system is installed but its keys are not backed up; {BACKUP_LATER}")
    };
    if interactive {
        ask::backup_intro().map_err(|_| not_backed_up("backup aborted"))?;
    }
    let mut given = plan.backup_device.clone();
    loop {
        let device = match given.take() {
            Some(device) => device,
            None => match ask::backup_target().map_err(|_| not_backed_up("backup aborted"))? {
                Some(device) => device,
                None => return Ok(false),
            },
        };
        let done = project::backup(opts, &device, passphrase, false)
            .and_then(|()| {
                if interactive {
                    ask::replug(&device)
                } else {
                    Ok(())
                }
            })
            .and_then(|()| project::backup(opts, &device, passphrase, true));
        match done {
            Ok(()) if interactive => return ask::backup_done().map(|()| true),
            Ok(()) => return Ok(true),
            Err(e) if interactive => ask::backup_failed(&e),
            Err(e) => return Err(not_backed_up(&e)),
        }
    }
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
        (
            "Backup",
            format!(
                "admin keys and project, encrypted, {}",
                match &plan.backup_device {
                    Some(device) => format!("on {device}"),
                    None => "on a USB stick chosen after the install".to_string(),
                }
            ),
        ),
        (
            "Cache",
            match &plan.cache {
                Some(url) => format!("{url}, for the install only"),
                None => "cache.nixos.org".to_string(),
            },
        ),
        ("Framework", opts.dnf_url.clone().unwrap_or_default()),
    ]
    .iter()
    .map(|(key, value)| format!("{key:<10} {value}"))
    .collect::<Vec<_>>()
    .join("\n")
}
