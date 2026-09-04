{
  desktop,
  self,
  nixpkgs,
  flake-utils,
  fenix,
  crane,
  gallery,
  hass,
  nix-tools,
  ...
}:
let
  systems =
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
      ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          inherit (pkgs) lib;
          workspaceSrc = ../..;
          toolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "clippy"
            "llvm-tools"
            "rust-src"
            "rustc"
            "rustfmt"
          ];
          wasmToolchain = fenix.packages.${system}.combine [
            toolchain
            fenix.packages.${system}.targets.wasm32-unknown-unknown.stable.rust-std
          ];
          # Hardware-only MTP lowers the initial floor. Raise it with simulator
          # coverage; the active plan tracks owner-specific thresholds.
          coverageMinimum = 30;
          build = import ./packages.nix {
            inherit
              crane
              lib
              pkgs
              system
              toolchain
              workspaceSrc
              ;
          };
          tooling = import ./tooling.nix {
            inherit
              lib
              nix-tools
              pkgs
              system
              toolchain
              workspaceSrc
              ;
          };
          inheritanceCheck = pkgs.callPackage ./cargo-workspace-inheritance-check.nix { };
          brandAssets = import ./brand-assets.nix {
            inherit pkgs;
            source = ../../crates/garmin-brand/assets/icon.svg;
          };
          desktopTarget = desktop.lib.mkTarget {
            inherit
              brandAssets
              system
              toolchain
              ;
            nixCargoTargetDir = ".tmp/nix-cargo-target";
            inherit workspaceSrc;
          };
          galleryTarget = gallery.lib.mkTool {
            inherit
              system
              toolchain
              ;
            nixCargoTargetDir = ".tmp/nix-cargo-target";
            inherit workspaceSrc;
          };
          hassTarget = hass.lib.mkTarget {
            inherit
              brandAssets
              system
              toolchain
              wasmToolchain
              ;
            nixCargoTargetDir = ".tmp/nix-cargo-target";
            inherit workspaceSrc;
          };
          formatjsCli = pkgs.callPackage ./formatjs-cli.nix { };
          licenseAutomation = import ./licenses.nix {
            inherit
              lib
              pkgs
              toolchain
              ;
            craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
            inherit workspaceSrc;
          };
          expandedQuality = import ./rust-checks.nix {
            inherit
              coverageMinimum
              formatjsCli
              inheritanceCheck
              lib
              pkgs
              toolchain
              ;
            craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
            galleryRuntimeLibraries = galleryTarget.runtimeLibraries;
            licenseChecker = licenseAutomation.checker;
            nixCargoTargetDir = ".tmp/nix-cargo-target";
            inherit workspaceSrc;
          };
        in
        {
          packages = {
            desktop = desktopTarget.package;
            desktop-demo = desktopTarget.demoPackage;
            garmin-hass = hassTarget.package;
            garmin-hass-demo = hassTarget.demoPackage;
            garmin-cli = build.garminCli;
            gallery = galleryTarget.package;
            default = build.garminCli;
          };

          apps =
            tooling.apps
            // lib.mapAttrs (_: package: {
              type = "app";
              program = lib.getExe package;
            }) expandedQuality.apps
            // {
              licenses = {
                type = "app";
                program = lib.getExe licenseAutomation.app;
              };
              licenses-check = {
                type = "app";
                program = lib.getExe licenseAutomation.checker;
              };
              garmin-cli = flake-utils.lib.mkApp { drv = build.garminCli; };
              default = self.apps.${system}.garmin-cli;
              desktop-appimage = {
                type = "app";
                program = lib.getExe desktopTarget.distribution.appImageExporter;
              };
              desktop-demo-appimage = {
                type = "app";
                program = lib.getExe desktopTarget.distribution.demoAppImageExporter;
              };
              desktop-flatpak = {
                type = "app";
                program = lib.getExe desktopTarget.distribution.flatpakBuilder;
              };
              desktop-demo-flatpak = {
                type = "app";
                program = lib.getExe desktopTarget.distribution.demoFlatpakBuilder;
              };
            };

          checks =
            tooling.checks
            // build.checks
            // (builtins.removeAttrs expandedQuality.checks [ "rust-coverage" ])
            // {
              desktop = desktopTarget.check;
              gallery = galleryTarget.check;
              hass = hassTarget.check;
              license-bundles = licenseAutomation.check;
            };
          inherit (tooling) formatter;

          devShells = {
            default = pkgs.mkShellNoCC {
              packages =
                tooling.packages
                ++ galleryTarget.runtimeLibraries
                ++ [
                  pkgs.cargo-deny
                  pkgs.cargo-llvm-cov
                  pkgs.cargo-machete
                  pkgs.cargo-nextest
                  pkgs.cargo-outdated
                  pkgs.gitleaks
                  pkgs.glib
                  pkgs.gvfs
                  pkgs.just
                  pkgs.pkg-config
                  pkgs.usbutils
                  pkgs.wrapGAppsNoGuiHook
                  toolchain
                ];
              GIO_EXTRA_MODULES = "${pkgs.gvfs}/lib/gio/modules";
              LD_LIBRARY_PATH = lib.makeLibraryPath galleryTarget.runtimeLibraries;
            };
            desktop = desktopTarget.devShell;
            gallery = galleryTarget.devShell;
            hass = hassTarget.devShell;
          };
        }
      );
in
systems
