# Changelog

All notable changes to generator are documented here.  
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]


## [0.4.0] - 2026-11-13

### Added

- **doc**: One reference page per module, sub-module options on its parent
- **installer**: Propose reboot after the install
- **installer**: Accept home.arpa as the default network domain
- **installer**: Pre-flight checks of RAM, network and clock
- **installer**: Zone prefix chosen outside the connected networks
- **installer**: Record the MACs of the physical cards in config.yaml
- **installer**: Resume an interrupted darkone new, or start from scratch
- **doc**: Category icon before the module page title

### Fixed

- **registry**: Refuse a missing or non-literal modules.nix instead of defaults
- **doc**: Leave code spans literal when escaping MDX text
- **installer**: Pre-flight spinner says failed when a check fails
- **installer**: RAM threshold 5500 MiB, a 4 GiB machine thrashes

### Removed

- **installer**: Drop the package cache question from darkone new

### Documentation

- **dev**: Drop pointers to unpublished agent files and stale paths
- **comments**: Drop pointers to private specs from published code

## [0.3.2] - 2026-10-24

### Added

- **services**: Reject a domain reusing a host or alias name

### Fixed

- **dns**: Host records lead, FQDN first, so a PTR answers the host itself

## [0.3.1] - 2026-10-19

### Added

- **installer**: Backup step can be skipped at the admin's own risk
- **installer**: Project README in the language of the admin
- **installer**: Optional extra package cache for the install
- **installer**: DNF_URL and DNF_CACHE defaults, cache question reworded

### Fixed

- **installer**: Project repository on branch main, not master

## [0.3.0] - 2026-10-17

### Added

- **schema**: JSON Schema of config.yaml via dnf-generator schema
- **installer**: Darkone new, first admin host from an answers file
- **installer**: Darkone new asks what the answers file leaves out
- **installer**: Darkone new encrypts the disk, passphrase asked or from file
- **installer**: Darkone new backs up the admin keys on a USB stick

### Fixed

- **network**: Always emit coordination.enable, false when left out
- **installer**: USB stick hint without a stray comma when its model is empty
- **build**: Cargo run picks dnf-generator again, not ambiguous with darkone

## [0.2.1] - 2026-10-05

### Fixed

- **dns**: Undeclared zone names no longer resolve to the gateway

## [0.2.0] - 2026-09-24

### ⚠ Breaking

- **machines**: Split usr/machines by provenance

## [0.1.1] - 2026-09-23

### Added

- **network**: Add fleetUpdate deployment defaults
- **network**: FleetUpdate timeouts and pingInterval
- **network**: Accept the publish timeout key of fleet-update
- **network**: Warn on a zone without harmonia
- **engine**: Read the fleet AI model from the generator
- **release**: Idempotent ordered release train

## [0.1.0] - 2026-09-09

### Added

- **gen**: Services options from modules.nix
- **doc**: Short (title) for categories
- **config.yaml**: Network.matrix new key check
- **modules**: Same-host service 'require' dependency validation
- **config.yaml**: Strict typed schema validation
- **config.yaml**: Whitelist host arch (cpu:board syntax)
- **aarch64**: Added rpi02, rpi3
- **network**: Expose zone services externally via HCS external-hosts
- **network**: Roaming reservations for out-of-zone hosts
- **generate**: Seed var/generated and per-user overlays on fresh projects
- **install**: Survive first reboot (2222, address probe, no suspend)
- **matrix**: Mandatory MAS and declarative network.matrix.admins

### Fixed

- **parser**: Services option parser improvement
- **gen**: Add locale prefix to internal module links
- **doc-gen**: Overview page generation
- **aarch64**: Deactivation
- **mdx**: Compact submodule type labels in generated reference
- **mdx**: Compact submodule type labels in generated reference
- **zones**: Overlay zones.common defaults on every declared zone
- **mdx**: Escape angle brackets in Nix comments to prevent JSX breakage

### Security

- **secrets**: Drop network.default.password-hash from the schema

### Changed

- **modules**: JSON registry (not the good solution)

### Documentation

- **gen**: Separated files for modules

[Unreleased]: https://github.com/darkone-linux/dnf-generator/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/darkone-linux/dnf-generator/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/darkone-linux/dnf-generator/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/darkone-linux/dnf-generator/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/darkone-linux/dnf-generator/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/darkone-linux/dnf-generator/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/darkone-linux/dnf-generator/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/darkone-linux/dnf-generator/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/darkone-linux/dnf-generator/releases/tag/v0.1.0
