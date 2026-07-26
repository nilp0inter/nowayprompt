{
  description = "nowayprompt - Rust Wayland prompt utility";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Pinned oracle revision for the NixOS differential parity tests:
    # pkgs.wayprompt (v0.1.2) from nixos-26.05 is the pinned behavioral
    # oracle the target is asserted against. Kept separate from the
    # dev/build nixpkgs so the oracle cannot drift with nixos-unstable.
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
      perSystem = { pkgs, system, config, ... }:
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
                ./man
                # Bundled fonts referenced via `include_bytes!` in the
                # Wayland render pipeline.
                ./assets
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.libxkbcommon ];
            cargoBuildFlags = [ "--bin" "nowayprompt" ];
            cargoInstallFlags = [ "--bin" "nowayprompt" ];
            postInstall = ''
              ln -s nowayprompt "$out/bin/pinentry-nowayprompt"
              ln -s nowayprompt "$out/bin/nowayprompt-ssh-askpass"
              install -Dm644 man/nowayprompt.1 \
                "$out/share/man/man1/nowayprompt.1"
              install -Dm644 man/pinentry-nowayprompt.1 \
                "$out/share/man/man1/pinentry-nowayprompt.1"
              install -Dm644 man/nowayprompt-ssh-askpass.1 \
                "$out/share/man/man1/nowayprompt-ssh-askpass.1"
              install -Dm644 man/nowayprompt.conf.5 \
                "$out/share/man/man5/nowayprompt.conf.5"
            '';
            meta = {
              description = "Wayland prompt tool (pinentry and ssh-askpass replacement)";
              homepage = "https://github.com/nilp0inter/nowayprompt";
              license = pkgs.lib.licenses.gpl3Only;
              mainProgram = "pinentry-nowayprompt";
              maintainers = [ pkgs.lib.maintainers.nilp0inter ];
            };
          };

          # Bare `nix build` resolves the public package.
          packages.default = config.packages.nowayprompt;

          # Test infrastructure only; it is consumed by the Wayland parity
          # derivation and is not installed by the public package.
          packages.nowayprompt-wayland-test =
            pkgs.rustPlatform.buildRustPackage {
              pname = "nowayprompt-wayland-test";
              version = "0.1.0";
              src = pkgs.lib.fileset.toSource {
                root = ./.;
                fileset = pkgs.lib.fileset.unions [
                  ./Cargo.toml
                  ./Cargo.lock
                  ./src
                  ./assets
                ];
              };
              cargoLock.lockFile = ./Cargo.lock;
              nativeBuildInputs = [ pkgs.pkg-config ];
              buildInputs = [ pkgs.libxkbcommon ];
              cargoBuildFlags = [ "--bin" "nowayprompt-wayland-test" ];
              cargoInstallFlags = [ "--bin" "nowayprompt-wayland-test" ];
            };

          checks.nowayprompt-package-interface = pkgs.runCommand
            "nowayprompt-package-interface"
            { target = self.packages.${system}.nowayprompt; }
            ''
              test -x "$target/bin/nowayprompt"
              test -L "$target/bin/pinentry-nowayprompt"
              test -L "$target/bin/nowayprompt-ssh-askpass"
              test "$(readlink "$target/bin/pinentry-nowayprompt")" = nowayprompt
              test "$(readlink "$target/bin/nowayprompt-ssh-askpass")" = nowayprompt
              test -f "$target/share/man/man1/nowayprompt.1.gz"
              test -f "$target/share/man/man1/pinentry-nowayprompt.1.gz"
              test -f "$target/share/man/man1/nowayprompt-ssh-askpass.1.gz"
              test -f "$target/share/man/man5/nowayprompt.conf.5.gz"
              touch "$out"
            '';

          devShells.default = pkgs.mkShell {
            packages = rustToolchain ++ nativeDeps;
            shellHook = hooks.shellHook;
          };
        };
    };
}