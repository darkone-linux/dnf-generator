//! `darkone add gateway`: the plan computed from a probe, on the project
//! `darkone new` writes, then its `yq` edit of `etc/config.yaml` (real `yq`
//! required). Fixtures under `tests/fixtures/gateway/`.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use dnf_generator::gateway::answers::GatewayAnswers;
use dnf_generator::gateway::plan::{AdminMac, AdminMachine, GatewayPlan};
use dnf_generator::gateway::probe::Probe;
use dnf_generator::gateway::project::{check_edit, load_network, Network};
use dnf_generator::gateway::write::{check_yq, Edit};
use dnf_generator::installer::answers::Plan;
use dnf_generator::installer::project::render_config;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gateway");

fn fixture(name: &str) -> String {
    fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()
}

fn probe(name: &str) -> Probe {
    Probe::parse(&fixture(name)).unwrap()
}

/// The admin host as `darkone new` declares it: zone `main`, no fixed address.
fn new_plan(macs: &[&str]) -> Plan {
    Plan {
        login: "alice".to_string(),
        name: "Alice Martin".to_string(),
        email: None,
        password_hash: "$y$j9T$salt$hash".to_string(),
        domain: "home.arpa".to_string(),
        hostname: "poste".to_string(),
        profile: "desktop".to_string(),
        ip_prefix: "10.1".to_string(),
        macs: macs.iter().map(|m| m.to_string()).collect(),
        device: "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001".to_string(),
        device_label: "512 GB EXAMPLE SSD".to_string(),
        disko_profile: "btrfs-1-disk".to_string(),
        encrypt: false,
        locale: None,
        keyboard: None,
        timezone: None,
        backup_device: None,
        cache: None,
        cache_key: None,
    }
}

/// A project on disk, with the framework files the generator reads.
fn project(config: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("dnf/home/profiles/nix-admin")).unwrap();
    fs::create_dir_all(root.join("dnf/config")).unwrap();
    fs::write(root.join("dnf/config/modules.nix"), "{ }\n").unwrap();
    fs::create_dir_all(root.join("dnf/hosts/disko")).unwrap();
    fs::write(root.join("dnf/hosts/disko/btrfs-1-disk.nix"), "{ }\n").unwrap();
    fs::write(root.join("dnf/hosts/disko/server-1-disk.nix"), "{ }\n").unwrap();
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/config.yaml"), config).unwrap();
    dir
}

fn network(config: &str) -> Network {
    load_network(project(config).path()).unwrap()
}

fn installer_network() -> Network {
    network(&render_config(&new_plan(&[])))
}

fn admin() -> AdminMachine {
    AdminMachine {
        hostname: "poste".to_string(),
        macs: vec!["aa:bb:cc:00:00:01".to_string()],
    }
}

fn answers(yaml: &str) -> GatewayAnswers {
    serde_yaml::from_str(yaml).unwrap()
}

fn nominal() -> GatewayAnswers {
    answers(&fixture("answers.yaml"))
}

fn resolve(a: &GatewayAnswers, probe_file: &str) -> Result<GatewayPlan, Vec<String>> {
    GatewayPlan::resolve(a, &probe(probe_file), &installer_network(), &admin(), None)
}

fn has(errors: &[String], fragment: &str) -> bool {
    errors.iter().any(|e| e.contains(fragment))
}

#[test]
fn installer_project_is_the_zone_without_gateway() {
    let n = installer_network();
    assert_eq!((n.zone.as_str(), n.ip_prefix.as_str()), ("main", "10.1"));
    let poste = n.host("poste").unwrap();
    assert_eq!((poste.ip.as_deref(), poste.mac.as_deref()), (None, None));

    let with_mac = network(&render_config(&new_plan(&["aa:bb:cc:00:00:09"])));
    assert_eq!(
        with_mac.host("poste").unwrap().mac.as_deref(),
        Some("aa:bb:cc:00:00:09")
    );
}

