{
  description = "Garmin Toolkit desktop target";

  inputs = {
    crane.url = "github:ipetkov/crane";
    nix-appimage = {
      url = "github:ralismark/nix-appimage";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixpkgs.url = "github:NixOS/nixpkgs";
  };

  outputs =
    {
      crane,
      nix-appimage,
      nixpkgs,
      ...
    }:
    {
      lib.mkTarget =
        {
          brandAssets,
          dirspec,
          formatjsCli,
          sharedCargoTargetDir,
          system,
          toolchain,
          workspaceSrc,
        }:
        let
          pkgs = import nixpkgs {
            inherit system;
          };
          inherit (pkgs) lib;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          productionIdentity = {
            id = "io.kubijo.GarminToolkit";
            name = "Garmin Toolkit";
            slug = "garmin-toolkit";
          };
          demoIdentity = {
            id = "io.kubijo.GarminToolkit.Demo";
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
          linuxRuntimeEnvironment = {
            GIO_EXTRA_MODULES = "${pkgs.gvfs}/lib/gio/modules";
            LD_LIBRARY_PATH = lib.makeLibraryPath linuxLibraries;
          };
          src = import (workspaceSrc + "/infra/nix/cargo-source.nix") {
            inherit craneLib lib workspaceSrc;
          };
          commonArgs = {
            inherit src;
            CARGO_TARGET_DIR = "target";
            buildInputs = runtimeLibraries;
            cargoLock = workspaceSrc + "/Cargo.lock";
            doCheck = false;
            nativeBuildInputs = [
              formatjsCli
            ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
              pkgs.pkg-config
              pkgs.wrapGAppsNoGuiHook
            ];
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly (
            commonArgs
            // import (workspaceSrc + "/infra/nix/cargo-deps.nix") { inherit lib workspaceSrc; }
            // {
              cargoExtraArgs = "-p garmin-desktop -p garmin-gpx-worker --all-features";
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
                    "@id" = "io.kubijo";
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
          documentation = pkgs.runCommandLocal "garmin-desktop-notices" { } ''
            destination="$out/share/doc/garmin-toolkit"
            mkdir -p "$destination/assets/licenses" "$destination/infra/fixtures/fit"
            for name in LICENSE-AGPL LICENSE-MIT LICENSE-APACHE COPYRIGHT.md THIRD_PARTY_NOTICES.md; do
              install -m444 "${workspaceSrc}/$name" "$destination/$name"
            done
            install -m444 ${workspaceSrc}/assets/licenses/bundle-desktop.json \
              "$destination/assets/licenses/bundle-desktop.json"
            cp -r ${workspaceSrc}/infra/fixtures/fit/development-activities \
              "$destination/infra/fixtures/fit/"
          '';
          installLauncher = launcher: destination: ''
            cp -r ${launcher}/share ${destination}/
            # Avoid cp --no-preserve=mode: its ACL updates fail on macOS builder virtiofs.
            chmod -R u+w ${destination}/share
          '';
          installDocumentation = destination: ''
            cp -r ${documentation}/share ${destination}/
            chmod -R u+w ${destination}/share
          '';
          mkPackage =
            { launcher, demo }:
            let
              cargoExtraArgs =
                "-p garmin-desktop -p garmin-gpx-worker"
                + lib.optionalString demo " --features garmin-desktop/demo";
              packageSpec =
                if pkgs.stdenv.hostPlatform.isLinux then
                  workspaceSrc
                  + "/apps/garmin-desktop/${if demo then "package-demo-linux" else "package-linux"}.dirspec.toml"
                else
                  workspaceSrc + "/apps/garmin-desktop/package-darwin.dirspec.toml";
            in
            craneLib.buildPackage (
              commonArgs
              // {
                inherit cargoExtraArgs;
                pname = "garmin-desktop" + lib.optionalString demo "-demo";
                meta = {
                  mainProgram = "garmin-desktop";
                  license = lib.licenses.agpl3Plus;
                };
                inherit cargoArtifacts;
                nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ dirspec ];
                # Keep dynamically loaded libraries and GIO modules in the
                # application's runtime closure, including outside the dev shell.
                preFixup = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
                  gappsWrapperArgs+=(
                    --prefix LD_LIBRARY_PATH : ${lib.escapeShellArg linuxRuntimeEnvironment.LD_LIBRARY_PATH}
                    --prefix GIO_EXTRA_MODULES : ${lib.escapeShellArg linuxRuntimeEnvironment.GIO_EXTRA_MODULES}
                  )
                '';
                postInstall =
                  installDocumentation ''"$out"''
                  + lib.optionalString pkgs.stdenv.hostPlatform.isLinux (installLauncher launcher ''"$out"'');
                postFixup = ''
                  dirspec "$out" --spec ${packageSpec}
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
              lib
              demoIdentity
              demoLauncher
              demoPackage
              package
              pkgs
              productionIdentity
              launcher
              src
              workspaceSrc
              ;
            mkAppImage = nix-appimage.lib.${system}.mkAppImage;
            appImageRuntime =
              nix-appimage.packages.${system}.appimage-runtimes.appimage-type2-runtime.overrideAttrs
                (
                  previous:
                  assert previous.src.outputHash == "sha256-GR3LMuWMSafQmc2RQyveue3sq+HYBtl+VkcZVYMS0CI=";
                  {
                    # Pinned runtime 01164bfc retains its upstream MIT notice in the source export.
                    meta = (previous.meta or { }) // {
                      license = lib.licenses.mit;
                    };
                  }
                );
            appRun = nix-appimage.packages.${system}.appimage-appruns.userns-chroot.overrideAttrs (
              previous:
              assert nix-appimage.rev == "7946addbc0d97e358a6d7aefe5e82310f0fe6b18";
              {
                # AppRun's original recipe embeds only main.c; retain the enclosing MIT notice too.
                src = nix-appimage.outPath;
                meta = (previous.meta or { }) // {
                  license = lib.licenses.mit;
                };
              }
            );
            withHostGraphics = import (workspaceSrc + "/infra/nix/host-graphics") { inherit pkgs; };
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
          launcherInstallCheck = pkgs.runCommandLocal "check-desktop-launcher-install" { } ''
            mkdir -p "$TMPDIR/production" "$TMPDIR/demo"
            ${installLauncher launcher ''"$TMPDIR/production"''}
            ${installLauncher demoLauncher ''"$TMPDIR/demo"''}
            diff --recursive --brief ${launcher}/share "$TMPDIR/production/share"
            diff --recursive --brief ${demoLauncher}/share "$TMPDIR/demo/share"
            ${installDocumentation ''"$TMPDIR/production"''}
            ${installDocumentation ''"$TMPDIR/demo"''}
            diff --recursive --brief ${documentation}/share/doc "$TMPDIR/production/share/doc"
            diff --recursive --brief ${documentation}/share/doc "$TMPDIR/demo/share/doc"
            test -w "$TMPDIR/production/share/applications/${productionIdentity.id}.desktop"
            test -w "$TMPDIR/demo/share/applications/${demoIdentity.id}.desktop"
            documentation=${documentation}/share/doc/garmin-toolkit
            cmp ${workspaceSrc}/assets/licenses/bundle-desktop.json "$documentation/assets/licenses/bundle-desktop.json"
            for name in LICENSE-AGPL LICENSE-MIT LICENSE-APACHE COPYRIGHT.md THIRD_PARTY_NOTICES.md; do
              cmp "${workspaceSrc}/$name" "$documentation/$name"
            done
            diff --recursive --brief ${workspaceSrc}/infra/fixtures/fit/development-activities \
              "$documentation/infra/fixtures/fit/development-activities"
            touch "$out"
          '';
        in
        {
          inherit
            distribution
            launcher
            demoLauncher
            demoPackage
            launcherInstallCheck
            package
            ;
          check =
            if pkgs.stdenv.hostPlatform.isLinux then
              pkgs.runCommandLocal "check-garmin-toolkit-desktop" { } ''
                test -x ${package}/bin/garmin-desktop
                test -x ${demoPackage}/bin/garmin-desktop
                test -x ${package}/bin/garmin-gpx-worker
                test -x ${demoPackage}/bin/garmin-gpx-worker
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
              CARGO_TARGET_DIR = sharedCargoTargetDir;
              checks = { inherit package; };
            }
            // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux linuxRuntimeEnvironment
          );
        };
    };
}
