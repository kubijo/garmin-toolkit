{
  craneLib,
  craneLibPortable,
  launcher,
  lib,
  demoIdentity,
  demoLauncher,
  pkgs,
  pkgsPortable,
  productionIdentity,
  src,
  workspaceSrc,
}:

let
  inherit (builtins) readFile;

  version = (fromTOML (readFile (workspaceSrc + "/Cargo.toml"))).workspace.package.version;
  flatpakRuntimeVersion = "25.08";
  arch =
    if pkgs.stdenv.hostPlatform.isx86_64 then
      "x86_64"
    else if pkgs.stdenv.hostPlatform.isAarch64 then
      "aarch64"
    else
      throw "desktop distribution supports only x86_64-linux and aarch64-linux";
  interpreter =
    if pkgs.stdenv.hostPlatform.isx86_64 then
      "/lib64/ld-linux-x86-64.so.2"
    else
      "/lib/ld-linux-aarch64.so.1";
  appImageRuntime = pkgs.fetchurl {
    url = "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-${arch}";
    hash =
      {
        x86_64 = "sha256-L8qLRDySUQ8Ug6iD9gBhrQm0a5eLJjHIB82HOkfsJg0=";
        aarch64 = "sha256-AMvfz5F8xsD/bTNH1Z4Moff0Wm3xpCig1tinhmTYdEQ=";
      }
      .${arch};
  };
  portableBuildInputs = with pkgsPortable; [
    glib
    libGL
    libxkbcommon
    vulkan-loader
    wayland
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXrandr
  ];
  portableCommonArgs = {
    inherit src version;
    CARGO_TARGET_DIR = "target";
    SQLX_OFFLINE = "true";
    buildInputs = portableBuildInputs;
    cargoLock = workspaceSrc + "/Cargo.lock";
    doCheck = false;
    nativeBuildInputs = [ pkgsPortable.pkg-config ];
    strictDeps = true;
  };
  portableCargoArtifacts = craneLibPortable.buildDepsOnly (
    portableCommonArgs
    // {
      cargoExtraArgs = "--locked -p garmin-desktop --all-features";
      pname = "garmin-toolkit-portable-deps";
    }
  );
  mkPortable =
    { identity, demo }:
    let
      cargoExtraArgs = "--locked -p garmin-desktop" + lib.optionalString demo " --features demo";
    in
    craneLibPortable.buildPackage (
      portableCommonArgs
      // {
        inherit cargoExtraArgs;
        cargoArtifacts = portableCargoArtifacts;
        pname = "${identity.slug}-portable";
        nativeBuildInputs = portableCommonArgs.nativeBuildInputs ++ [ pkgsPortable.patchelf ];
        postInstall = ''
          patchelf --set-interpreter ${interpreter} --remove-rpath "$out/bin/garmin-desktop"
        '';
      }
    );
  portable = mkPortable {
    identity = productionIdentity;
    demo = false;
  };
  demoPortable = mkPortable {
    identity = demoIdentity;
    demo = true;
  };
  mkAppImage =
    {
      identity,
      launcher,
      portable,
    }:
    pkgs.runCommand "${identity.slug}-${arch}.AppImage" { nativeBuildInputs = [ pkgs.squashfsTools ]; }
      ''
        mkdir -p AppDir/usr/bin
        cp ${portable}/bin/garmin-desktop AppDir/usr/bin/
        cp -r --no-preserve=mode ${launcher}/share AppDir/usr/
        ln -s usr/bin/garmin-desktop AppDir/AppRun
        cp AppDir/usr/share/applications/${identity.id}.desktop AppDir/
        cp AppDir/usr/share/icons/hicolor/256x256/apps/${identity.id}.png AppDir/
        ln -s ${identity.id}.png AppDir/.DirIcon
        mksquashfs AppDir fs.squashfs -root-owned -noappend -no-progress -comp zstd
        cat ${appImageRuntime} fs.squashfs > "$out"
        chmod +x "$out"
      '';
  appImage = mkAppImage {
    inherit launcher portable;
    identity = productionIdentity;
  };
  demoAppImage = mkAppImage {
    identity = demoIdentity;
    launcher = demoLauncher;
    portable = demoPortable;
  };
  mkAppImageExporter =
    { appImage, identity }:
    pkgs.writeShellApplication {
      name = "export-${identity.slug}-appimage";
      runtimeInputs = [ pkgs.coreutils ];
      text = ''
        mkdir -p dist
        install -Dm755 ${appImage} "dist/${identity.slug}-${arch}.AppImage"
        printf 'wrote %s\n' "dist/${identity.slug}-${arch}.AppImage"
      '';
    };
  appImageExporter = mkAppImageExporter {
    inherit appImage;
    identity = productionIdentity;
  };
  demoAppImageExporter = mkAppImageExporter {
    appImage = demoAppImage;
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
        "cargo --offline build --locked --release -p garmin-desktop"
        + lib.optionalString demo " --features demo";
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
    flatpakBuilder
    flatpakStage
    demoAppImage
    demoAppImageExporter
    demoFlatpakBuilder
    demoFlatpakStage
    demoPortable
    portable
    ;
}