#[test]
fn nominal_two_cards() {
    let plan = resolve(&nominal(), "probe_two_nics.json").unwrap();
    assert_eq!(plan.zone, "main");
    assert_eq!(plan.hostname, "gw");
    assert_eq!(plan.zone_spec, "main:1.1");
    assert_eq!(plan.lan_ip, "10.1.1.1");
    assert_eq!(plan.target.to_string(), "192.168.1.50");
    assert_eq!(plan.wan.name, "enp1s0");
    assert!(plan.wan.route);
    assert_eq!(
        plan.lan.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
        ["enp2s0"]
    );
    assert_eq!(plan.disk.path, "/dev/disk/by-id/ata-EXAMPLE_SSD_0001");
    assert!(plan.disk.stable);
    assert_eq!(plan.disko_profile, "server-1-disk");
    assert_eq!(plan.admin.hostname, "poste");
    assert_eq!(plan.admin.zone_spec, "main:2.1");
    assert_eq!(plan.admin.ip, "10.1.2.1");
    assert!(plan.admin.new_address);
    assert_eq!(
        plan.admin.mac,
        AdminMac::Add("aa:bb:cc:00:00:01".to_string())
    );
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);

    let recap = plan.recap();
    for fragment in [
        "main, 10.1.0.0/16, definitive",
        "gw, 10.1.1.1 (main:1.1)",
        "enp1s0 (aa:bb:cc:00:10:01), to the box, default route",
        "enp2s0: bridged",
        "ata-EXAMPLE_SSD_0001 (256 GB EXAMPLE SSD), ERASED",
        "server-1-disk",
        "poste: fixed address 10.1.2.1 (main:2.1), new, MAC aa:bb:cc:00:00:01, added",
    ] {
        assert!(recap.contains(fragment), "{fragment} not in\n{recap}");
    }
}

#[test]
fn detection_fills_name_wan_and_single_disk() {
    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  lan: [enp2s0]\n");
    let plan = resolve(&a, "probe_two_nics.json").unwrap();
    assert_eq!(plan.hostname, "gw");
    assert_eq!(plan.wan.name, "enp1s0");

    // The 8 GB stick is no install target: one disk left, no question.
    assert_eq!(plan.disk.dev, "/dev/sda");
}

#[test]
fn lan_is_confirmed_never_guessed() {
    let a = answers("version: 1\naddress: 192.168.1.50\n");
    let errs = resolve(&a, "probe_two_nics.json").unwrap_err();
    assert!(
        has(&errs, "missing answer: nics.lan, proposed: enp2s0"),
        "{errs:?}"
    );
}

#[test]
fn one_wired_card_allows_no_lan() {
    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  wan: enp0s2\n  lan: [wlp3s0]\n");
    let errs = resolve(&a, "probe_one_nic.json").unwrap_err();
    assert!(
        has(
            &errs,
            "one wired card only, enp0s2 (52:54:00:12:34:56, link)"
        ),
        "{errs:?}"
    );
    assert!(has(&errs, "a gateway needs two"), "{errs:?}");
}

#[test]
fn no_route_asks_for_the_wan() {
    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  lan: [enp2s0]\n");
    let errs = resolve(&a, "probe_no_route.json").unwrap_err();
    assert!(
        has(
            &errs,
            "missing answer: nics.wan, no wired card carries the default route"
        ),
        "{errs:?}"
    );

    let a = answers(
        "version: 1\naddress: 192.168.1.50\nnics:\n  wan: enp1s0\n  lan: [enp2s0, enp3s0]\n",
    );
    let plan = resolve(&a, "probe_no_route.json").unwrap();
    assert_eq!(plan.wan.name, "enp1s0");
    assert_eq!(plan.lan.len(), 2);
    assert!(
        has(&plan.warnings, "no card carries the default route"),
        "{:?}",
        plan.warnings
    );
}

