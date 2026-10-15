//! `darkone new`: install the first host of a DNF network, its admin host, on
//! the machine booted on the DNF installer image.
//!
//! - `answers`: the answers file and the plan resolved from it;
//! - `ask`: the questions for what the answers file leaves out;
//! - `catalog`: the locales and timezones the installer offers;
//! - `machine`: what is detected instead of asked (firmware, battery, disks);
//! - `project`: the project written in RAM, then installed by
//!   `just install-here`, which moves it to /etc/nixos of the new system.

pub mod answers;
pub mod ask;
pub mod catalog;
pub mod machine;
pub mod project;

/// Present on the DNF installer image only (`dnf/hosts/iso.nix`): nothing that
/// erases a disk runs without it.
pub const INSTALLER_MARKER: &str = "/etc/dnf-installer";

pub type Result<T> = std::result::Result<T, String>;
