{
  description = "nowayprompt - Rust Wayland prompt utility";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Pinned oracle revision for the NixOS differential parity tests:
    # pkgs.wayprompt (v0.1.2) from nixos-26.05 is the legacy baseline the
    # Rust target is asserted against. Kept separate from the dev/build
    # nixpkgs so the oracle cannot drift with nixos-unstable.
    nixpkgs-26_05.url = "github:nixos/nixpkgs/nixos-26.05";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks-nix = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self
    , nixpkgs
    , nixpkgs-26_05
    , flake-parts
    , git-hooks-nix
    , ...
    }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" ];

      # NixOS VM differential parity tests. Deliberately a top-level (not
      # perSystem) output: nixosTest VMs are x86_64-linux, and the test
      # closure must install the oracle from the pinned nixos-26.05 input
      # while installing the target built by this flake.
      flake.nixosTests = import ./nixos-tests {
        inherit (nixpkgs) lib;
        nixpkgs = nixpkgs-26_05;
        pkgs = nixpkgs-26_05.legacyPackages.x86_64-linux;
        selfpkgs = self.packages;
      };
      perSystem = { pkgs, system, ... }:
        let
          rustToolchain = with pkgs; [
            rustc
            cargo
            rust-analyzer
            clippy
            rustfmt
          ];
          nativeDeps = with pkgs; [
            pkg-config
            libxkbcommon
          ];

          hooks = git-hooks-nix.lib.${system}.run {
            src = ./.;
            hooks = {
              # Format check on pre-commit: reject commits with unformatted code.
              rustfmt = {
                enable = true;
                package = pkgs.rustfmt;
                entry = "${pkgs.cargo}/bin/cargo fmt -- --check";
              };
              # Clippy gate on pre-commit: no warnings allowed. `cargo clippy`
              # does not accept filenames as positional arguments.
              clippy = {
                enable = true;
                package = pkgs.cargo;
                entry = "${pkgs.cargo}/bin/cargo clippy -- -D warnings";
                pass_filenames = false;
                files = "\\.(rs|toml)$";
              };
              # Full test suite on pre-push. `cargo test` does not accept
              # filenames as positional arguments, so disable file passing.
              cargo-test = {
                enable = true;
                package = pkgs.cargo;
                entry = "${pkgs.cargo}/bin/cargo test";
                pass_filenames = false;
                stages = [ "pre-push" ];
              };
            };
          };
        in
        {
          packages.nowayprompt = pkgs.rustPlatform.buildRustPackage {
            pname = "nowayprompt";
            version = "0.1.0";
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./src
                # Bundled fonts referenced via `include_bytes!` in the
                # Wayland render pipeline (design D7).
                ./assets
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.libxkbcommon ];
            # Minimal Stage 2 package: just the nowayprompt binary. The
            # full packaging (pinentry-/askpass symlinks, manpages) is a
            # Stage 4 deliverable; the nixosTests must not depend on it.
            meta.mainProgram = "nowayprompt";
          };

          devShells.default = pkgs.mkShell {
            packages = rustToolchain ++ nativeDeps;
            shellHook = hooks.shellHook;
          };
        };
    };
}