#[test]
fn two_disks_need_an_answer() {
    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  lan: [enp2s0]\n");
    let errs = resolve(&a, "probe_two_disks.json").unwrap_err();
    assert!(
        has(&errs, "missing answer: disk.device, one of /dev/disk/by-id/nvme-EXAMPLE_NVME_0001 (512 GB EXAMPLE NVME), /dev/disk/by-id/ata-EXAMPLE_HDD_0002 (2.0 TB EXAMPLE HDD)"),
        "{errs:?}"
    );

    // The kernel name selects the stable path.
    let a = answers(
        "version: 1\naddress: 192.168.1.50\nnics:\n  lan: [enp2s0]\ndisk:\n  device: /dev/sda\n",
    );
    let plan = resolve(&a, "probe_two_disks.json").unwrap();
    assert_eq!(plan.disk.path, "/dev/disk/by-id/ata-EXAMPLE_HDD_0002");
}

#[test]
fn disk_without_by_id_is_flagged() {
    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  lan: [ens4]\n");
    let plan = resolve(&a, "probe_no_by_id.json").unwrap();
    assert_eq!(plan.disk.path, "/dev/vda");
    assert!(!plan.disk.stable);
    assert!(
        has(&plan.warnings, "/dev/vda has no /dev/disk/by-id name"),
        "{:?}",
        plan.warnings
    );
    assert!(plan.recap().contains("Warning    /dev/vda has no"));
}

#[test]
fn wrong_cards_and_disks_are_refused_at_once() {
    let a = answers(
        "version: 1\naddress: 192.168.1.50\nhost:\n  hostname: poste\n\
         nics:\n  wan: eth9\n  lan: [enp2s0]\ndisk:\n  device: /dev/sdb\n",
    );
    let errs = resolve(&a, "probe_two_nics.json").unwrap_err();
    for fragment in [
        "'poste' is already a host",
        "nics.wan: no card 'eth9'",
        "disk.device: '/dev/sdb' holds 8 GB, 20 GB at least",
    ] {
        assert!(has(&errs, fragment), "{fragment} not in {errs:?}");
    }

    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  lan: [enp1s0, enp2s0, enp2s0]\n");
    let errs = resolve(&a, "probe_two_nics.json").unwrap_err();
    assert!(has(&errs, "'enp1s0' is the WAN"), "{errs:?}");
    assert!(has(&errs, "'enp2s0' given twice"), "{errs:?}");
}

#[test]
fn every_missing_answer_is_reported_at_once() {
    let errs = resolve(&GatewayAnswers::default(), "probe_two_nics.json").unwrap_err();
    for key in ["version", "address", "nics.lan"] {
        assert!(
            has(&errs, &format!("missing answer: {key}")),
            "{key} not reported in {errs:?}"
        );
    }
}

/// First case: box on 10.1.0.0/24, the zone prefix of `darkone new`.
#[test]
fn box_inside_the_zone_is_refused_before_writing() {
    let json = fixture("probe_two_nics.json").replace("192.168.1.50/24", "10.1.0.50/24");
    let p = Probe::parse(&json).unwrap();
    let a = answers("version: 1\naddress: 10.1.0.50\nnics:\n  lan: [enp2s0]\n");
    let errs = GatewayPlan::resolve(&a, &p, &installer_network(), &admin(), None).unwrap_err();
    assert!(
        has(
            &errs,
            "the box network (10.1.0.0/24 on enp1s0) overlaps zone 'main' (10.1.0.0/16)"
        ),
        "{errs:?}"
    );
    assert!(has(&errs, "The zone prefix is final"), "{errs:?}");

    // A box /8 holds the zone even when the address does not.
    let json = fixture("probe_two_nics.json").replace("192.168.1.50/24", "10.200.0.50/8");
    let p = Probe::parse(&json).unwrap();
    let a = answers("version: 1\naddress: 10.200.0.50\nnics:\n  lan: [enp2s0]\n");
    let errs = GatewayPlan::resolve(&a, &p, &installer_network(), &admin(), None).unwrap_err();
    assert!(has(&errs, "10.0.0.0/8 on enp1s0"), "{errs:?}");

    // No address on the WAN card: the address alone is judged, as `install` does.
    let json = fixture("probe_two_nics.json").replace("\"192.168.1.50/24\"", "");
    let p = Probe::parse(&json).unwrap();
    let a = answers("version: 1\naddress: 10.1.3.200\nnics:\n  lan: [enp2s0]\n");
    let errs = GatewayPlan::resolve(&a, &p, &installer_network(), &admin(), None).unwrap_err();
    assert!(
        has(&errs, "the box network (address 10.1.3.200)"),
        "{errs:?}"
    );
}

