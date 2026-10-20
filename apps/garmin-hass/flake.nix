{
  description = "Garmin Toolkit Home Assistant target";

  inputs = {
    crane.url = "github:ipetkov/crane";
    nixpkgs.url = "github:NixOS/nixpkgs";
    nix-oci.url = "github:systemstart/nix-oci/v0.6.0";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      crane,
      nixpkgs,
      nix-oci,
      rust-overlay,
      ...
    }:
    {
      lib.mkTarget =
        {
          brandAssets,
          dirspec,
          formatjsCli,
          sharedCargoTargetDir,
          nodejs,
          system,
          toolchain,
          wasmToolchain,
          workspaceSrc,
          sourceRevision,
        }:
        let
          inherit (builtins) fromTOML readFile;
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          inherit (pkgs) lib;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          wasmCraneLib = (crane.mkLib pkgs).overrideToolchain wasmToolchain;
          auditableCargo = pkgs.writeShellScriptBin "cargo" ''
            exec ${lib.getExe' wasmToolchain "cargo"} auditable "$@"
          '';
          enableAuditMetadata = ''
            export PATH=${auditableCargo}/bin:$PATH
          '';
          src = import (workspaceSrc + "/infra/nix/cargo-source.nix") {
            inherit craneLib lib workspaceSrc;
          };
          webSpec = workspaceSrc + "/apps/garmin-hass/web.dirspec.toml";
          packageSpec = workspaceSrc + "/apps/garmin-hass/package.dirspec.toml";
          commonArgs = {
            inherit src;
            nativeBuildInputs = [ formatjsCli ];
            CARGO_TARGET_DIR = "target";
            cargoLock = workspaceSrc + "/Cargo.lock";
            doCheck = false;
            strictDeps = true;
          };
          nativeArgs = commonArgs // {
            buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.glib ];
            nativeBuildInputs =
              commonArgs.nativeBuildInputs ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.pkg-config ];
          };
          cargoArtifacts = craneLib.buildDepsOnly (
            nativeArgs
            // import (workspaceSrc + "/infra/nix/cargo-deps.nix") { inherit lib workspaceSrc; }
            // {
              cargoExtraArgs = "-p garmin-hass -p garmin-gpx-worker --all-features";
              pname = "garmin-hass-deps";
            }
          );
          webArgs = commonArgs // {
            CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
            cargoExtraArgs = "-p garmin-hass-web";
          };
          webCargoArtifacts = wasmCraneLib.buildDepsOnly (
            webArgs
            // import (workspaceSrc + "/infra/nix/cargo-deps.nix") { inherit lib workspaceSrc; }
            // {
              pname = "garmin-hass-web-deps";
            }
          );
          webPackage = wasmCraneLib.buildTrunkPackage (
            webArgs
            // {
              pname = "garmin-hass-web";
              cargoArtifacts = webCargoArtifacts;
              trunkIndexPath = "apps/garmin-hass/web/index.html";
              wasm-bindgen-cli = pkgs.wasm-bindgen-cli_0_2_126;
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
                dirspec
                nodejs
                pkgs.cargo-auditable
                pkgs.esbuild
              ];
              preBuild = enableAuditMetadata;

              buildPhaseCargoCommand = ''
                ( cd apps/garmin-hass/web && trunk build --release=true index.html )
              '';

              postBuild = ''
                export GARMIN_TEST_WEB_ROOT="$PWD/apps/garmin-hass/web/dist"
                for test in infra/javascript/*.test.mjs infra/javascript/*.test.mts; do
                  ${lib.getExe nodejs} "$test"
                done
              '';

              installPhaseCommand = ''
                webRoot="$out/share/garmin-hass/web"
                mkdir -p "$webRoot"
                cp -r apps/garmin-hass/web/dist/. "$webRoot/"
                dirspec "$webRoot" --spec ${webSpec}
              '';
            }
          );
          presentation = pkgs.runCommandLocal "garmin-hass-presentation" { } ''
            install -Dm444 ${brandAssets}/icon-128.png "$out/icon.png"
            install -Dm444 ${brandAssets}/logo-250x100.png "$out/logo.png"
          '';
          mkPackage =
            { demo }:
            let
              cargoExtraArgs =
                "-p garmin-hass -p garmin-gpx-worker" + lib.optionalString demo " --features garmin-hass/demo";
              runtimeWrapperArgs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
                "--prefix"
                "GIO_EXTRA_MODULES"
                ":"
                "${pkgs.gvfs}/lib/gio/modules"
                "--prefix"
                "LD_LIBRARY_PATH"
                ":"
                (lib.makeLibraryPath [
                  pkgs.glib
                  pkgs.gvfs
                ])
              ];
            in
            craneLib.buildPackage (
              nativeArgs
              // {
                inherit cargoExtraArgs;
                pname = "garmin-hass" + lib.optionalString demo "-demo";
                meta.mainProgram = "garmin-hass";
                meta.license = lib.licenses.agpl3Plus;
                inherit cargoArtifacts;
                nativeBuildInputs = nativeArgs.nativeBuildInputs ++ [
                  dirspec
                  pkgs.cargo-auditable
                  pkgs.makeWrapper
                  pkgs.pkg-config
                ];
                preBuild = enableAuditMetadata;
                postInstall = ''
                  install -Dm444 ${presentation}/icon.png "$out/share/garmin-hass/icon.png"
                  install -Dm444 ${presentation}/logo.png "$out/share/garmin-hass/logo.png"
                  cp -r ${webPackage}/share/garmin-hass/web "$out/share/garmin-hass/web"
                  chmod -R u+w "$out/share/garmin-hass/web"
                '';
                postFixup = ''
                  wrapProgram "$out/bin/garmin-hass" \
                    --set-default GARMIN_TOOLKIT_HASS_WEB_ROOT "$out/share/garmin-hass/web" \
                    ${lib.escapeShellArgs runtimeWrapperArgs}
                  dirspec "$out" --spec ${packageSpec}
                '';
              }
            );
          package = mkPackage { demo = false; };
          demoPackage = mkPackage { demo = true; };
          appArch =
            if system == "x86_64-linux" then
              "amd64"
            else if system == "aarch64-linux" then
              "aarch64"
            else
              throw "Home Assistant app images require amd64 or aarch64 Linux";
          appVersion = (fromTOML (readFile (workspaceSrc + "/Cargo.toml"))).workspace.package.version;
          sourceMetadata = import (workspaceSrc + "/infra/nix/hass-source-metadata.nix") {
            inherit lib;
            roots = [
              # Reviewed ICU metadata; retain the complete LICENSE in its source archive.
              (
                assert pkgs.icu.version == "78.3";
                assert pkgs.icu.src.outputHash == "sha256-Oi56R2BLpwLzRYeDCOb+/sphLuiVz0pfIi55Vfq/4MA=";
                pkgs.icu.overrideAttrs (previous: {
                  meta = previous.meta // {
                    license = lib.licenses.unicode-30;
                  };
                })
              )
              package
              pkgs.gvfs
              pkgs.glib
              pkgs.cacert
              # Helpers embedded in upstream patches and wrappers, outside buildInputs.
              pkgs.btrfs-progs
              pkgs.dconf
              pkgs.dejavu_fonts.minimal
              pkgs.dns-root-data
              pkgs.exfat
              pkgs.f2fs-tools
              pkgs.gnused
              pkgs.gzip
              pkgs.liburcu
              pkgs.mailcap
              pkgs.mdadm
              pkgs.ncompress
              pkgs.nilfs-utils
              pkgs.ntfs3g
              pkgs.python3Packages.dbus-python
              pkgs.shadow
              pkgs.system-sendmail
              pkgs.thin-provisioning-tools
              pkgs.xfsprogs
              (pkgs.python3.withPackages (packages: [ packages.dbus-python ]))
            ];
          };
          appImage = nix-oci.legacyPackages.${system}.buildOCIImage {
            name = "garmin-toolkit-hass.oci.tar";
            format = "archive";
            ref = "ghcr.io/kubijo/garmin-toolkit-hass:${appVersion}";
            arch = if appArch == "aarch64" then "arm64" else appArch;
            contents = [
              package
              pkgs.cacert
            ];
            extraCommands = ''
              mkdir -p tmp data
              chmod 1777 tmp
            '';
            entrypoint = [ "${package}/bin/garmin-hass" ];
            workingDir = "/data";
            volumes = [ "/data" ];
            exposedPorts = [ "8099/tcp" ];
            env = [
              "HOME=/data"
              "TMPDIR=/tmp"
              "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              "GARMIN_TOOLKIT_HASS_ADDRESS=0.0.0.0:8099"
              "GARMIN_TOOLKIT_HASS_DEVICE_SOURCE=raw-mtp"
              "GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY=172.30.32.2"
            ];
            labels = {
              "io.hass.version" = appVersion;
              "io.hass.type" = "app";
              "io.hass.arch" = appArch;
            };
            annotations = {
              "org.opencontainers.image.source" = "https://github.com/kubijo/garmin-toolkit";
              "org.opencontainers.image.revision" = sourceRevision;
              "org.opencontainers.image.version" = appVersion;
              "org.opencontainers.image.licenses" =
                (fromTOML (readFile (workspaceSrc + "/Cargo.toml"))).workspace.package.license;
            };
          };
        in
        {
          inherit
            appImage
            demoPackage
            package
            presentation
            sourceMetadata
            webPackage
            ;
          check = pkgs.runCommandLocal "check-garmin-toolkit-hass" { } ''
            test -x ${package}/bin/garmin-hass
            test -x ${demoPackage}/bin/garmin-hass
            test -x ${package}/bin/garmin-gpx-worker
            test -x ${demoPackage}/bin/garmin-gpx-worker
            test -f ${package}/share/garmin-hass/icon.png
            test -f ${package}/share/garmin-hass/logo.png
            test -f ${demoPackage}/share/garmin-hass/icon.png
            test -f ${demoPackage}/share/garmin-hass/logo.png
            test -f ${package}/share/garmin-hass/web/index.html
            find ${package}/share/garmin-hass/web -maxdepth 1 -name '*.js' -print -quit | grep -q .
            find ${package}/share/garmin-hass/web -maxdepth 1 -name '*.wasm' -print -quit | grep -q .
            find ${package}/share/garmin-hass/web -maxdepth 1 -name '*.svg' -print -quit | grep -q .
            find ${demoPackage}/share/garmin-hass/web -maxdepth 1 -name '*.wasm' -print -quit | grep -q .
            touch "$out"
          '';
          devShell = wasmCraneLib.devShell {
            CARGO_TARGET_DIR = sharedCargoTargetDir;
            checks = { inherit package; };
            packages = [
              nodejs
              pkgs.esbuild
              pkgs.trunk
              pkgs.wasm-bindgen-cli_0_2_126
              pkgs.just
            ];
          };
        };
    };
}
