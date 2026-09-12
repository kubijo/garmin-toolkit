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
          nixCargoTargetDir,
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
          commonArgs = {
            inherit src;
            CARGO_TARGET_DIR = "target";
            cargoLock = workspaceSrc + "/Cargo.lock";
            doCheck = false;
            strictDeps = true;
          };
          nativeArgs = commonArgs // {
            buildInputs = [ pkgs.glib ];
            nativeBuildInputs = [ pkgs.pkg-config ];
          };
          cargoArtifacts = craneLib.buildDepsOnly (
            nativeArgs
            // {
              cargoExtraArgs = "-p garmin-hass --all-features";
              pname = "garmin-hass-deps";
            }
          );
          webArgs = commonArgs // {
            CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
            cargoExtraArgs = "-p garmin-hass-web";
          };
          webCargoArtifacts = wasmCraneLib.buildDepsOnly (
            webArgs
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
              buildPhaseCargoCommand = ''
                ( cd apps/garmin-hass/web && trunk build --release=true index.html )
              '';
              installPhaseCommand = ''
                webRoot="$out/share/garmin-hass/web"
                mkdir -p "$webRoot"
                cp -r apps/garmin-hass/web/dist/. "$webRoot/"
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
              cargoExtraArgs = "-p garmin-hass" + lib.optionalString demo " --features demo";
            in
            craneLib.buildPackage (
              nativeArgs
              // {
                inherit cargoExtraArgs;
                pname = "garmin-hass" + lib.optionalString demo "-demo";
                meta.mainProgram = "garmin-hass";
                inherit cargoArtifacts;
                nativeBuildInputs = [
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
                    --prefix GIO_EXTRA_MODULES : "${pkgs.gvfs}/lib/gio/modules" \
                    --prefix LD_LIBRARY_PATH : "${
                      lib.makeLibraryPath [
                        pkgs.glib
                        pkgs.gvfs
                      ]
                    }"
                '';
              }
            );
          package = mkPackage { demo = false; };
          demoPackage = mkPackage { demo = true; };
        in
        {
          inherit
            demoPackage
            package
            presentation
            webPackage
            ;
          check = pkgs.runCommandLocal "check-garmin-toolkit-hass" { } ''
            test -x ${package}/bin/garmin-hass
            test -x ${demoPackage}/bin/garmin-hass
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
          devShell = craneLib.devShell {
            CARGO_TARGET_DIR = nixCargoTargetDir;
            checks = { inherit package; };
          };
        };
    };
}
