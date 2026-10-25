{
  description = "bonsai - GFS snapshot manager for bcachefs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    flake-parts.url = "github:hercules-ci/flake-parts";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ self, flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];

      perSystem =
        { system, ... }:
        let
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };
          # Pinned toolchain; keep in sync with rust-toolchain.toml and the
          # CI workflow.
          toolchain = pkgs.rust-bin.stable."1.95.0".default;
          rustPlatform = pkgs.makeRustPlatform {
            rustc = toolchain;
            cargo = toolchain;
          };
        in
        {
          packages.default = rustPlatform.buildRustPackage {
            pname = "bonsai";
            # Keep in sync with Cargo.toml automatically.
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;

            meta = {
              description = "Grandfather-father-son snapshot manager for bcachefs";
              homepage = "https://github.com/elcattivo66/bonsai";
              license = pkgs.lib.licenses.agpl3Only;
              mainProgram = "bonsai";
            };
          };

          devShells.default = pkgs.mkShell {
            packages = [
              toolchain
              pkgs.rust-analyzer
            ];
          };

          formatter = pkgs.nixfmt;
        };
    };
}
