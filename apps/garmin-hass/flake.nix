{
  description = "Garmin Toolkit Home Assistant target";

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
        }:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          inherit (pkgs) lib;
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          wasmCraneLib = (crane.mkLib pkgs).overrideToolchain wasmToolchain;
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
                pkgs.esbuild
              ];

              buildPhaseCargoCommand = ''
                ( cd apps/garmin-hass/web && trunk build --release=true index.html )
              '';

              postBuild = ''
                export GARMIN_TEST_WEB_ROOT="$PWD/apps/garmin-hass/web/dist"
                for test in infra/javascript/*.test.mjs; do
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
                inherit cargoArtifacts;
                nativeBuildInputs = nativeArgs.nativeBuildInputs ++ [
                  dirspec
                  pkgs.makeWrapper
                  pkgs.pkg-config
                ];
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
          appVersion =
            (builtins.fromTOML (builtins.readFile (workspaceSrc + "/Cargo.toml"))).workspace.package.version;
          appImage = pkgs.dockerTools.buildLayeredImage {
            name = "ghcr.io/kubijo/garmin-toolkit-hass";
            tag = appVersion;
            contents = [
              package
              pkgs.cacert
            ];
            extraCommands = ''
              mkdir -p tmp data
              chmod 1777 tmp
            '';
            config = {
              Entrypoint = [ "${package}/bin/garmin-hass" ];
              WorkingDir = "/data";
              Volumes."/data" = { };
              ExposedPorts."8099/tcp" = { };
              Env = [
                "HOME=/data"
                "TMPDIR=/tmp"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "GARMIN_TOOLKIT_HASS_ADDRESS=0.0.0.0:8099"
                "GARMIN_TOOLKIT_HASS_DEVICE_SOURCE=raw-mtp"
                "GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY=172.30.32.2"
              ];
              Labels = {
                "io.hass.version" = appVersion;
                "io.hass.type" = "app";
                "io.hass.arch" = appArch;
              };
            };
          };
        in
        {
          inherit
            appImage
            demoPackage
            package
            presentation
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
