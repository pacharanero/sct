# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later
{
  description = "sct - a fast, local-first SNOMED CT toolchain in a single binary";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # The platforms sct's released binaries already target.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
      # Single-source the version from Cargo.toml so a release bump flows through
      # without touching this file.
      version = (nixpkgs.lib.importTOML ./Cargo.toml).package.version;
    in
    {
      packages = forAllSystems (
        system: pkgs: rec {
          sct = pkgs.rustPlatform.buildRustPackage {
            pname = "sct-rs";
            inherit version;
            # `self` is the flake's git tree, so gitignored artefacts (a local
            # `snomed.db`, `target/`) are excluded from the build source.
            src = self;
            cargoLock.lockFile = ./Cargo.lock;

            # No system libraries to link: SQLite is vendored (rusqlite
            # "bundled", compiled by the stdenv C toolchain) and TLS is rustls
            # (via ureq), not OpenSSL. pkg-config is harmless insurance for any
            # build script that probes for it.
            nativeBuildInputs = [ pkgs.pkg-config ];

            # The suite mutates $HOME / cwd / env and expects a real filesystem
            # layout, which the sealed build sandbox does not provide; it runs in
            # CI instead.
            doCheck = false;

            meta = {
              description = "Local-first SNOMED CT toolchain: query, FHIR terminology server, crossmaps, codelists";
              homepage = "https://github.com/pacharanero/sct";
              license = pkgs.lib.licenses.agpl3Plus;
              mainProgram = "sct";
              platforms = systems;
            };
          };
          default = sct;
        }
      );

      apps = forAllSystems (
        system: _pkgs: rec {
          sct = {
            type = "app";
            program = "${self.packages.${system}.sct}/bin/sct";
          };
          default = sct;
        }
      );

      devShells = forAllSystems (
        system: pkgs: {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.sct ];
            packages = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer
            ];
          };
        }
      );
    };
}
