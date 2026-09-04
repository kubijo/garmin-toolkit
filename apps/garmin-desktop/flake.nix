{
  description = "Garmin Toolkit desktop target";

  inputs = {
    crane.url = "github:ipetkov/crane";
    crane-portable = {
      url = "github:ipetkov/crane/v0.16.6";
      inputs.nixpkgs.follows = "nixpkgs-portable";
    };
    nixpkgs.url = "github:NixOS/nixpkgs";
    nixpkgs-portable.url = "github:NixOS/nixpkgs/release-23.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      crane,
      crane-portable,
      nixpkgs,
      nixpkgs-portable,
      rust-overlay,
      ...
    }:
    {
      lib.mkTarget =
        {
          brandAssets,
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
          pkgsPortable = import nixpkgs-portable {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          portableToolchain = pkgsPortable.rust-bin.stable."1.98.0".default;
          craneLibPortable = (crane-portable.mkLib pkgsPortable).overrideToolchain portableToolchain;
          productionIdentity = {
            id = "io.github.kubijo.GarminToolkit";
            name = "Garmin Toolkit";
            slug = "garmin-toolkit";
          };
          demoIdentity = {
            id = "io.github.kubijo.GarminToolkit.Demo";
            name = "Garmin Toolkit Demo";
            slug = "garmin-toolkit-demo";
          };
          xmlFormat = pkgs.formats.xml { };
          linuxLibraries = with pkgs; [
            glib
            gvfs
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
          };
          commonArgs = {
            inherit src;
            CARGO_TARGET_DIR = "target";
            buildInputs = runtimeLibraries;
            cargoLock = workspaceSrc + "/Cargo.lock";
            doCheck = false;
            nativeBuildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
              pkgs.pkg-config
              pkgs.wrapGAppsNoGuiHook
            ];
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly (
            commonArgs
            // {
              cargoExtraArgs = "-p garmin-desktop --all-features";
              pname = "garmin-desktop-deps";
            }
          );
          mkLauncher =
            identity:
            let
              desktopEntry = pkgs.makeDesktopItem {
                name = identity.id;
                desktopName = identity.name;
                comment = "Local-first Garmin companion";
                exec = "garmin-desktop";
                icon = identity.id;
                categories = [ "Utility" ];
                startupWMClass = identity.id;
              };
              metainfo = xmlFormat.generate "${identity.id}.metainfo.xml" {
                component = {
                  inherit (identity) id name;
                  "@type" = "desktop-application";
                  summary = "Local-first Garmin companion";
                  metadata_license = "CC0-1.0";
                  project_license = "AGPL-3.0-or-later";
                  description.p = "Import, inspect, and manage Garmin activity data without a mandatory cloud service.";
                  developer = {
                    "@id" = "io.github.kubijo";
                    name = "kubijo";
                  };
                  url = {
                    "@type" = "homepage";
                    "#text" = "https://github.com/kubijo/garmin-toolkit";
                  };
                  launchable = {
                    "@type" = "desktop-id";
                    "#text" = "${identity.id}.desktop";
                  };
                  content_rating."@type" = "oars-1.1";
                };
              };
            in
            pkgs.runCommandLocal "${identity.slug}-desktop-launcher" { } ''
              mkdir -p "$out/share"
              cp -r ${desktopEntry}/share/applications "$out/share/"
              install -Dm444 ${metainfo} "$out/share/metainfo/${identity.id}.metainfo.xml"
              install -Dm444 ${brandAssets}/icon-128.png \
                "$out/share/icons/hicolor/128x128/apps/${identity.id}.png"
              install -Dm444 ${brandAssets}/icon-256.png \
                "$out/share/icons/hicolor/256x256/apps/${identity.id}.png"
              install -Dm444 ${brandAssets}/icon-512.png \
                "$out/share/icons/hicolor/512x512/apps/${identity.id}.png"
            '';
          launcher = mkLauncher productionIdentity;
          demoLauncher = mkLauncher demoIdentity;
          mkPackage =
            { launcher, demo }:
            let
              cargoExtraArgs = "-p garmin-desktop" + lib.optionalString demo " --features demo";
            in
            craneLib.buildPackage (
              commonArgs
              // {
                inherit cargoExtraArgs;
                pname = "garmin-desktop" + lib.optionalString demo "-demo";
                meta.mainProgram = "garmin-desktop";
                inherit cargoArtifacts;
                postInstall = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
                  cp -r --no-preserve=mode ${launcher}/share "$out/"
                '';
              }
            );
          package = mkPackage {
            inherit launcher;
            demo = false;
          };
          demoPackage = mkPackage {
            launcher = demoLauncher;
            demo = true;
          };
          distribution = import ./distribution.nix {
            inherit
              craneLib
              craneLibPortable
              lib
              demoIdentity
              demoLauncher
              pkgs
              pkgsPortable
              productionIdentity
              launcher
              src
              workspaceSrc
              ;
          };
          launcherCheck =
            pkgs.runCommandLocal "check-garmin-toolkit-desktop-launcher"
              {
                nativeBuildInputs = [
                  pkgs.appstream
                  pkgs.desktop-file-utils
                ];
              }
              ''
                desktop-file-validate ${launcher}/share/applications/${productionIdentity.id}.desktop
                desktop-file-validate ${demoLauncher}/share/applications/${demoIdentity.id}.desktop
                appstreamcli validate --no-net ${launcher}/share/metainfo/${productionIdentity.id}.metainfo.xml
                appstreamcli validate --no-net ${demoLauncher}/share/metainfo/${demoIdentity.id}.metainfo.xml
                touch "$out"
              '';
        in
        {
          inherit
            distribution
            launcher
            demoLauncher
            demoPackage
            package
            ;
          check =
            if pkgs.stdenv.hostPlatform.isLinux then
              pkgs.runCommandLocal "check-garmin-toolkit-desktop" { } ''
                test -x ${package}/bin/garmin-desktop
                test -x ${demoPackage}/bin/garmin-desktop
                test -e ${launcherCheck}
                test -x ${distribution.appImage}
                test -x ${distribution.demoAppImage}
                test -f ${distribution.flatpakStage}/${productionIdentity.id}.yml
                test -f ${distribution.demoFlatpakStage}/${demoIdentity.id}.yml
                touch "$out"
              ''
            else
              package;
          devShell = craneLib.devShell (
            {
              CARGO_TARGET_DIR = nixCargoTargetDir;
              checks = { inherit package; };
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              GIO_EXTRA_MODULES = "${pkgs.gvfs}/lib/gio/modules";
              LD_LIBRARY_PATH = lib.makeLibraryPath linuxLibraries;
            }
          );
        };
    };
}
