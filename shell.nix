{
  pkgs ? import <nixpkgs> { },
}:

pkgs.mkShell {
  buildInputs = with pkgs; [
    rustc
    cargo
    cargo-audit
    rustfmt
    clippy
    rust-analyzer
    pkg-config
    gcc

    # `cargo test`: the etc/config.yaml edits of `darkone add gateway`.
    yq-go

    # CI runs the `just` recipes (`ci`, `audit`); `just bump` renders the
    # CHANGELOG with git-cliff.
    just
    git-cliff
  ];

  shellHook = ''
    alias build="cargo build --release"
    alias test="cargo test"
  '';

  RUST_BACKTRACE = 1;
}
