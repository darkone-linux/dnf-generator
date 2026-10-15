//! `darkone`: the DNF assistant. `darkone new` installs the first host of a
//! network, its admin host, from the DNF installer image.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use dnf_generator::installer::answers::{Answers, Plan};
use dnf_generator::installer::machine::Machine;
use dnf_generator::installer::project::{self, log, Options, IP_PREFIX, ZONE};
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
    let answers = match &args.answers {
        Some(path) => Answers::load(path)?,
        None => Answers::default(),
    };

    // Questions come with the interactive mode: until then, every answer
    // missing from the file is an error, with or without `--non-interactive`.
    let plan = Plan::resolve(&answers, &Machine::detect()).map_err(|errors| {
        let hint = if args.non_interactive {
            ""
        } else {
            "\n  (questions are not available yet: give every answer with --answers)"
        };
        format!(
            "the answers do not allow an install:\n  - {}{hint}",
            errors.join("\n  - ")
        )
    })?;
    let opts = Options {
        dnf_url: Some(match args.dnf_url {
            Some(url) => url,
            None => project::template_dnf_url(&args.dnf_source)?,
        }),
        project: args.project,
        dnf_source: args.dnf_source,
    };
    recap(&plan, &opts);
    if args.dry_run {
        return Ok(());
    }
    if !args.yes {
        if args.non_interactive {
            return Err("--non-interactive needs --yes: the disk will be erased".to_string());
        }
        confirm(&plan)?;
    }
    project::create(&plan, &opts)?;
    project::install(&plan, &opts)?;
    log("Done: remove the installation media, then reboot.");
    Ok(())
}

/// Definitive choices are flagged: nothing changes them after the install (D5).
fn recap(plan: &Plan, opts: &Options) {
    let email = plan
        .email
        .as_ref()
        .map(|e| format!(", {e}"))
        .unwrap_or_default();
    let locale = match (&plan.locale, &plan.timezone) {
        (None, None) => "framework defaults".to_string(),
        (l, t) => [l.as_deref(), t.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", "),
    };
    log("Plan:");
    for (key, value) in [
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
                "{} ({}), ERASED, layout {}",
                plan.device, plan.device_label, plan.disko_profile
            ),
        ),
        ("Locale", locale),
        ("Framework", opts.dnf_url.clone().unwrap_or_default()),
    ] {
        println!("    {key:<10} {value}");
    }
}

fn confirm(plan: &Plan) -> Result<()> {
    print!(
        "Erase {} and install {}? (y/N) ",
        plan.device, plan.hostname
    );
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    if answer.trim() == "y" {
        Ok(())
    } else {
        Err("aborted, nothing changed".to_string())
    }
}
