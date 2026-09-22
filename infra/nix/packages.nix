{
  crane,
  lib,
  pkgs,
  toolchain,
  workspaceSrc,
  ...
}:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
  runtimeLibraries = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
    pkgs.glib
    pkgs.gvfs
    pkgs.udev
  ];
  src = import ./cargo-source.nix {
    inherit
      craneLib
      lib
      workspaceSrc
      ;
  };
  commonArgs = {
    inherit src;
    pname = "garmin-cli";
    version = "0.1.0";
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.pkg-config
    ]
    ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [
      pkgs.autoPatchelfHook
      pkgs.wrapGAppsNoGuiHook
    ];
    buildInputs = runtimeLibraries;
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
  }
  // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
    LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibraries;
  };
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
  garminCli = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoExtraArgs = "--package garmin-cli";
      preFixup = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
        gappsWrapperArgs+=(
          --set-default SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
        )
      '';
    }
  );
in
{
  inherit garminCli;
  checks.build = garminCli;
}
