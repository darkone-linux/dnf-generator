//! Questions of `darkone new`: what the answers file leaves out and the machine
//! cannot tell. Each answer lands in `Answers`, then the same `Plan::resolve`
//! as a scripted install checks the whole.

use std::fs;
use std::io::{self, Write as _};
use std::process::{Command, Stdio};

use regex::Regex;

use super::answers::{
    Answers, ANSWERS_VERSION, RESERVED_LOGINS, RE_EMAIL, RE_FQDN, RE_HOSTNAME, RE_LOGIN, RE_NAME,
};
use super::catalog::Catalog;
use super::machine::{human_size, Machine};
use super::Result;

const MIN_PASSWORD: usize = 8;

/// Ctrl-C or Esc in a prompt.
fn aborted(e: io::Error) -> String {
    if e.kind() == io::ErrorKind::Interrupted {
        "aborted, nothing changed".to_string()
    } else {
        e.to_string()
    }
}

/// Validator of a prompt, from the regex the plan checks too.
fn rule(
    pattern: &str,
    message: &'static str,
) -> impl Fn(&String) -> std::result::Result<(), &'static str> {
    let re = Regex::new(pattern).unwrap();
    move |v: &String| if re.is_match(v) { Ok(()) } else { Err(message) }
}

pub fn complete(answers: &mut Answers, machine: &Machine, catalog: &Catalog) -> Result<()> {
    cliclack::intro("Darkone NixOS Framework: a new network").map_err(aborted)?;
    answers.version.get_or_insert(ANSWERS_VERSION);

    // Keyboard first: everything typed after it, the password above all, must
    // come out the same on the installed system.
    if answers.locale.language.is_none() {
        let mut select = cliclack::select("Language and keyboard")
            .filter_mode()
            .max_rows(10);
        for l in &catalog.locales {
            select = select.item(l.locale.clone(), &l.label, format!("keyboard {}", l.xkb));
        }
        if catalog.locale("en_US.UTF-8").is_some() {
            select = select.initial_value("en_US.UTF-8".to_string());
        }
        answers.locale.language = Some(select.interact().map_err(aborted)?);
    }
    if let Some(entry) = answers
        .locale
        .language
        .as_deref()
        .and_then(|l| catalog.locale(l))
    {
        if load_console_keymap(&entry.console) {
            cliclack::log::remark(format!("Keyboard of this console: {}", entry.xkb)).ok();
        }

        // A single timezone in the country is taken without a question.
        let zones = catalog.timezones(entry.country());
        if answers.locale.timezone.is_none() && zones.len() > 1 {
            let mut select = cliclack::select("Timezone").filter_mode().max_rows(10);
            for z in &zones {
                select = select.item(z.clone(), z, "");
            }
            answers.locale.timezone = Some(select.interact().map_err(aborted)?);
        }
    }

    let admin = &mut answers.admin;
    if admin.name.is_none() {
        let name: String = cliclack::input("Your full name")
            .placeholder("Alice Martin")
            .validate(rule(RE_NAME, "3 to 128 characters"))
            .interact()
            .map_err(aborted)?;
        admin.name = Some(name);
    }
    if admin.login.is_none() {
        let re = Regex::new(RE_LOGIN).unwrap();
        let mut input = cliclack::input("Your login (definitive)").validate(move |v: &String| {
            if RESERVED_LOGINS.contains(&v.as_str()) {
                Err("a system account, choose another one")
            } else if re.is_match(v) {
                Ok(())
            } else {
                Err("a letter, then letters, digits, _ or -")
            }
        });
        if let Some(login) = admin.name.as_deref().and_then(suggest_login) {
            input = input.default_input(&login);
        }
        admin.login = Some(input.interact().map_err(aborted)?);
    }
    if admin.email.is_none() {
        let re = Regex::new(RE_EMAIL).unwrap();
        let email: String = cliclack::input("Your email (optional)")
            .required(false)
            .validate(move |v: &String| {
                if v.is_empty() || re.is_match(v) {
                    Ok(())
                } else {
                    Err("an email address, or nothing")
                }
            })
            .interact()
            .map_err(aborted)?;
        admin.email = (!email.is_empty()).then_some(email);
    }
    if admin.password_hash.is_none() {
        let password = loop {
            let first = cliclack::password("Your password")
                .mask('*')
                .validate(|v: &String| {
                    if v.chars().count() >= MIN_PASSWORD {
                        Ok(())
                    } else {
                        Err("8 characters at least")
                    }
                })
                .interact()
                .map_err(aborted)?;
            let again = cliclack::password("Your password, again")
                .mask('*')
                .interact()
                .map_err(aborted)?;
            if first == again {
                break first;
            }
            cliclack::log::error("The two passwords differ.").ok();
        };
        admin.password_hash = Some(hash_password(&password)?);
    }

    if answers.network.domain.is_none() {
        cliclack::note(
            "Network domain",
            "Chosen for good: Matrix identities, the Kanidm domain and the YubiKey\n\
             origin derive from it. A server reachable from the Internet (HCS)\n\
             needs a domain you own. None of your own: home.arpa, reserved for\n\
             home networks.",
        )
        .map_err(aborted)?;
        let domain: String = cliclack::input("Network domain (definitive)")
            .placeholder("home.arpa")
            .validate(rule(RE_FQDN, "a domain name, like example.org"))
            .interact()
            .map_err(aborted)?;
        answers.network.domain = Some(domain);
    }

    if answers.host.hostname.is_none() {
        let profile = answers
            .host
            .profile
            .clone()
            .unwrap_or_else(|| if machine.battery { "laptop" } else { "desktop" }.to_string());
        let mut input = cliclack::input("Name of this machine")
            .validate(rule(RE_HOSTNAME, "a letter, then letters, digits, _ or -"));
        if let Some(login) = &answers.admin.login {
            input = input.default_input(&format!("{login}-{profile}"));
        }
        answers.host.hostname = Some(input.interact().map_err(aborted)?);
    }

    if answers.disk.device.is_none() && machine.disks.len() > 1 {
        let mut select = cliclack::select("Disk to install on, erased");
        for d in &machine.disks {
            select = select.item(
                d.path.clone(),
                &d.path,
                format!("{} {}", human_size(d.size), d.model),
            );
        }
        answers.disk.device = Some(select.interact().map_err(aborted)?);
    }

    // Encryption comes with the LUKS lot: not asked yet.
    answers.disk.encrypt.get_or_insert(false);
    Ok(())
}