#[test]
fn bios_machine_is_refused() {
    let json = fixture("probe_two_nics.json").replace("\"uefi\": true", "\"uefi\": false");
    let errs = GatewayPlan::resolve(
        &nominal(),
        &Probe::parse(&json).unwrap(),
        &installer_network(),
        &admin(),
        None,
    )
    .unwrap_err();
    assert!(
        has(&errs, "BIOS mode, layout server-1-disk needs UEFI"),
        "{errs:?}"
    );
}

#[test]
fn declared_mac_and_fixed_address_are_kept() {
    let config = render_config(&new_plan(&["aa:bb:cc:00:00:09"]))
        .replace("zone: \"main\"", "zone: \"main:4.2\"");
    let plan = GatewayPlan::resolve(
        &nominal(),
        &probe("probe_two_nics.json"),
        &network(&config),
        &admin(),
        Some("ssh-ed25519 AAAAexample".to_string()),
    )
    .unwrap();
    assert_eq!(plan.admin.zone_spec, "main:4.2");
    assert_eq!(plan.admin.ip, "10.1.4.2");
    assert!(!plan.admin.new_address);
    assert_eq!(
        plan.admin.mac,
        AdminMac::Declared("aa:bb:cc:00:00:09".to_string())
    );
    assert_eq!(plan.host_key.as_deref(), Some("ssh-ed25519 AAAAexample"));
}

#[test]
fn zone_with_a_gateway_is_refused() {
    let base = render_config(&new_plan(&[]));

    // A host at `<zone>:1.1` is the gateway, by the generator's convention.
    let with_host = format!(
        "{base}  - hostname: \"gw\"\n    name: \"Gateway\"\n    zone: \"main:1.1\"\n    profile: \"gateway\"\n"
    );
    let dir = project(&with_host);
    let err = load_network(dir.path()).unwrap_err();
    assert!(err.contains("already has a gateway, gw"), "{err}");

    // A gateway block without its host: half done by hand.
    let with_block = base.replace(
        "    ipPrefix: \"10.1\"\n",
        "    ipPrefix: \"10.1\"\n    gateway:\n      wan:\n        interface: \"enp1s0\"\n      \
         lan:\n        interfaces: [\"enp2s0\"]\n",
    );
    let err = load_network(project(&with_block).path()).unwrap_err();
    assert!(err.contains("declares zones.main.gateway"), "{err}");
}

#[test]
fn several_zones_are_out_of_scope() {
    let config = render_config(&new_plan(&[]))
        .replace("zones:\n", "zones:\n  annex:\n    ipPrefix: \"10.2\"\n");
    let err = load_network(project(&config).path()).unwrap_err();
    assert!(err.contains("several zones (annex, main)"), "{err}");
}

#[test]
fn project_needs_the_linked_framework() {
    let dir = project(&render_config(&new_plan(&[])));
    fs::remove_file(dir.path().join("dnf/config/modules.nix")).unwrap();
    let err = load_network(dir.path()).unwrap_err();
    assert!(err.contains("is dnf/ linked"), "{err}");
}

// ─── etc/config.yaml edited by yq (spec 2b.5, step 5) ───────────────────────

/// These tests run the real `yq`: without it they fail, never pass unseen.
fn require_yq() {
    if let Err(e) = check_yq() {
        panic!("the yq tests need yq-go in PATH (nix develop, framework dev shell): {e}");
    }
}

