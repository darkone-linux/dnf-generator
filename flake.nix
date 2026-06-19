{
  description = "Darkone NixOS Framework — configuration generator (Rust binary).";

  #----------------------------------------------------------------------------
  # FLAKE INPUTS
  #----------------------------------------------------------------------------
  #
  # Single input: nixpkgs. The generator is a pure-Rust crate with no foreign
  # runtime dependencies, so `rustPlatform.buildRustPackage` is all we need.

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  #----------------------------------------------------------------------------
  # FLAKE OUTPUTS
  #----------------------------------------------------------------------------
  #
  # Consumed by `dnf/flake.nix` as a flake input, then re-exposed there as
  # `packages.<system>.dnf-generator` and surfaced on PATH through the
  # framework devShell. The standalone outputs below keep the crate usable on
  # its own (build, run, dev shell) without going through the framework.

  outputs =
    { nixpkgs, ... }:
    let

      supportedSystems = [
        "x86_64-linux"
        # "aarch64-linux"
      ];

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      # Version is read from Cargo.toml so a single source of truth governs
      # both `cargo build` and the Nix derivation.
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

    in
    {

      #------------------------------------------------------------------------
      # PACKAGES
      #------------------------------------------------------------------------

      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          generator = pkgs.rustPlatform.buildRustPackage {
            pname = "dnf-generator";
            version = cargoToml.package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            meta = {
              description = "Configuration generator for the Darkone NixOS Framework";
              mainProgram = "dnf-generator";
            };
          };
        in
        {
          default = generator;
          dnf-generator = generator;
        }
      );

      #------------------------------------------------------------------------
      # DEV SHELL
      #------------------------------------------------------------------------
      #
      # Re-uses `shell.nix` so co-dev (`nix-shell` or `nix develop`) sees the
      # same toolchain regardless of entry point.

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = import ./shell.nix { inherit pkgs; };
        }
      );
    };
}
