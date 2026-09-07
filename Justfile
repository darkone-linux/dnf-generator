set shell := ["bash", "-euo", "pipefail", "-c"]

# Default -> just --list
_default:
    @just --list

# Build the generator (release)
build:
    cargo build --release

# Run unit tests
test:
    cargo test --quiet

# Run unit tests with output
test-verbose:
    cargo test -- --nocapture

# Build + test
check: build test

# Run the generator (usage: just run <command> [debug])
run command debug='':
    cargo run -- {{command}} {{debug}}

# Shares the framework's release script and git-cliff config: one changelog
# format and one commit vocabulary across the whole ecosystem. The framework
# pins the tag this produces (`just release`, step 3).

# Bump version, changelog and tag: just bump [auto|patch|minor|major|X.Y.Z]
bump level="auto" *args:
    bash ../../dnf/assets/scripts/just-bump.sh \
        --config ../../dnf/assets/release/cliff.toml --level {{ level }} {{ args }}

# Preview the entry the next release would carry — writes nothing
changelog:
    @git-cliff --config ../../dnf/assets/release/cliff.toml --unreleased --bump 2>/dev/null
