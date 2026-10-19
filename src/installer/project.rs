//! The project of the new network: copied from the framework template, written
//! in RAM, then installed by `just install-here`, which moves it to /etc/nixos
//! of the new system.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;

use super::answers::Plan;
use super::Result;
use crate::nix_generator::nix_network::DEFAULT_LOCALE;

/// Fixed for good (D5), shown in the recap without a question.
pub const ZONE: &str = "main";
pub const IP_PREFIX: &str = "10.0";

/// First uid of a declared account (`RE_UID` range of the generator).
const ADMIN_UID: u32 = 1000;

pub struct Options {
    /// Where the project is written before it moves to the new system.
    pub project: PathBuf,

    /// Framework tree holding `templates/project`.
    pub dnf_source: PathBuf,

    /// Overrides the framework URL of the template, a release tag.
    pub dnf_url: Option<String>,
}

/// JSON strings are YAML double-quoted scalars: any value, safely quoted.
fn q(value: &str) -> String {
    serde_json::to_string(value).expect("a string always serializes")
}

/// `etc/config.yaml` of the assistant contract (§ config.yaml minimal):
/// zone and prefix by default, plain DHCP until a gateway exists.
pub fn render_config(plan: &Plan) -> String {
    let mut default = String::new();
    if plan.locale.is_some() || plan.timezone.is_some() {
        default.push_str("  default:\n");
        if let Some(locale) = &plan.locale {
            default.push_str(&format!("    locale: {}\n", q(locale)));
        }
        if let Some(tz) = &plan.timezone {
            default.push_str(&format!("    timezone: {}\n", q(tz)));
        }
    }
    let email = plan
        .email
        .as_ref()
        .map(|e| format!("    email: {}\n", q(e)))
        .unwrap_or_default();
    format!(
        r#"# yaml-language-server: $schema=../dnf/config/config.schema.json
#
# Written by the DNF installer. Every key is documented on hover.

network:
  domain: {domain}
{default}  coordination:
    enable: false

zones:
  {ZONE}:
    description: "Main network"
    ipPrefix: {prefix}

users:
  {login}:
    uid: {ADMIN_UID}
    name: {name}
{email}    profile: "nix-admin"
    groups: ["global"]

hosts:
  - hostname: {hostname}
    name: "Admin workstation"
    zone: {zone}
    profile: {profile}
    users: [{login_q}]
    disko:
      profile: {disko}
      devices:
        main: {device}
"#,
        domain = q(&plan.domain),
        prefix = q(IP_PREFIX),
        login = plan.login,
        name = q(&plan.name),
        hostname = q(&plan.hostname),
        zone = q(ZONE),
        profile = q(&plan.profile),
        login_q = q(&plan.login),
        disko = q(&plan.disko_profile),
        device = q(&plan.device),
    )
}

/// `usr/machines/<host>/configuration.nix` of the first host, the admin one:
/// the header of the framework template, then what an admin host needs.
pub fn render_host_config(template: &str) -> String {
    let header: String = template
        .lines()
        .take_while(|l| l.starts_with('#'))
        .map(|l| format!("{l}\n"))
        .collect();
    format!(
        "{header}\n{{ ... }}:\n{{\n\n  \
         # Admin host of the network: fleet tools (`just`, colmena, sops) and git\n  \
         # access to /etc/nixos for the `nix` account (`profile.advanced`).\n  \
         darkone.admin.nix.enable = true;\n  \
         darkone.profile.advanced.enable = true;\n\n  \
         # Next steps, in the browser at the first session.\n  \
         darkone.admin.welcome.enable = true;\n}}\n"
    )
}

/// Framework URL the template pins: the release of this installer image.
pub fn template_dnf_url(dnf_source: &Path) -> Result<String> {
    let flake =
        fs::read_to_string(dnf_source.join("templates/project/flake.nix")).map_err(|e| {
            format!(
                "framework template not found under {}: {e}",
                dnf_source.display()
            )
        })?;
    dnf_url_re()
        .captures(&flake)
        .map(|c| c[2].to_string())
        .ok_or_else(|| "no `dnf.url` in the template flake.nix".to_string())
}

