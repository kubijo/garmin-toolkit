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
  pyproject-build-systems,
  pyproject-nix,
  uv2nix,
  ...
}:
let
  systems =
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          inherit (pkgs) lib;
          # Match the ambient Just entrypoints. Target-scoped flags never reach WASM or Darwin.
          linuxLinkFlags = "-C link-arg=-fuse-ld=mold";
          withDevLinker =
            shell:
            shell.overrideAttrs (
              old:
              lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.mold ];
                CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C linker-features=-lld ${linuxLinkFlags}";
                CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS = linuxLinkFlags;
              }
            );
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
          devTrunk = import ./trunk { inherit pkgs; };
          diagnosticToolchain = fenix.packages.${system}.latest.withComponents [
            "cargo"
            "rustc"
            "rust-std"
          ];
          coverageMinimum = 65;
          pythonToolsEnv = import ./python-tools.nix {
            inherit
              nixpkgs
              pkgs
              pyproject-build-systems
              pyproject-nix
              uv2nix
              ;
          };
          build = import ./packages.nix {
            inherit
              crane
              formatjsCli
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
              pythonToolsEnv
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
              formatjsCli
              system
              toolchain
              ;
            nixCargoTargetDir = ".tmp/nix-cargo-target";
            inherit workspaceSrc;
          };
          galleryTarget = gallery.lib.mkTool {
            inherit
              formatjsCli
              system
              toolchain
              ;
            nixCargoTargetDir = ".tmp/nix-cargo-target";
            inherit workspaceSrc;
          };
          hassTarget = hass.lib.mkTarget {
            inherit
              brandAssets
              formatjsCli
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
              pythonToolsEnv
              toolchain
              wasmToolchain
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
            formatjs-cli = formatjsCli;
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
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
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
            // (removeAttrs expandedQuality.checks [ "rust-coverage" ])
            // {
              desktop = desktopTarget.check;
              gallery = galleryTarget.check;
              hass = hassTarget.check;
              license-bundles = licenseAutomation.check;
            };
          inherit (tooling) formatter;

          devShells = {
            default = withDevLinker (
              pkgs.mkShell (
                {
                  buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
                  packages =
                    tooling.packages
                    ++ galleryTarget.runtimeLibraries
                    ++ [
                      pkgs.bash
                      formatjsCli
                      pkgs.cmake
                      pkgs.cargo-deny
                      pkgs.cargo-llvm-cov
                      pkgs.cargo-machete
                      pkgs.cargo-nextest
                      pkgs.cargo-outdated
                      pkgs.gitleaks
                      pkgs.just
                      pkgs.nodejs
                      pkgs.esbuild
                      pkgs.pkg-config
                      pkgs.samply
                      pkgs.ty
                      pkgs.uv
                      devTrunk
                      pkgs.wasm-bindgen-cli_0_2_126
                      pythonToolsEnv
                      wasmToolchain
                    ]
                    ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                      pkgs.glib
                      pkgs.gvfs
                      pkgs.usbutils
                      pkgs.wrapGAppsNoGuiHook
                    ];
                }
                // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
                  GIO_EXTRA_MODULES = "${pkgs.gvfs}/lib/gio/modules";
                  LD_LIBRARY_PATH = lib.makeLibraryPath galleryTarget.runtimeLibraries;
                }
              )
            );
            desktop = withDevLinker desktopTarget.devShell;
            compiler-profile = self.devShells.${system}.default.overrideAttrs (old: {
              nativeBuildInputs = builtins.filter (package: package != wasmToolchain) old.nativeBuildInputs ++ [
                diagnosticToolchain
                pkgs.measureme
              ];
            });
            gallery = withDevLinker galleryTarget.devShell;
            hass = withDevLinker (
              hassTarget.devShell.overrideAttrs (old: {
                nativeBuildInputs = builtins.filter (package: package != pkgs.trunk) old.nativeBuildInputs ++ [
                  devTrunk
                ];
              })
            );
          };
        }
      );
in
systems