/// The plan of a dry run, which ends the session.
pub fn show(recap: &str) -> Result<()> {
    cliclack::note("Plan", recap).map_err(aborted)?;
    cliclack::outro("Dry run: nothing changed").map_err(aborted)
}

/// The plan, then a confirmation that defaults to no.
pub fn confirm(recap: &str, device: &str) -> Result<()> {
    cliclack::note("Plan", recap).map_err(aborted)?;
    let erase = cliclack::confirm(format!("Erase {device} and install?"))
        .initial_value(false)
        .interact()
        .map_err(aborted)?;
    if erase {
        cliclack::outro("Installing, this takes a while...").map_err(aborted)
    } else {
        cliclack::outro_cancel("Nothing changed").ok();
        Err("aborted, nothing changed".to_string())
    }
}

/// On a virtual console the keyboard is this machine's: switched now, so the
/// password typed next is the one the new system expects. Over ssh, the
/// keyboard is the remote terminal's.
fn load_console_keymap(keymap: &str) -> bool {
    let on_vt = fs::read_link("/proc/self/fd/0").is_ok_and(|p| {
        let p = p.to_string_lossy();
        p.strip_prefix("/dev/tty")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    });
    on_vt
        && Command::new("sudo")
            .args(["loadkeys", keymap])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// yescrypt, as `passwd` does on NixOS. Through stdin: never in argv.
fn hash_password(password: &str) -> Result<String> {
    let mut child = Command::new("mkpasswd")
        .args(["-m", "yescrypt", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run mkpasswd: {e}"))?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(format!("{password}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !hash.is_empty() {
        Ok(hash)
    } else {
        Err("mkpasswd failed to hash the password".to_string())
    }
}

/// `Zoé Martin` → `zoe`: the first name, lowercase, without accents.
fn suggest_login(name: &str) -> Option<String> {
    let login: String = name
        .split_whitespace()
        .next()?
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
            'ç' => Some('c'),
            'è' | 'é' | 'ê' | 'ë' => Some('e'),
            'ì' | 'í' | 'î' | 'ï' => Some('i'),
            'ñ' => Some('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => Some('o'),
            'ù' | 'ú' | 'û' | 'ü' => Some('u'),
            'ý' | 'ÿ' => Some('y'),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect();
    let valid = Regex::new(RE_LOGIN).unwrap().is_match(&login)
        && !RESERVED_LOGINS.contains(&login.as_str());
    valid.then_some(login)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_suggestion_from_the_first_name() {
        assert_eq!(suggest_login("Zoé Martin").as_deref(), Some("zoe"));
        assert_eq!(
            suggest_login("Jean-Noël Dupont").as_deref(),
            Some("jean-noel")
        );
        assert_eq!(suggest_login("Al"), Some("al".to_string()));
        assert_eq!(suggest_login("Ö"), None);
        assert_eq!(suggest_login("Nix"), None);
        assert_eq!(suggest_login("  "), None);
    }
}
