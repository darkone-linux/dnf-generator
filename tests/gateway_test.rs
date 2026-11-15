//! `darkone add gateway`: the plan computed from a probe, on the project
//! `darkone new` writes. Fixtures under `tests/fixtures/gateway/`.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use dnf_generator::gateway::answers::GatewayAnswers;
use dnf_generator::gateway::plan::{AdminMac, AdminMachine, GatewayPlan};
use dnf_generator::gateway::probe::Probe;
use dnf_generator::gateway::project::{load_network, Network};
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