fn dnf_url_re() -> Regex {
    Regex::new(r#"(?m)^(\s*dnf\.url = )"([^"]*)";"#).unwrap()
}

/// Template, configuration, framework link, generated files, first commit.
pub fn create(plan: &Plan, opts: &Options) -> Result<()> {
    let dir = &opts.project;
    if dir.exists()
        && fs::read_dir(dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(format!(
            "{} is not empty: remove it, or choose another place with --project",
            dir.display()
        ));
    }
    log("Project from the framework template...");
    copy_tree(&opts.dnf_source.join("templates/project"), dir)?;
    keep_readme(dir, plan.locale.as_deref().unwrap_or(DEFAULT_LOCALE))?;
    if let Some(url) = &opts.dnf_url {
        let flake = dir.join("flake.nix");
        let text = fs::read_to_string(&flake).map_err(|e| e.to_string())?;
        let text = dnf_url_re().replace(&text, format!("${{1}}\"{url}\";"));
        fs::write(&flake, text.as_ref()).map_err(|e| e.to_string())?;
    }
    fs::write(dir.join("etc/config.yaml"), render_config(plan)).map_err(|e| e.to_string())?;

    // Written before `just generate`, which seeds this file only when missing.
    let template = opts
        .dnf_source
        .join("hosts/templates/usr-machines-configuration.nix");
    let template =
        fs::read_to_string(&template).map_err(|e| format!("{}: {e}", template.display()))?;
    let host_dir = dir.join("usr/machines").join(&plan.hostname);
    fs::create_dir_all(&host_dir).map_err(|e| e.to_string())?;
    fs::write(
        host_dir.join("configuration.nix"),
        render_host_config(&template),
    )
    .map_err(|e| e.to_string())?;

    let email = plan
        .email
        .clone()
        .unwrap_or_else(|| format!("{}@{}", plan.login, plan.domain));

    // The installer image has no `init.defaultBranch`: `master` otherwise.
    run(dir, "git", &["init", "-q", "-b", "main"])?;
    run(dir, "git", &["config", "user.name", &plan.name])?;
    run(dir, "git", &["config", "user.email", &email])?;
    run(dir, "git", &["add", "-A"])?;

    // Locks the framework and links `dnf/` to the locked tree: what `just`
    // imports and the generator reads.
    log("Locking the framework...");
    run(
        dir,
        "nix",
        &[NIX_FEATURES[0], NIX_FEATURES[1], "run", ".#init"],
    )?;
    run(dir, "just", &["generate"])?;
    run(dir, "git", &["add", "-A"])?;
    run(
        dir,
        "git",
        &[
            "commit",
            "-q",
            "-m",
            "chore(new): project created by the DNF installer",
        ],
    )
}

/// One README, in the language of the admin: the template has an English
/// `README.md` and a French `README.fr.md`.
fn keep_readme(dir: &Path, locale: &str) -> Result<()> {
    let french = dir.join("README.fr.md");
    if !french.exists() {
        return Ok(());
    }
    let done = if locale.starts_with("fr_") {
        fs::rename(&french, dir.join("README.md"))
    } else {
        fs::remove_file(&french)
    };
    done.map_err(|e| format!("{}: {e}", french.display()))
}

/// Disks, keys, secrets and `nixos-install`: all in `just install-here`.
pub fn install(plan: &Plan, opts: &Options, luks: Option<&Passphrase>) -> Result<()> {
    let luks = luks.map(|p| p.path.to_string_lossy().into_owned());
    run(
        &opts.project,
        "just",
        &[
            "install-here",
            &plan.hostname,
            &plan.login,
            &plan.password_hash,
            "yes",
            luks.as_deref().unwrap_or_default(),
            plan.cache.as_deref().unwrap_or_default(),
            plan.cache_key.as_deref().unwrap_or_default(),
        ],
    )
}

/// Where `install` leaves the new system mounted, keys included.
const TARGET_ROOT: &str = "/mnt";

/// Encrypted backup of the admin keys and the project on a USB stick, or its
/// check (`just backup-admin`, `just backup-check`).
pub fn backup(opts: &Options, device: &str, passphrase: &Passphrase, check: bool) -> Result<()> {
    let recipe = if check {
        "backup-check"
    } else {
        "backup-admin"
    };
    run(
        &opts.project,
        "just",
        &[
            recipe,
            device,
            &passphrase.path.to_string_lossy(),
            TARGET_ROOT,
        ],
    )
}

/// Passphrase handed to the recipes as a file (LUKS, backup): the one given on
/// the command line, or a typed one written in RAM and removed on drop.
pub struct Passphrase {
    pub path: PathBuf,
    written: bool,
}

impl Passphrase {
    /// Trailing newlines do not count, as for disko's `passwordFile`.
    pub fn from_file(path: &Path, min_len: usize) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if text.trim_end_matches('\n').chars().count() < min_len {
            return Err(format!(
                "{}: a passphrase of {min_len} characters at least",
                path.display()
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            written: false,
        })
    }

    /// Session tmpfs, readable by this user only; `name` tells it apart.
    pub fn write(name: &str, passphrase: &str) -> Result<Self> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("dnf-{name}-{}", std::process::id()));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| f.write_all(passphrase.as_bytes()))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            path,
            written: true,
        })
    }
}

