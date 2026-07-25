{
  description = "nowayprompt - Rust Wayland prompt utility";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks-nix = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-parts, git-hooks-nix, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [ "x86_64-linux" "aarch64-linux" ];

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
          devShells.default = pkgs.mkShell {
            packages = rustToolchain ++ nativeDeps;
            shellHook = hooks.shellHook;
          };
        };
    };
}