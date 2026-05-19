# dnf-generator

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
| `disko` | per-host `disko.nix` | Disk partitioning declarations |
| `doc` | MDX fragments | Module reference for the documentation site |

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
  lib.rs                  Public module re-exports
  error.rs                Error types (thiserror)
  generate.rs             Generate::run() dispatcher
  nix_generator/
    configuration.rs      YAML → Configuration struct (serde)
    item/host.rs          Host model
    item/user.rs          User model
    nix_builder.rs        Nix token tree helpers
    nix_network.rs        Network generation logic
    nix_service.rs        Service generation logic
    nix_zone.rs           Zone generation logic
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
