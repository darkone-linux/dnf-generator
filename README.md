# dnf-generator

[![Cargo Tests](https://github.com/darkone-linux/dnf-generator/actions/workflows/cargo-tests.yml/badge.svg)](https://github.com/darkone-linux/dnf-generator/actions/workflows/cargo-tests.yml)
[![Rust Edition 2021](https://img.shields.io/badge/Rust-edition%202021-CE412B?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

Configuration generator for the
[Darkone NixOS Framework](https://github.com/darkone-linux/darkone-nixos-framework).

Reads `etc/config.yaml` and writes deterministic Nix files under
`var/generated/`.

## Commands

| Command | Output file | Description |
|---------|-------------|-------------|
| `hosts` | `var/generated/hosts.nix` | Host list with profiles, services, colmena tags |
| `users` | `var/generated/users.nix` | User list with profiles and group memberships |
| `network` | `var/generated/network.nix` | Network topology (zones, DNS, VPN) |
| `machines` | `usr/machines/<host>/install/disko.nix`, `var/generated/hosts/<host>.nix` | Disk layout (profile + devices, frozen at install), boot flags it implies |
| `doc` | MDX fragments | Module reference for the documentation site |
| `schema` | stdout | JSON Schema of `etc/config.yaml` (editor completion and validation), no workdir needed |

## Usage

```sh
dnf-generator <command> [--workdir <path>] [--debug]
```

`--workdir` defaults to the current directory. The binary expects
`<workdir>/etc/config.yaml` to exist.

In practice the generator is invoked through the project Justfile:

```sh
QUIET=1 just generate    # from the arthur-network root
```

## darkone

Second binary of the crate: the DNF assistant. `darkone new` installs the
machine booted on the DNF installer image as the first host of a new network,
its admin host. It resolves an answers file (`--answers`) against what it
detects (UEFI, Secure Boot, battery, disks), shows the plan, writes the project
from the framework template, then runs `just install-here`.

```sh
darkone new --answers answers.yaml [--non-interactive] [--yes] [--dry-run]
```

`--dry-run` shows the plan and runs anywhere; anything else requires the
installer image (`/etc/dnf-installer`). Questions are not implemented yet: every
answer comes from the file.

## Development

```sh
just build          # cargo build --release
just test           # cargo test --quiet
just test-verbose   # cargo test -- --nocapture
just check          # build + test
just run <command>  # cargo run -- <command>
```

## Architecture

```
src/
  main.rs                 CLI entry point
  bin/darkone.rs          Assistant CLI (clap): `darkone new`
  lib.rs                  Public module re-exports
  error.rs                Error types (thiserror)
  generate.rs             Generate::run() dispatcher
  installer/
    answers.rs            Answers file, install plan and its checks
    machine.rs            Detection: firmware, battery, install disks
    project.rs            Project from the template, config.yaml, install-here
  nix_generator/
    configuration.rs      YAML → Configuration struct (serde)
    item/host.rs          Host model
    item/user.rs          User model
    nix_builder.rs        Nix token tree helpers
    nix_network.rs        Network generation logic
    nix_service.rs        Service generation logic
    nix_zone.rs           Zone generation logic
    schema.rs             Typed config.yaml schema, JSON Schema derivation
    validation.rs         Configuration validation
    yaml.rs               YAML loading helpers
    token/                Nix token types (attr set, list, value)
  mdx_generator/
    generator.rs          MDX doc generation entry point
    mdx_writer.rs         MDX output helpers
    module.rs             Module descriptor
    nix_parser.rs         rnix-based Nix source parser
```

Output files are formatted with `nixfmt -sv` and are fully deterministic —
diffs between runs reflect only actual configuration changes.