fn plan_on(config: &str) -> GatewayPlan {
    GatewayPlan::resolve(
        &nominal(),
        &probe("probe_two_nics.json"),
        &network(config),
        &admin(),
        None,
    )
    .unwrap()
}

/// The edit of `config` for the nominal plan, read back by the generator.
fn edited(config: &str) -> (GatewayPlan, String) {
    require_yq();
    let plan = plan_on(config);
    let out = Edit::from_plan(&plan).unwrap().apply(config).unwrap();
    check_edit(project(config).path(), &out, &plan).unwrap();
    (plan, out)
}

fn blank_lines(text: &str) -> usize {
    text.lines().filter(|l| l.trim().is_empty()).count()
}

/// `kept` in `text`, in the same order: lines were only added around them.
fn in_order(kept: &[&str], text: &str) -> bool {
    let mut lines = text.lines();
    kept.iter().all(|k| lines.any(|l| l == *k))
}

#[test]
fn yq_edit_of_the_installer_config_only_adds() {
    let config = render_config(&new_plan(&[]));
    let (_, out) = edited(&config);

    // One line changes, the zone of the admin host: every other one stays.
    let kept: Vec<&str> = config
        .lines()
        .filter(|l| *l != "    zone: \"main\"")
        .collect();
    assert!(in_order(&kept, &out), "{out}");
    assert_eq!(blank_lines(&out), blank_lines(&config) + 1, "{out}");
    for fragment in [
        "    ipPrefix: \"10.1\"\n    gateway:\n      wan:\n        interface: \"enp1s0\"\n      \
         lan:\n        interfaces: [\"enp2s0\"]\n\nusers:\n",
        "  - hostname: \"poste\"\n    name: \"Admin workstation\"\n    zone: \"main:2.1\"\n",
        "    mac: \"aa:bb:cc:00:00:01\"\n\n  - hostname: \"gw\"\n    name: \"Gateway\"\n    \
         zone: \"main:1.1\"\n    profile: \"gateway\"\n    disko:\n      profile: \"server-1-disk\"\n      \
         devices:\n        main: \"/dev/disk/by-id/ata-EXAMPLE_SSD_0001\"\n",
    ] {
        assert!(out.contains(fragment), "{fragment}\nnot in\n{out}");
    }
    assert!(out.starts_with("# yaml-language-server: $schema="), "{out}");

    // The generator must read the plan back: the unedited file does not.
    let plan = plan_on(&config);
    let err = check_edit(project(&config).path(), &config, &plan).unwrap_err();
    assert!(err.contains("nothing written"), "{err}");
    assert!(err.contains("gateway of the zone: None"), "{err}");

    // Written, the project is the one `darkone add gateway` refuses.
    let err = load_network(project(&out).path()).unwrap_err();
    assert!(err.contains("already has a gateway, gw"), "{err}");
}

#[test]
fn yq_edit_leaves_a_declared_mac_and_address_alone() {
    let config = render_config(&new_plan(&["aa:bb:cc:00:00:09"]))
        .replace("zone: \"main\"", "zone: \"main:4.2\"");
    let (plan, out) = edited(&config);
    assert!(!plan.admin.new_address);
    assert!(out.contains("    zone: \"main:4.2\"\n"), "{out}");
    assert_eq!(out.matches("mac:").count(), 1, "{out}");
    assert!(out.contains("    mac: \"aa:bb:cc:00:00:09\"\n"), "{out}");

    // Nothing but additions this time.
    let kept: Vec<&str> = config.lines().collect();
    assert!(in_order(&kept, &out), "{out}");
}