impl Drop for Passphrase {
    fn drop(&mut self) {
        if self.written {
            fs::remove_file(&self.path).ok();
        }
    }
}

const NIX_FEATURES: [&str; 2] = ["--extra-experimental-features", "nix-command flakes"];

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`{program} {}` failed ({status})",
            args.first().unwrap_or(&"")
        ))
    }
}

/// Store files are read-only: the copy becomes the user's, writable.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    fs::set_permissions(dst, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    let entries = fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let meta = fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else if meta.file_type().is_symlink() {
            symlink(fs::read_link(&from).map_err(|e| e.to_string())?, &to)
                .map_err(|e| e.to_string())?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
            let mode = if meta.permissions().mode() & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&to, fs::Permissions::from_mode(mode))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Same rendering as the `_log` recipe of `just/common/_helpers.just`.
pub fn log(msg: &str) {
    println!("[ \x1b[1;36mDNF\x1b[0m ] \x1b[1;35mNEW\x1b[0m • {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnf_url_is_replaced_in_place() {
        let flake = "  inputs = {\n    dnf.url = \"github:o/r/v0.4.0\";\n  };\n";
        let out = dnf_url_re().replace(flake, "${1}\"path:/nix/store/x-source\";");
        assert_eq!(
            out,
            "  inputs = {\n    dnf.url = \"path:/nix/store/x-source\";\n  };\n"
        );
        assert_eq!(
            &dnf_url_re().captures(flake).unwrap()[2],
            "github:o/r/v0.4.0"
        );
    }

    #[test]
    fn readme_in_the_language_of_the_admin() {
        for (locale, expected) in [("fr_FR.UTF-8", "fr"), ("en_US.UTF-8", "en")] {
            let dir = tempfile::TempDir::new().unwrap();
            fs::write(dir.path().join("README.md"), "en").unwrap();
            fs::write(dir.path().join("README.fr.md"), "fr").unwrap();
            keep_readme(dir.path(), locale).unwrap();
            assert_eq!(
                fs::read_to_string(dir.path().join("README.md")).unwrap(),
                expected
            );
            assert!(!dir.path().join("README.fr.md").exists());
        }
    }

    #[test]
    fn host_config_keeps_the_template_header() {
        let template =
            "# Host specific configuration (writable)\n#\n# Never edit those.\n\n{ ... }: { }\n";
        let out = render_host_config(template);
        assert!(out.starts_with("# Host specific configuration (writable)\n#\n# Never edit those.\n\n{ ... }:\n{\n\n  # Admin host"), "{out}");
        assert!(
            out.contains("  darkone.admin.nix.enable = true;\n"),
            "{out}"
        );
        assert!(
            out.contains("  darkone.profile.advanced.enable = true;\n"),
            "{out}"
        );
        assert!(
            out.ends_with("  darkone.admin.welcome.enable = true;\n}\n"),
            "{out}"
        );
        assert!(!out.contains("{ ... }: { }"), "{out}");
    }

    #[test]
    fn passphrase_file_is_checked_and_a_written_one_removed() {
        let dir = tempfile::TempDir::new().unwrap();
        let short = dir.path().join("short");
        fs::write(&short, "1234567\n").unwrap();
        assert!(Passphrase::from_file(&short, 8).is_err());
        assert!(Passphrase::from_file(&dir.path().join("none"), 8).is_err());
        let given = dir.path().join("given");
        fs::write(&given, "12345678\n").unwrap();
        drop(Passphrase::from_file(&given, 8).unwrap());
        assert!(given.exists());

        let written = Passphrase::write("test", "correct horse").unwrap();
        let path = written.path.clone();
        assert_eq!(fs::read_to_string(&path).unwrap(), "correct horse");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(written);
        assert!(!path.exists());
    }

    #[test]
    fn quoting_survives_any_name() {
        assert_eq!(q(r#"Zoé "Z" O'Neil"#), r#""Zoé \"Z\" O'Neil""#);
    }
}
