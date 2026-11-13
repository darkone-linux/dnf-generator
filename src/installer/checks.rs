//! Pre-flight checks of `darkone new`, run before the first question: enough
//! RAM, a real Internet access (not a captive portal), a clock TLS accepts.
//!
//! The verdicts are pure functions (exit code of `curl`, body, `/proc/meminfo`
//! → message); the functions that call the system stay thin.

use std::fs;
use std::process::{Command, Stdio};

use super::ask::aborted;
use super::project::log;
use super::Result;

/// MemTotal, in MiB, under which the install is refused. PROVISIONAL: to be
/// confirmed by measuring with `dnf-vm --memory` (default 8192). The image
/// runs from RAM; a 4 GiB machine reports about 3.8 GiB, 3500 lets it pass.
pub const MIN_RAM_MIB: u64 = 3500;

/// The file every binary cache serves: its body starts with `StoreDir:`.
const CACHE_INFO: &str = "nix-cache-info";

/// The cache every install depends on.
const MAIN_CACHE: &str = "https://cache.nixos.org";

/// Failures to show, and what is only worth a warning.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,

    /// An error comes from the network: Wi-Fi may fix it.
    pub network: bool,
}

/// `MemTotal` of `/proc/meminfo` (kB there), in MiB.
fn parse_mem_total_mib(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

/// `None`: enough RAM, or not readable (never blocks on a guess).
fn ram_error(total_mib: Option<u64>) -> Option<String> {
    let total = total_mib?;
    (total < MIN_RAM_MIB).then(|| {
        format!(
            "RAM: {total} MiB found, at least {MIN_RAM_MIB} MiB (a 4 GiB machine) needed: \
             the installer image and the install live in memory"
        )
    })
}

/// What `timedatectl show -p NTPSynchronized --value` says, as a phrase.
fn clock_state(ntp_synchronized: &str) -> &'static str {
    match ntp_synchronized.trim() {
        "yes" => "clock synchronised (NTP)",
        "no" => "clock not synchronised (NTP)",
        _ => "clock state unknown",
    }
}

/// `https://host/path` → `host`.
fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', ':']).next().unwrap_or(rest)
}

/// Verdict of one `curl` probe of `url`: `None` when it served a binary cache.
/// `exit` is the exit code of `curl`, `body` what it printed, `clock` the
/// phrase of [`clock_state`], quoted for a certificate error.
fn probe_error(url: &str, exit: i32, body: &str, clock: &str) -> Option<String> {
    let host = host_of(url);
    Some(match exit {
        0 if body.lines().any(|l| l.starts_with("StoreDir:")) => return None,
        0 => format!(
            "{url} answers, but not as a package cache: a captive portal? Sign in on \
             the network's login page first, or use another network"
        ),
        6 => format!("{host}: name not resolved (DNS): check the network, or its DNS server"),
        7 | 28 => format!("{host} unreachable: check the cable or the Wi-Fi, and the router"),
        60 => format!(
            "{host}: certificate refused, the clock is probably wrong ({clock}): \
             sudo timedatectl set-time 'YYYY-MM-DD HH:MM:SS'"
        ),
        code => format!("{url}: curl failed (exit code {code})"),
    })
}

/// `https://…/nix-cache-info` of a cache base URL; `None` for a scheme `curl`
/// cannot probe (`s3://`, `ssh://`, `file://`), left to the install.
fn cache_info_url(cache: &str) -> Option<String> {
    let probed = cache.starts_with("http://") || cache.starts_with("https://");
    probed.then(|| format!("{}/{CACHE_INFO}", cache.trim_end_matches('/')))
}

/// Failures, all at once, in the way `Plan::resolve` reports them.
fn failure(errors: &[String]) -> String {
    format!(
        "the pre-flight checks failed:\n  - {}",
        errors.join("\n  - ")
    )
}