#[test]
fn yq_edit_keeps_hand_written_comments_and_blank_lines() {
    let config = fixture("config_commented.yaml");
    let (_, out) = edited(&config);
    for line in config.lines().filter(|l| l.contains('#')) {
        let comment = line[line.find('#').unwrap()..].trim();
        assert!(out.contains(comment), "comment lost: {comment}\n{out}");
    }
    assert!(blank_lines(&out) >= blank_lines(&config), "{out}");
    for fragment in [
        "    zone: main:2.1 # DHCP of the box for now\n",
        "    gateway:\n      wan:\n        interface: \"enp1s0\"\n",
        "  # A second machine, on a lease\n  - hostname: laptop\n",
        "  - hostname: \"gw\"\n",
        "# - hostname: printer\n",
    ] {
        assert!(out.contains(fragment), "{fragment}\nnot in\n{out}");
    }
}

/// A marker comment inside a block scalar would end it: blank lines are
/// dropped for that file, its values kept.
#[test]
fn yq_edit_survives_a_block_scalar_with_a_blank_line() {
    let config = render_config(&new_plan(&[])).replace(
        "    description: \"Main network\"\n",
        "    description: |\n      Main network\n\n      second paragraph\n",
    );
    let (_, out) = edited(&config);
    let doc: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
    assert_eq!(
        doc["zones"]["main"]["description"].as_str(),
        Some("Main network\n\nsecond paragraph\n")
    );
}

/// `strenv()` alone, the name checks left out: whatever a probed machine
/// answers lands as a string, never as an expression or as YAML.
#[test]
fn yq_takes_hostile_values_as_plain_strings() {
    require_yq();
    let config = render_config(&new_plan(&[]));
    let hostile = [
        "eth0\"; .hosts = [] #",
        "$(touch /tmp/pwned) `id` ${HOME}",
        "x: {y: [1, 2]}\n- z",
        "strenv(HOME) | env(PATH)",
        "' || true",
    ];
    for value in hostile {
        let mut edit = Edit::from_plan(&plan_on(&config)).unwrap();
        edit.wan = value.to_string();
        edit.disk = value.to_string();

        // A comma separates the cards: that case is below.
        let card = value.replace(',', ";");
        edit.lan = vec![card.clone()];
        let out = edit
            .apply(&config)
            .unwrap_or_else(|e| panic!("{value:?}: {e}"));
        let doc: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        let gateway = &doc["zones"]["main"]["gateway"];
        assert_eq!(gateway["wan"]["interface"].as_str(), Some(value));
        assert_eq!(
            gateway["lan"]["interfaces"][0].as_str(),
            Some(card.as_str())
        );
        let hosts = doc["hosts"].as_sequence().unwrap();
        assert_eq!(hosts.len(), 2, "{value:?}");
        assert_eq!(hosts[1]["disko"]["devices"]["main"].as_str(), Some(value));
    }

    // A comma would split one card in two: not the planned document, refused.
    let mut edit = Edit::from_plan(&plan_on(&config)).unwrap();
    edit.lan = vec!["enp2s0,enp3s0".to_string()];
    let err = edit.apply(&config).unwrap_err();
    assert!(err.contains("nothing written"), "{err}");
}

#[test]
fn names_from_the_probe_are_checked_before_yq() {
    let config = render_config(&new_plan(&[]));
    let mut plan = plan_on(&config);
    plan.wan.name = "eth0\"; .hosts = []".to_string();
    plan.disk.path = "/dev/$(id)".to_string();
    let err = Edit::from_plan(&plan).unwrap_err();
    assert!(err.contains("unexpected characters"), "{err}");
    assert!(
        err.contains("card name") && err.contains("disk path"),
        "{err}"
    );

    // The plan itself refuses them, a dry run included.
    let json = fixture("probe_two_nics.json").replace("enp2s0", "enp2s0;reboot");
    let a = answers("version: 1\naddress: 192.168.1.50\nnics:\n  lan: [\"enp2s0;reboot\"]\n");
    let errs = GatewayPlan::resolve(
        &a,
        &Probe::parse(&json).unwrap(),
        &installer_network(),
        &admin(),
        None,
    )
    .unwrap_err();
    assert!(
        has(&errs, "card name with unexpected characters"),
        "{errs:?}"
    );
}
