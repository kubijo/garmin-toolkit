{
  description = "Garmin Toolkit component gallery";

  inputs = {
    crane.url = "github:ipetkov/crane";
    nixpkgs.url = "github:NixOS/nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      crane,
      nixpkgs,
      rust-overlay,
      ...
    }:
    {
      lib.mkTool =
        {
          nixCargoTargetDir,
          system,
          toolchain,
          workspaceSrc,
        }:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          inherit (pkgs) lib;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          manifestArgs = "--manifest-path Cargo.toml --locked --all-features";
          unlockedManifestArgs = "--manifest-path Cargo.toml --all-features";
          linuxLibraries = with pkgs; [
            fontconfig
            glib
            libGL
            libx11
            libxcursor
            libxi
            libxkbcommon
            libxrandr
            vulkan-loader
            wayland
          ];
          runtimeLibraries = lib.optionals pkgs.stdenv.hostPlatform.isLinux linuxLibraries;
          src = import (workspaceSrc + "/infra/nix/cargo-source.nix") {
            inherit craneLib lib workspaceSrc;
            extraFilesets = [
              (workspaceSrc + "/infra/gallery/fonts")
              (workspaceSrc + "/infra/gallery/gallery.toml")
            ];
            includeGallery = true;
          };
          cargoLock = workspaceSrc + "/infra/gallery/Cargo.lock";
          baseArgs = {
            inherit src;
            CARGO_TARGET_DIR = "target";
            buildInputs = runtimeLibraries;
            inherit cargoLock;
            nativeBuildInputs = [
              pkgs.cmake
            ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.pkg-config ];
            pname = "garmin-gallery";
            postUnpack = ''
              cd "$sourceRoot/infra/gallery"
              sourceRoot=.
            '';
            strictDeps = true;
            version = "0.1.0";
          };
          cargoVendorDir = craneLib.vendorCargoDeps baseArgs;
          commonArgs =
            baseArgs
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibraries;
            }
            // {
              inherit cargoVendorDir;
            };
          cargoArtifacts = craneLib.buildDepsOnly (
            commonArgs
            // {
              cargoExtraArgs = manifestArgs;
              doCheck = false;
            }
          );
          package = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs = manifestArgs;
              doCheck = false;
            }
          );
          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "${unlockedManifestArgs} --all-targets -- --deny warnings";
            }
          );
          tests = craneLib.cargoNextest (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs = "${manifestArgs} --no-tests pass";
              doCheck = true;
            }
          );
          docs = craneLib.cargoDoc (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoDocExtraArgs = "${unlockedManifestArgs} --no-deps";
              RUSTDOCFLAGS = "-D warnings";
            }
          );
          check = pkgs.linkFarm "garmin-toolkit-gallery-check" [
            {
              name = "clippy";
              path = clippy;
            }
            {
              name = "docs";
              path = docs;
            }
            {
              name = "package";
              path = package;
            }
            {
              name = "tests";
              path = tests;
            }
          ];
        in
        {
          inherit check package;
          devShell = craneLib.devShell (
            {
              CARGO_TARGET_DIR = nixCargoTargetDir;
              checks = { inherit check; };
              packages = [ pkgs.cargo-watch ];
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath linuxLibraries;
            }
          );
          inherit runtimeLibraries;
        };
    };
}
