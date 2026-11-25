set shell := ["bash", "-euo", "pipefail", "-c"]

# Shared with the framework, dnf-fleet-update and dnf-doc: one changelog
# format, one commit vocabulary. Needs the co-development workspace (`../../dnf`).
dnfScripts := justfile_directory() / "../../dnf/just/scripts"

# Default -> just --list
_default:
    @just --list

#------------------------------------------------------------------------------
# Development
#------------------------------------------------------------------------------

# Build the generator (release)
[group('dev')]
build:
    cargo build --release

# Run the generator (usage: just run <command> [debug])
[group('dev')]
run command debug='':
    cargo run -- {{ command }} {{ debug }}

#------------------------------------------------------------------------------
# Quality
#------------------------------------------------------------------------------

# Run unit tests
[group('quality')]
test:
    cargo test --quiet

# Run unit tests with output
[group('quality')]
test-verbose:
    cargo test -- --nocapture

# Build + test
[group('quality')]
check: build test

# `--locked`: the framework builds from Cargo.lock (`cargoLock.lockFile`), so
# a lock cargo would silently rewrite breaks the Nix build of the tag.
#
# Exactly what CI runs: tests on the committed Cargo.lock
[group('quality')]
ci:
    cargo test --locked --quiet

# Known vulnerabilities in the dependency tree (RustSec advisory database)
[group('quality')]
audit:
    cargo audit

#------------------------------------------------------------------------------
# Release
#------------------------------------------------------------------------------

# Preview the entry the next release would carry — writes nothing
[group('release')]
changelog: _workspace
    @git-cliff --config {{ dnfScripts }}/cliff.toml --unreleased --bump 2>/dev/null

# Version, CHANGELOG entry, commit and tag, no push: just bump [auto|patch|minor|major|X.Y.Z]
[group('release')]
bump level="auto" *args: _workspace
    bash {{ dnfScripts }}/bump.sh \
        --config {{ dnfScripts }}/cliff.toml --level {{ level }} {{ args }}

# Gates, bump, push branch and tag; GitHub publishes the release (release.yml): just release [level] [--yes]
[group('release')]
release level="auto" *args: _workspace
    #!/usr/bin/env bash
    set -euo pipefail

    # Same gates as CI: a red release.yml after the tag push means a published
    # tag without a release.
    just ci
    just audit

    before=$(git rev-parse HEAD)
    just bump {{ level }} {{ args }}

    # `--dry-run` and `--next` commit nothing: HEAD's own tag is not this release.
    if [ "$(git rev-parse HEAD)" = "$before" ]; then
        echo "No release commit — nothing pushed."
        exit 0
    fi
    tag=$(git describe --tags --exact-match --match 'v[0-9]*' HEAD)

    # The branch and this tag only, both or neither: `--tags` would publish
    # every local tag too.
    git push --atomic origin main "refs/tags/$tag" ||
        { echo "Push failed — retry: git push --atomic origin main refs/tags/$tag" >&2; exit 1; }
    echo "Pushed $tag — release.yml publishes the GitHub release; the workspace train pins it in dnf/flake.nix."

# Current version, last tag, revision
[group('release')]
version:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "version .... $(awk '/^\[package\]/{p=1;next} /^\[/{p=0} p&&/^version *=/{split($0,a,"\"");print a[2];exit}' Cargo.toml)"
    echo "last tag ... $(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || echo none)"
    echo "revision ... $(git rev-parse --short HEAD)$(git diff --quiet || echo ' (dirty)')"

_workspace:
    #!/usr/bin/env bash
    if [ ! -f "{{ dnfScripts }}/bump.sh" ]; then
        echo "Release recipes need the DNF co-development workspace: {{ dnfScripts }} not found." >&2
        exit 1
    fi
