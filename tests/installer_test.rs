//! `darkone new` writes an `etc/config.yaml` the generator must accept as is.

use std::fs;

use tempfile::TempDir;

use dnf_generator::generate::Generate;
use dnf_generator::installer::answers::Plan;
use dnf_generator::installer::project::render_config;
use dnf_generator::nix_generator::nix_service::ServiceRegistry;

fn plan() -> Plan {
    Plan {
        login: "alice".to_string(),
        name: "Alice O'Neil".to_string(),
        email: Some("alice@example.org".to_string()),
        password_hash: "$y$j9T$salt$hash".to_string(),
        domain: "example.org".to_string(),
        hostname: "poste".to_string(),
        profile: "laptop".to_string(),
        device: "/dev/disk/by-id/nvme-EXAMPLE_SSD_0001".to_string(),
        device_label: "512 GB EXAMPLE SSD".to_string(),
        disko_profile: "btrfs-1-disk".to_string(),
        encrypt: false,
        locale: Some("fr_FR.UTF-8".to_string()),
        keyboard: Some("fr".to_string()),
        timezone: Some("Europe/Paris".to_string()),
    }
}

/// Framework files the generator reads on disk: user profile, disko profile,
/// machine template.
fn generate(config: &str) -> (TempDir, Generate) {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("dnf/home/profiles/nix-admin")).unwrap();
    fs::create_dir_all(root.join("dnf/hosts/disko")).unwrap();
    fs::write(
        root.join("dnf/hosts/disko/btrfs-1-disk.nix"),
        "{ disko.devices.disk.main.device = \"@DEVICE:main@\"; }\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("dnf/hosts/templates")).unwrap();
    fs::write(
        root.join("dnf/hosts/templates/usr-machines-configuration.nix"),
        "{ ... }:\n{\n}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/config.yaml"), config).unwrap();
    let registry = ServiceRegistry::from_nix("{ }").unwrap();
    let generate = Generate::new(
        &root.join("etc/config.yaml"),
        &root.join("var/generated/config.yaml"),
        registry,
    )
    .expect("the installer config must load");
    (dir, generate)
}

#[test]
fn installer_config_is_accepted_by_the_generator() {
    let (dir, generate) = generate(&render_config(&plan()));
    let hosts = generate.generate_hosts_raw().unwrap();
    assert!(hosts.contains(r#"hostname = "poste";"#), "{hosts}");
    assert!(hosts.contains(r#"profile = "laptop";"#), "{hosts}");
    let users = generate.generate_users_raw().unwrap();
    assert!(users.contains("uid = 1000;"), "{users}");
    assert!(users.contains(r#"name = "Alice O'Neil";"#), "{users}");
    let network = generate.generate_network_raw().unwrap();
    assert!(network.contains(r#"domain = "example.org";"#), "{network}");
    assert!(network.contains(r#""fr_FR.UTF-8""#), "{network}");

    generate.run("machines").unwrap();
    let disko =
        fs::read_to_string(dir.path().join("usr/machines/poste/install/disko.nix")).unwrap();
    assert!(
        disko.contains("/dev/disk/by-id/nvme-EXAMPLE_SSD_0001"),
        "{disko}"
    );
}

#[test]
fn installer_config_without_locale_or_email() {
    let mut plan = plan();
    plan.email = None;
    plan.locale = None;
    plan.keyboard = None;
    plan.timezone = None;
    let config = render_config(&plan);
    assert!(!config.contains("default:"), "{config}");
    let (_dir, generate) = generate(&config);
    assert!(generate.generate_users_raw().unwrap().contains("alice"));
}