fn read_ram() -> Option<u64> {
    parse_mem_total_mib(&fs::read_to_string("/proc/meminfo").ok()?)
}

fn read_clock() -> String {
    let out = Command::new("timedatectl")
        .args(["show", "-p", "NTPSynchronized", "--value"])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) => clock_state(&String::from_utf8_lossy(&o.stdout)).to_string(),
        Err(_) => clock_state("").to_string(),
    }
}

/// `curl` exit code and body; `Err` when `curl` does not run at all.
fn curl(url: &str) -> std::result::Result<(i32, String), String> {
    let out = Command::new("curl")
        .args([
            "--silent",
            "--connect-timeout",
            "8",
            "--max-time",
            "20",
            url,
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    let body = String::from_utf8_lossy(&out.stdout).into_owned();

    // No code: curl killed by a signal.
    Ok((out.status.code().unwrap_or(-1), body))
}

/// A probe of `base`, an error message when it fails.
fn probe(base: &str) -> Option<String> {
    let url = cache_info_url(base)?;
    match curl(&url) {
        Ok((exit, body)) => probe_error(&url, exit, &body, &read_clock()),
        Err(e) => Some(e),
    }
}

/// All the checks. The extra cache only ever warns: the install falls back
/// to cache.nixos.org.
pub fn run(extra_cache: Option<&str>) -> Report {
    let mut report = Report::default();
    report.errors.extend(ram_error(read_ram()));
    if let Some(e) = probe(MAIN_CACHE) {
        report.errors.push(e);
        report.network = true;
    }
    if let Some(e) = extra_cache.and_then(probe) {
        report.warnings.push(format!(
            "extra cache: {e}; the install falls back to cache.nixos.org"
        ));
    }
    report
}

/// What the user chose after a failure.
#[derive(Clone, PartialEq, Eq)]
enum Choice {
    Retry,
    Wifi,
    Abort,
}

fn choose(network: bool) -> Result<Choice> {
    let mut select = cliclack::select("The install cannot start").item(
        Choice::Retry,
        "Check again",
        "once the cause is fixed",
    );
    if network {
        select = select.item(Choice::Wifi, "Connect to Wi-Fi", "nmtui");
    }
    select
        .item(Choice::Abort, "Abort", "nothing is changed")
        .interact()
        .map_err(aborted)
}

/// Before the first question. Interactive: failures are shown, then check
/// again / Wi-Fi / abort. Scripted: every failure at once, then an error.
/// `dry_run`: failures are warnings, never blocking.
pub fn preflight(extra_cache: Option<&str>, interactive: bool, dry_run: bool) -> Result<()> {
    loop {
        let spinner = interactive.then(cliclack::spinner);
        if let Some(s) = &spinner {
            s.start("Checking memory, network and clock...");
        }
        let mut report = run(extra_cache);
        match &spinner {
            Some(s) if report.errors.is_empty() => s.stop("Pre-flight checks done"),
            Some(s) => s.error("Pre-flight checks failed"),
            None => {}
        }
        if dry_run {
            report.warnings.append(&mut report.errors);
        }
        for w in &report.warnings {
            if interactive {
                cliclack::log::warning(w).map_err(aborted)?;
            } else {
                log(&format!("warning: {w}"));
            }
        }
        if report.errors.is_empty() {
            return Ok(());
        }
        if !interactive {
            return Err(failure(&report.errors));
        }
        for e in &report.errors {
            cliclack::log::error(e).map_err(aborted)?;
        }
        match choose(report.network)? {
            Choice::Retry => {}

            // Failing or leaving nmtui is not an error: the next check tells.
            Choice::Wifi => {
                Command::new("nmtui").arg("connect").status().ok();
            }
            Choice::Abort => return Err("aborted, nothing changed".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CACHE_BODY: &str = "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 40\n";
    const URL: &str = "https://cache.nixos.org/nix-cache-info";
    const CLOCK: &str = "clock not synchronised (NTP)";

    fn error(exit: i32, body: &str) -> Option<String> {
        probe_error(URL, exit, body, CLOCK)
    }

    #[test]
    fn meminfo_gives_mib() {
        let meminfo = "MemTotal:        8123456 kB\nMemFree:         1000 kB\n";
        assert_eq!(parse_mem_total_mib(meminfo), Some(7933));
        assert_eq!(parse_mem_total_mib("MemFree: 1 kB\n"), None);
        assert_eq!(parse_mem_total_mib("MemTotal: lots kB\n"), None);
        assert_eq!(parse_mem_total_mib(""), None);
    }

    #[test]
    fn ram_threshold() {
        assert!(ram_error(Some(MIN_RAM_MIB - 1)).is_some());
        assert_eq!(ram_error(Some(MIN_RAM_MIB)), None);

        // 4 GiB machine: MemTotal about 3.8 GiB.
        assert_eq!(ram_error(Some(3900)), None);
        assert_eq!(ram_error(Some(8000)), None);

        // Not readable: no verdict.
        assert_eq!(ram_error(None), None);
        assert!(ram_error(Some(1900)).unwrap().contains("1900 MiB"));
    }

    #[test]
    fn a_binary_cache_passes() {
        assert_eq!(error(0, CACHE_BODY), None);
    }

    #[test]
    fn dns_failure() {
        let msg = error(6, "").unwrap();
        assert!(msg.contains("cache.nixos.org") && msg.contains("DNS"));
    }

    #[test]
    fn unreachable_and_timeout_share_a_message() {
        assert!(error(7, "").unwrap().contains("unreachable"));
        assert_eq!(error(7, ""), error(28, ""));
    }

    #[test]
    fn certificate_quotes_the_clock() {
        let msg = error(60, "").unwrap();
        assert!(msg.contains("certificate") && msg.contains(CLOCK));
    }

    #[test]
    fn captive_portal_is_a_success_of_curl_with_a_wrong_body() {
        let html = "<html><body>Sign in to the Wi-Fi</body></html>";
        assert!(error(0, html).unwrap().contains("captive portal"));
        assert!(error(0, "").unwrap().contains("captive portal"));

        // `StoreDir:` must start a line, not hide in a page.
        assert!(error(0, "<p>StoreDir: x</p>").is_some());
    }

    #[test]
    fn other_codes_are_named() {
        let msg = error(35, "").unwrap();
        assert!(msg.contains("exit code 35") && msg.contains(URL));
    }

    #[test]
    fn clock_phrases() {
        assert_eq!(clock_state("yes\n"), "clock synchronised (NTP)");
        assert_eq!(clock_state("no\n"), "clock not synchronised (NTP)");
        assert_eq!(clock_state(""), "clock state unknown");
    }

    #[test]
    fn hosts_of_urls() {
        assert_eq!(
            host_of("https://cache.nixos.org/nix-cache-info"),
            "cache.nixos.org"
        );
        assert_eq!(host_of("http://10.0.0.1:5000"), "10.0.0.1");
        assert_eq!(host_of("cache.example"), "cache.example");
    }

    #[test]
    fn only_http_caches_are_probed() {
        assert_eq!(
            cache_info_url("https://cache.example/"),
            Some("https://cache.example/nix-cache-info".to_string())
        );
        assert_eq!(
            cache_info_url("http://10.0.0.1:5000"),
            Some("http://10.0.0.1:5000/nix-cache-info".to_string())
        );
        assert_eq!(cache_info_url("s3://bucket"), None);
        assert_eq!(cache_info_url("ssh://host"), None);
        assert_eq!(cache_info_url("file:///srv/cache"), None);
    }

    #[test]
    fn scripted_failure_lists_everything() {
        let msg = failure(&["a".to_string(), "b".to_string()]);
        assert_eq!(msg, "the pre-flight checks failed:\n  - a\n  - b");
    }
}
