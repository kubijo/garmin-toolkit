{
  crane,
  dirspec,
  formatjsCli,
  lib,
  pkgs,
  toolchain,
  workspaceSrc,
  ...
}:
let
  inherit (builtins) fromTOML readFile;

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
    version = (fromTOML (readFile (workspaceSrc + "/Cargo.toml"))).workspace.package.version;
    cargoExtraArgs = "--locked --package garmin-cli";
    strictDeps = true;
    nativeBuildInputs = [
      formatjsCli
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
  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs // import ./cargo-deps.nix { inherit lib workspaceSrc; }
  );
  packageSpec = workspaceSrc + "/infra/nix/garmin-cli.dirspec.toml";
  garminCli = craneLib.buildPackage (
    commonArgs
    // {
      inherit cargoArtifacts;
      nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ dirspec ];
      preFixup = lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
        gappsWrapperArgs+=(
          --set-default SSL_CERT_FILE "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
        )
      '';
      postFixup = ''
        dirspec "$out" --spec ${packageSpec}
      '';
    }
  );
in
{
  inherit garminCli;
  checks.build = garminCli;
}
