//! `darkone`: the DNF assistant. `darkone new` installs the first host of a
//! network, its admin host, from the DNF installer image.

use std::io::{ErrorKind, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use dnf_generator::installer::answers::{saved_answers_path, Answers, Plan, DEFAULT_IP_PREFIX};
use dnf_generator::installer::ask::{self, PreviousRun};
use dnf_generator::installer::catalog::Catalog;
use dnf_generator::installer::checks;
use dnf_generator::installer::machine::{self, Machine};
use dnf_generator::installer::project::{
    self, log, Options, Passphrase, Previous, ResumeFacts, TARGET_ROOT, ZONE,
};
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

    /// Continue the unfinished install of this installer session, its project
    /// and disk kept. Scripted runs only: an interactive one offers it.
    #[arg(long, requires = "non_interactive")]
    resume: bool,

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
    #[arg(long, value_name = "URL", env = "DNF_URL")]
    dnf_url: Option<String>,

    /// Extra package cache of the install, unless the answers file gives one.
    #[arg(long, value_name = "URL", env = "DNF_CACHE")]
    cache: Option<String>,
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

    // `--cache` / `DNF_CACHE` (`just try` image): used when the answers file gives no URL.
    if answers.cache.url.is_none() {
        answers.cache.url = args.cache.clone();
    }
    let catalog = Catalog::load(&args.dnf_source)?;
    if !args.non_interactive {
        if !std::io::stdin().is_terminal() {
            return Err(
                "no terminal to ask on: give every answer with --answers --non-interactive"
                    .to_string(),
            );
        }
        ask::intro()?;
    }

    // Before the first question: a missing network fails here, not in the install.
    checks::preflight(
        answers.cache.url.as_deref(),
        !args.non_interactive,
        args.dry_run,
    )?;

    // A dry run changes nothing: it neither reads nor keeps answers. Scripted
    // runs read their own file. Unreadable saved answers: asked again.
    let saved_path =
        saved_answers_path(std::env::var_os("XDG_RUNTIME_DIR")).filter(|_| !args.dry_run);
    let saved = saved_path
        .as_deref()
        .filter(|_| !args.non_interactive)
        .and_then(|p| Answers::load(p).ok());
    let after = after_previous_run(&args, saved.is_some())?;
    let resume = after == After::Resume;

    // An interactive resume takes the answers its project was written from.
    let mut reused = false;
    if let Some(saved) = saved.filter(|_| resume || args.answers.is_none()) {
        if resume || ask::reuse_answers(&saved)? {
            answers = saved;
            reused = true;
        }
    }

    // A previous run of this session mounted its disk on /mnt: still ours.
    let reclaimed = (after != After::New).then_some(TARGET_ROOT);
    let machine = Machine::detect(reclaimed);
    match (args.non_interactive, reused) {
        (true, _) => {}
        (false, true) => ask::use_keyboard(&answers, &catalog),
        (false, false) => ask::complete(&mut answers, &machine, &catalog)?,
    }
    let plan = Plan::resolve(&answers, &machine, &catalog).map_err(|errors| {
        format!(
            "the answers do not allow an install:\n  - {}",
            errors.join("\n  - ")
        )
    })?;
    if resume {
        project::check_resume(&plan, &args.project)?;
    }

    // A relaunch after a failure skips the questions; not kept is no failure.
    let not_kept = match saved_path.as_deref().map(|p| answers.save(p)) {
        Some(Ok(())) => None,
        Some(Err(e)) => Some(format!("answers not kept for a relaunch: {e}")),
        None if args.dry_run => None,
        None => Some("no XDG_RUNTIME_DIR: answers not kept for a relaunch".to_string()),
    };
    match not_kept {
        Some(why) if args.non_interactive => log(&why),
        Some(why) => {
            cliclack::log::warning(why).ok();
        }
        None => {}
    }

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
        (None, true) if resume => Some(Passphrase::write("luks", &ask::luks_passphrase_again()?)?),
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
    let recap = recap(&plan, &opts, resume);
    match (args.non_interactive, args.dry_run, args.yes) {
        (false, true, _) => ask::show(&recap)?,
        (false, false, false) if resume => ask::resuming(&recap)?,
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

    // Only now: an abort at the recap leaves the earlier run as it was.
    if after == After::Scratch {
        project::remove_previous(&opts.project)?;
    }
    if !resume {
        project::create(&plan, &opts)?;
    }
    project::install(&plan, &opts, luks.as_ref(), resume)?;
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

    // Installed: nothing left to resume, the password hash leaves the RAM too.
    if let Some(path) = &saved_path {
        std::fs::remove_file(path).ok();
    }

    // Scripted runs stop at the message: the caller decides when to reboot.
    if !args.non_interactive && reboot_asked()? {
        reboot()?;
    }
    Ok(())
}

/// Where this run starts from, given what an earlier one left at the project
/// path.
#[derive(Debug, Clone, Copy, PartialEq)]
enum After {
    New,
    Resume,
    Scratch,
}

/// Only an operator's choice deletes a project: a scripted run resumes with
/// `--resume`, or is refused. Every doubt refuses, nothing is erased.
fn after_previous_run(args: &NewArgs, answers_kept: bool) -> Result<After> {
    let dir = &args.project;
    if args.dry_run {
        return Ok(After::New);
    }
    match project::previous(dir) {
        Previous::Foreign => Err(format!(
            "{} is not empty and was not written by the DNF installer: left untouched, \
             move it away or choose another place with --project",
            dir.display()
        )),
        Previous::None if args.resume => Err(format!(
            "--resume: no previous install in {}",
            dir.display()
        )),
        Previous::None => Ok(After::New),
        Previous::Installer if !args.non_interactive => {
            let facts = ResumeFacts::read(dir, answers_kept);
            match ask::previous_run(dir, facts.blocker())? {
                PreviousRun::Resume => Ok(After::Resume),
                PreviousRun::Scratch => Ok(After::Scratch),
                PreviousRun::Abort => Err("aborted, nothing changed".to_string()),
            }
        }
        Previous::Installer if !args.resume => Err(format!(
            "{0} holds the project of a previous install: --resume continues it; to start \
             from scratch, remove it first (rm -rf {0})",
            dir.display()
        )),
        Previous::Installer => match ResumeFacts::read(dir, args.answers.is_some()).blocker() {
            Some(why) => Err(format!(
                "cannot resume: {why}; to start from scratch, remove {} first",
                dir.display()
            )),
            None => Ok(After::Resume),
        },
    }
}

/// Default no: a reboot with the key still plugged boots the installer again.
fn reboot_asked() -> Result<bool> {
    match cliclack::confirm("Remove the installation USB key. Reboot now?")
        .initial_value(false)
        .interact()
    {
        Ok(answer) => Ok(answer),

        // Ctrl-C after the install: staying up is not a failure.
        Err(e) if e.kind() == ErrorKind::Interrupted => Ok(false),
        Err(e) => Err(format!("reboot question: {e}")),
    }
}

/// `sudo`, as the installer recipes do: the ISO user has it passwordless.
fn reboot() -> Result<()> {
    let status = std::process::Command::new("sudo")
        .args(["systemctl", "reboot"])
        .status()
        .map_err(|e| format!("cannot run sudo: {e}; reboot by hand: systemctl reboot"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`sudo systemctl reboot` failed ({status}); reboot by hand: systemctl reboot"
        ))
    }
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
fn recap(plan: &Plan, opts: &Options, resume: bool) -> String {
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
    let mut rows = vec![
        (
            "Admin",
            format!("{} ({}{email}), definitive login", plan.login, plan.name),
        ),
        ("Network", format!("{}, definitive", plan.domain)),
        (
            "Zone",
            format!("{ZONE}, {}.0.0/16, definitive", plan.ip_prefix),
        ),
        ("Host", format!("{}, {}", plan.hostname, plan.profile)),
        (
            "Disk",
            format!(
                "{} ({}), {}, layout {}, {}",
                plan.device,
                plan.device_label,
                if resume {
                    "kept from the previous attempt"
                } else {
                    "ERASED"
                },
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
    ];

    // The default prefix is taken: say so under the zone, the choice is definitive.
    if plan.ip_prefix != DEFAULT_IP_PREFIX {
        let zone = rows.iter().position(|(key, _)| *key == "Zone");
        rows.insert(
            zone.map_or(rows.len(), |i| i + 1),
            (
                "",
                format!(
                    "{DEFAULT_IP_PREFIX} is used by the current network: {} chosen",
                    plan.ip_prefix
                ),
            ),
        );
    }
    rows.iter()
        .map(|(key, value)| format!("{key:<10} {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}
