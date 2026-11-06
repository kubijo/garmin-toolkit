{
  appImageRuntime,
  craneLib,
  launcher,
  lib,
  demoIdentity,
  demoLauncher,
  demoPackage,
  mkAppImage,
  package,
  pkgs,
  productionIdentity,
  src,
  withHostGraphics,
}:

let
  flatpakRuntimeVersion = "25.08";

  arch =
    if pkgs.stdenv.hostPlatform.isx86_64 then
      "x86_64"
    else if pkgs.stdenv.hostPlatform.isAarch64 then
      "aarch64"
    else
      throw "desktop distribution supports only x86_64-linux and aarch64-linux";

  updateInfo =
    identity:
    "gh-releases-zsync|kubijo|garmin-toolkit|latest-all|${identity.slug}-${arch}.AppImage.zsync";

  updateRuntime =
    identity:
    pkgs.runCommand "${identity.slug}-update-runtime"
      {
        nativeBuildInputs = [ pkgs.binutils ];
      }
      ''
        cp ${appImageRuntime} "$out"
        chmod u+w "$out"
        printf '%s' ${lib.escapeShellArg (updateInfo identity)} > update-info
        test "$(stat -c %s update-info)" -lt 1024
        truncate -s 1024 update-info
        objcopy --update-section .upd_info=update-info "$out"
        # objcopy may reset the ELF padding used for the AppImage type marker.
        printf '\101\111\002' | dd of="$out" bs=1 seek=8 conv=notrunc status=none
      '';
  productionRuntime = updateRuntime productionIdentity;
  demoRuntime = updateRuntime demoIdentity;

  appImage = (mkAppImage.override { mkappimage-runtime = productionRuntime; }) {
    program = lib.getExe (withHostGraphics {
      inherit package;
    });
    name = "${productionIdentity.slug}-${arch}.AppImage";
  };

  demoAppImage = (mkAppImage.override { mkappimage-runtime = demoRuntime; }) {
    program = lib.getExe (withHostGraphics {
      package = demoPackage;
    });
    name = "${demoIdentity.slug}-${arch}.AppImage";
  };

  mkZsync =
    identity: image:
    pkgs.runCommand "${identity.slug}-${arch}.AppImage.zsync"
      {
        nativeBuildInputs = [ pkgs.zsync ];
      }
      ''
        zsyncmake -f ${identity.slug}-${arch}.AppImage \
          -u https://github.com/kubijo/garmin-toolkit/releases/download/v${package.version}/${identity.slug}-${arch}.AppImage \
          -o "$out" ${image}
      '';
  appImageZsync = mkZsync productionIdentity appImage;
  demoAppImageZsync = mkZsync demoIdentity demoAppImage;

  releaseAssets = pkgs.runCommand "garmin-desktop-release-assets-${arch}" { } ''
    mkdir -p "$out"
    cp ${appImage} "$out/${productionIdentity.slug}-${arch}.AppImage"
    cp ${appImageZsync} "$out/${productionIdentity.slug}-${arch}.AppImage.zsync"
    cp ${demoAppImage} "$out/${demoIdentity.slug}-${arch}.AppImage"
    cp ${demoAppImageZsync} "$out/${demoIdentity.slug}-${arch}.AppImage.zsync"
  '';

  updateMetadataCheck =
    pkgs.runCommand "check-desktop-update-metadata"
      {
        nativeBuildInputs = [ pkgs.zsync ];
      }
      ''
        test "$(${productionRuntime} --appimage-updateinformation)" = ${lib.escapeShellArg (updateInfo productionIdentity)}
        test "$(${demoRuntime} --appimage-updateinformation)" = ${lib.escapeShellArg (updateInfo demoIdentity)}
        for runtime in ${productionRuntime} ${demoRuntime}; do
          test "$("$runtime" --appimage-offset)" = "$(stat -c %s "$runtime")"
        done
        zsync -i ${productionRuntime} -o restored-runtime ${mkZsync productionIdentity productionRuntime}
        cmp ${productionRuntime} restored-runtime
        touch "$out"
      '';

  mkAppImageExporter =
    {
      appImage,
      identity,
      zsync,
    }:
    pkgs.writeShellApplication {
      name = "export-${identity.slug}-appimage";
      runtimeInputs = [ pkgs.coreutils ];
      text = ''
        mkdir -p dist
        install -Dm755 ${appImage} "dist/${identity.slug}-${arch}.AppImage"
        install -Dm644 ${zsync} "dist/${identity.slug}-${arch}.AppImage.zsync"
        printf 'wrote %s\n' "dist/${identity.slug}-${arch}.AppImage"
      '';
    };

  appImageExporter = mkAppImageExporter {
    inherit appImage;
    zsync = appImageZsync;
    identity = productionIdentity;
  };

  demoAppImageExporter = mkAppImageExporter {
    appImage = demoAppImage;
    zsync = demoAppImageZsync;
    identity = demoIdentity;
  };

  buildDir = "/run/build/garmin-desktop";
  vendor = craneLib.vendorCargoDeps { inherit src; };
  flatpakVendor = pkgs.runCommandLocal "garmin-toolkit-flatpak-cargo-vendor" { } ''
    cp -rL ${vendor} "$out"
    chmod -R u+w "$out"
    substituteInPlace "$out/config.toml" --replace-fail "${vendor}" "${buildDir}/vendor"
  '';
  yamlFormat = pkgs.formats.yaml { };

  mkFlatpakStage =
    {
      identity,
      launcher,
      demo,
    }:
    let
      cargoCommand =
        "cargo --offline build --locked --release -p garmin-desktop -p garmin-gpx-worker"
        + lib.optionalString demo " --features garmin-desktop/demo";

      manifest = yamlFormat.generate "${identity.id}.yml" {
        app-id = identity.id;
        runtime = "org.freedesktop.Platform";
        runtime-version = flatpakRuntimeVersion;
        sdk = "org.freedesktop.Sdk";
        sdk-extensions = [ "org.freedesktop.Sdk.Extension.rust-stable" ];
        command = "garmin-desktop";
        finish-args = [
          "--device=all"
          "--filesystem=xdg-run/gvfs:ro"
          "--share=ipc"
          "--share=network"
          "--socket=fallback-x11"
          "--socket=wayland"
          "--talk-name=org.gtk.vfs.*"
        ];

        modules = [
          {
            name = "garmin-desktop";
            buildsystem = "simple";
            build-options = {
              append-path = "/usr/lib/sdk/rust-stable/bin";

              env = {
                CARGO_HOME = "${buildDir}/cargo";
                CARGO_TARGET_DIR = "target";
                SQLX_OFFLINE = "true";
              };
            };

            build-commands = [
              "mkdir -p cargo && cp vendor/config.toml cargo/config.toml"
              cargoCommand
              "install -Dm755 target/release/garmin-desktop /app/bin/garmin-desktop"
              "install -Dm755 target/release/garmin-gpx-worker /app/bin/garmin-gpx-worker"
              "mkdir -p /app/share && cp -r launcher/share/. /app/share/"
            ];

            sources = [
              {
                type = "dir";
                path = "src";
              }
              {
                type = "dir";
                path = "launcher";
                dest = "launcher";
              }
              {
                type = "dir";
                path = "vendor";
                dest = "vendor";
              }
            ];
          }
        ];
      };
    in
    pkgs.runCommandLocal "${identity.slug}-flatpak-stage" { } ''
      mkdir "$out"
      cp ${manifest} "$out/${identity.id}.yml"
      ln -s ${src} "$out/src"
      ln -s ${launcher} "$out/launcher"
      ln -s ${flatpakVendor} "$out/vendor"
    '';

  flatpakStage = mkFlatpakStage {
    identity = productionIdentity;
    inherit launcher;
    demo = false;
  };

  demoFlatpakStage = mkFlatpakStage {
    identity = demoIdentity;
    launcher = demoLauncher;
    demo = true;
  };

  mkFlatpakBuilder =
    { identity, stage }:
    pkgs.writeShellApplication {
      name = "build-${identity.slug}-flatpak";
      runtimeInputs = [
        pkgs.appstream
        pkgs.coreutils
        pkgs.flatpak
        pkgs.flatpak-builder
      ];
      text = ''
        runtime="org.freedesktop.Platform//${flatpakRuntimeVersion}"
        sdk="org.freedesktop.Sdk//${flatpakRuntimeVersion}"
        rust="org.freedesktop.Sdk.Extension.rust-stable//${flatpakRuntimeVersion}"
        for dependency in "$runtime" "$sdk" "$rust"; do
          if ! flatpak info "$dependency" >/dev/null 2>&1; then
            printf 'missing Flatpak dependency: %s\n' "$dependency" >&2
            printf 'install with: flatpak install flathub %s\n' "$dependency" >&2
            exit 69
          fi
        done

        work="$PWD/.tmp/flatpak-${identity.slug}"
        destination="$PWD/dist/${identity.id}.flatpak"
        rm -rf -- "$work"
        mkdir -p "$work/stage"
        cp -rL ${stage}/. "$work/stage"
        chmod -R u+w "$work/stage"
        (
          cd "$work/stage"
          flatpak-builder \
            --disable-rofiles-fuse \
            --force-clean \
            --state-dir ../state \
            --repo ../repo \
            ../build \
            ${identity.id}.yml
        )
        mkdir -p "$PWD/dist"
        rm -f -- "$destination"
        flatpak build-bundle "$work/repo" "$destination" ${identity.id}
        printf 'wrote %s\n' "$destination"
      '';
    };

  flatpakBuilder = mkFlatpakBuilder {
    identity = productionIdentity;
    stage = flatpakStage;
  };

  demoFlatpakBuilder = mkFlatpakBuilder {
    identity = demoIdentity;
    stage = demoFlatpakStage;
  };
in
{
  inherit
    appImage
    appImageExporter
    appImageZsync
    demoAppImageZsync
    releaseAssets
    updateMetadataCheck
    flatpakBuilder
    flatpakStage
    demoAppImage
    demoAppImageExporter
    demoFlatpakBuilder
    demoFlatpakStage
    ;
}
