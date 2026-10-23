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
  nix-oci,
  pyproject-build-systems,
  pyproject-nix,
  uv2nix,
  ...
}:
let
  inherit (builtins) attrNames filter;

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
          pkgs = import nixpkgs {
            inherit system;
            overlays = [
              (final: prev: {
                # Coverage fixtures and reports must use the selected compiler's LLVM tools.
                cargo-llvm-cov =
                  (prev.cargo-llvm-cov.override {
                    rustPlatform = final.makeRustPlatform {
                      cargo = toolchain;
                      rustc = toolchain;
                    };
                  }).overrideAttrs
                    (old: {
                      env = (old.env or { }) // llvmToolsEnv;
                    });
              })
            ];
          };
          inherit (pkgs) lib;
          inherit (nix-tools.lib.packagesFor pkgs) dirspec;
          nodejs = pkgs.nodejs_26;
          # Match the ambient Just entrypoints. Target-scoped flags never reach WASM or Darwin.
          linuxLinkFlags = "-C link-arg=-fuse-ld=mold";
          hostSccacheHook = ''
            if [[ -z ''${RUSTC_WRAPPER+x} ]] && host_sccache=$(command -v sccache); then
              export RUSTC_WRAPPER="$host_sccache"
            fi
          '';
          nativeCompilerEnv = lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            CC = lib.getExe' pkgs.stdenv.cc "cc";
            CXX = lib.getExe' pkgs.stdenv.cc "c++";
            "CARGO_TARGET_${
              lib.toUpper (lib.replaceStrings [ "-" ] [ "_" ] pkgs.stdenv.hostPlatform.rust.rustcTarget)
            }_LINKER" =
              lib.getExe' pkgs.stdenv.cc "cc";
          };
          withDevLinker =
            shell:
            shell.overrideAttrs (
              old:
              {
                shellHook = (old.shellHook or "") + "\n" + hostSccacheHook;
              }
              // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
                nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.mold ];
                CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C linker-features=-lld ${linuxLinkFlags}";
                CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS = linuxLinkFlags;
                VK_ADD_DRIVER_FILES = softwareVulkanIcd;
              }
              // nativeCompilerEnv
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
          rustLlvmTools = "${toolchain}/lib/rustlib/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/bin";
          llvmToolsEnv = {
            LLVM_COV = "${rustLlvmTools}/llvm-cov";
            LLVM_PROFDATA = "${rustLlvmTools}/llvm-profdata";
          };
          softwareVulkanIcd = "${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json";
          wasmToolchain = fenix.packages.${system}.combine [
            toolchain
            fenix.packages.${system}.targets.wasm32-unknown-unknown.stable.rust-std
          ];
          devTrunk = import ./trunk { inherit pkgs; };
          sharedCargoTargetDir = ".tmp/cargo-target";
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
              dirspec
              formatjsCli
              lib
              pkgs
              system
              toolchain
              workspaceSrc
              ;
          };
          tooling = import ./tooling.nix {
            nixOci = nix-oci.packages.${system}.default;
            inherit
              dirspec
              lib
              nix-tools
              nodejs
              pkgs
              pythonToolsEnv
              system
              toolchain
              ;
            # Reuse the flake source without copying a path during read-only evaluation.
            src = self;
          };
          inheritanceCheck = pkgs.callPackage ./cargo-workspace-inheritance-check.nix { };
          brandAssets = import ./brand-assets.nix {
            inherit pkgs;
            source = ../../crates/garmin-brand/assets/icon.svg;
          };
          desktopTarget = desktop.lib.mkTarget {
            inherit
              brandAssets
              dirspec
              formatjsCli
              system
              toolchain
              ;
            inherit sharedCargoTargetDir;
            inherit workspaceSrc;
          };
          galleryTarget = gallery.lib.mkTool {
            inherit
              dirspec
              formatjsCli
              system
              toolchain
              ;
            inherit sharedCargoTargetDir;
            inherit workspaceSrc;
          };
          hassTarget = hass.lib.mkTarget {
            sourceRevision = self.rev or self.dirtyRev or "unreleased";
            inherit
              brandAssets
              dirspec
              formatjsCli
              nodejs
              system
              toolchain
              wasmToolchain
              ;
            inherit sharedCargoTargetDir;
            inherit workspaceSrc;
          };
          mkHassIntegration =
            options:
            import ../integration/hass (
              {
                inherit nodejs pkgs;
                package = hassTarget.demoPackage;
              }
              // options
            );
          hassIntegration = mkHassIntegration { };
          hassReconnect = mkHassIntegration { testFile = "reconnect.test.mjs"; };
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
          releaseAutomation = import ./hass-release.nix {
            inherit
              brandAssets
              dirspec
              lib
              nodejs
              pkgs
              pythonToolsEnv
              workspaceSrc
              ;
            inherit (hassTarget) appImage package;
          };
          expandedQuality = import ./rust-checks.nix {
            inherit
              coverageMinimum
              formatjsCli
              hostSccacheHook
              inheritanceCheck
              lib
              llvmToolsEnv
              nativeCompilerEnv
              pkgs
              pythonToolsEnv
              toolchain
              wasmToolchain
              ;
            craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
            galleryRuntimeLibraries = galleryTarget.runtimeLibraries;
            licenseChecker = licenseAutomation.checker;
            inherit sharedCargoTargetDir;
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
            hass-release = releaseAutomation.app;
            hass-ci = releaseAutomation.ci;
            hass-recovery = releaseAutomation.recovery;
            default = build.garminCli;
          }
          // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            garmin-hass-app-image = hassTarget.appImage;
            garmin-hass-app-sbom = releaseAutomation.sbom;
            garmin-hass-app-repository = releaseAutomation.repository;
            garmin-hass-arm64-test-repository = releaseAutomation.arm64TestRepository;
            hass-reconnect-driver = hassReconnect.driver;
          };

          legacyPackages.hass-source-metadata = hassTarget.sourceMetadata;

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
              hass-release = flake-utils.lib.mkApp { drv = releaseAutomation.app; };
              hass-ci = flake-utils.lib.mkApp { drv = releaseAutomation.ci; };
              hass-recovery = flake-utils.lib.mkApp { drv = releaseAutomation.recovery; };
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
              hass-app-repository = releaseAutomation.repository;
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              hass-integration = hassIntegration;
            };
          legacyPackages.ci =
            let
              checks = self.checks.${system};
              groups = lib.groupBy (
                name:
                if lib.hasPrefix "rust-" name then
                  "rust"
                else if lib.hasPrefix "hass" name then
                  "hass"
                else if name == "desktop" || name == "gallery" then
                  name
                else
                  "tooling"
              ) (attrNames checks);
            in
            lib.mapAttrs (group: names: pkgs.linkFarm "garmin-ci-${group}" (lib.getAttrs names checks)) groups;
          inherit (tooling) formatter;

          devShells = {
            default = withDevLinker (
              pkgs.mkShell (
                llvmToolsEnv
                // {
                  buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
                  packages =
                    tooling.packages
                    ++ galleryTarget.runtimeLibraries
                    ++ [
                      pkgs.bash
                      pkgs.coreutils
                      pkgs.git
                      dirspec
                      formatjsCli
                      pkgs.cmake
                      pkgs.cargo-deny
                      pkgs.cargo-llvm-cov
                      pkgs.cargo-machete
                      pkgs.cargo-nextest
                      pkgs.gitleaks
                      pkgs.just
                      nodejs
                      pkgs.esbuild
                      pkgs.pkg-config
                      pkgs.samply
                      pkgs.basedpyright
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
              nativeBuildInputs = filter (package: package != wasmToolchain) old.nativeBuildInputs ++ [
                diagnosticToolchain
                pkgs.measureme
              ];
            });
            gallery = withDevLinker galleryTarget.devShell;
            hass = withDevLinker (
              hassTarget.devShell.overrideAttrs (old: {
                nativeBuildInputs = filter (package: package != pkgs.trunk) old.nativeBuildInputs ++ [
                  devTrunk
                ];
              })
            );
          };
        }
      );
in
systems
