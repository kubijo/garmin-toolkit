{
  craneLib,
  lib,
  pkgs,
  toolchain,
  workspaceSrc,
}:

let
  carbonRevision = "7518c84ffd00f22434fe19d83119692c12fccb2f";
  flagIconsRevision = "7aa5b2bdddd570ece62c812c0cb588ccdc099e2e";
  notoSansRevision = "c4a321e123e4d4ff315f57f4e0adf294fe3a95be";
  notoFallbackRevision = "ffebf8c1ee449e544955a7e813c54f9b73848eac";
  phosphorRevision = "7790ae563ef83ac36094b15b5e109d89fef09337";
  carbonLicense = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/carbon-design-system/carbon/${carbonRevision}/LICENSE";
    hash = "sha256-A3s+nq/1FHeycOkIL1S5kXXk3Yk1SGVPAJlQRbaClLg=";
  };
  phosphorLicense = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/phosphor-icons/core/${phosphorRevision}/LICENSE";
    hash = "sha256-tbHx2hEtGOohR97P1I3cG/K1rrbCZTgVeTQOlbFaK7I=";
  };
  flagIconsLicense = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/lipis/flag-icons/${flagIconsRevision}/LICENSE";
    hash = "sha256-jxGV1Vov0xWgfYEjKEcMqboqu3jI0xf/GWGdUSXgDOo=";
  };
  notoSansLicense = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/notofonts/latin-greek-cyrillic/${notoSansRevision}/OFL.txt";
    hash = "sha256-zumJL58MyP6ILJ6VN+5qiWIdhu586vcLAuKyscJcBho=";
  };
  notoFallbackLicense = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/notofonts/noto-fonts/${notoFallbackRevision}/LICENSE";
    hash = "sha256-DauS0FRPeyM0A/FLhKZjvb+nRpgu2mKef0+f/hsDb+s=";
  };
  config = (pkgs.formats.json { }).generate "garmin-toolkit-license-config.json" {
    assets = [
      {
        name = "carbon-colors";
        version = "11.57.0";
        license = "Apache-2.0";
        license_file = carbonLicense;
        source = "https://github.com/carbon-design-system/carbon/tree/${carbonRevision}";
        targets = [
          "desktop"
          "hass"
        ];
      }
      {
        name = "carbon-themes";
        version = "11.80.0";
        license = "Apache-2.0";
        license_file = carbonLicense;
        source = "https://github.com/carbon-design-system/carbon/tree/${carbonRevision}";
        targets = [
          "desktop"
          "hass"
        ];
      }
      {
        name = "flag-icons";
        version = "7.5.0";
        license = "MIT";
        license_file = flagIconsLicense;
        source = "https://github.com/lipis/flag-icons/tree/${flagIconsRevision}";
        targets = [
          "desktop"
          "hass"
        ];
      }
      {
        name = "noto-sans";
        version = "2.015";
        license = "OFL-1.1";
        license_file = notoSansLicense;
        source = "https://github.com/notofonts/latin-greek-cyrillic/releases/tag/NotoSans-v2.015";
        targets = [
          "desktop"
          "hass"
        ];
      }
      {
        name = "noto-sans-arabic";
        version = "2.009";
        license = "OFL-1.1";
        license_file = notoFallbackLicense;
        source = "https://github.com/notofonts/noto-fonts/tree/${notoFallbackRevision}/hinted/ttf/NotoSansArabic";
        targets = [
          "desktop"
          "hass"
        ];
      }
      {
        name = "noto-sans-tifinagh";
        version = "2.002";
        license = "OFL-1.1";
        license_file = notoFallbackLicense;
        source = "https://github.com/notofonts/noto-fonts/tree/${notoFallbackRevision}/hinted/ttf/NotoSansTifinagh";
        targets = [
          "desktop"
          "hass"
        ];
      }
      {
        name = "phosphor-icons";
        version = "2.1.1";
        license = "MIT";
        license_file = phosphorLicense;
        source = "https://github.com/phosphor-icons/core/tree/${phosphorRevision}";
        targets = [
          "desktop"
          "hass"
        ];
      }
    ];
    targets = [
      {
        name = "desktop";
        package = "garmin-desktop";
        triples = [
          "aarch64-apple-darwin"
          "x86_64-unknown-linux-gnu"
        ];
      }
      {
        name = "hass";
        package = "garmin-hass";
        triples = [ "aarch64-unknown-linux-gnu" ];
      }
    ];
  };
  generator = workspaceSrc + "/infra/licenses/generate.py";
  testCommand = "${lib.getExe pkgs.python3} -m unittest discover -s infra/licenses -p 'test_*.py'";
  generateCommand =
    arguments: "${lib.getExe pkgs.python3} ${generator} --config ${config} ${arguments}";
  runtimeInputs = [
    pkgs.cargo-bundle-licenses
    pkgs.python3
    toolchain
  ];
  app = pkgs.writeShellApplication {
    name = "licenses";
    inherit runtimeInputs;
    text = ''
      ${generateCommand ''"$@"''}
    '';
  };
  checker = pkgs.writeShellApplication {
    name = "license-bundles-check";
    inherit runtimeInputs;
    text = ''
      ${testCommand}
      ${generateCommand "--check"}
    '';
  };
  src = lib.fileset.toSource {
    root = workspaceSrc;
    fileset = lib.fileset.unions [
      (craneLib.fileset.commonCargoSources workspaceSrc)
      # Path dependencies are not Cargo registry downloads: keep their notices in
      # the sandbox source, just as in the build source closure.
      (workspaceSrc + "/vendor/fast-mvt")
      (workspaceSrc + "/vendor/winit")
      (workspaceSrc + "/infra/licenses")
      (lib.fileset.maybeMissing (workspaceSrc + "/assets/licenses"))
    ];
  };
  check = craneLib.mkCargoDerivation {
    pname = "garmin-toolkit-license-bundles";
    version = "0.0.0";
    inherit src;
    cargoArtifacts = null;
    cargoLock = workspaceSrc + "/Cargo.lock";
    doInstallCargoArtifacts = false;
    nativeBuildInputs = [
      pkgs.cargo-bundle-licenses
      pkgs.python3
    ];
    CARGO_NET_OFFLINE = "true";
    buildPhaseCargoCommand = ''
      ${testCommand}
      ${generateCommand "--check"}
    '';
    installPhaseCommand = ''
      touch "$out"
    '';
  };
in
{
  inherit app check checker;
}
