# Changelog

All notable changes to generator are documented here.  
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]


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

[Unreleased]: https://github.com/darkone-linux/dnf-generator/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/darkone-linux/dnf-generator/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/darkone-linux/dnf-generator/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/darkone-linux/dnf-generator/releases/tag/v0.1.0
