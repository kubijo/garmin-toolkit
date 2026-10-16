{
  appImage,
  dirspec,
  package,
  lib,
  pkgs,
  pythonToolsEnv,
  workspaceSrc,
}:
let
  closure = pkgs.closureInfo {
    rootPaths = [
      package
      pkgs.cacert
    ];
  };
  source =
    paths:
    lib.fileset.toSource {
      root = workspaceSrc;
      fileset = lib.fileset.unions (map (path: workspaceSrc + "/${path}") paths);
    };
  inventoryModules = map (name: "infra/python/${name}.py") [
    "archive_io"
    "json_data"
    "haos_platform"
    "hass_image_inventory"
    "hass_image_sbom"
  ];
  inventorySource = source (
    inventoryModules
    ++ [
      "infra/release/syft.yaml"
      "assets/licenses/bundle-hass.json"
    ]
  );
  releaseSource = source (
    inventoryModules
    ++ (map (name: "infra/python/${name}.py") [
      "cli_output"
      "hass_release"
      "hass_release_build"
      "hass_release_source"
      "hass_release_contract"
    ])
    ++ (map (name: "infra/release/${name}") [
      "hass-release.schema.json"
      "hass-files.toml"
      "SHA256SUMS.j2"
      "hass-release.dirspec.toml"
      "hass-native-build.dirspec.toml"
    ])
  );
  sbom =
    pkgs.runCommand "garmin-hass-image.spdx.json"
      {
        nativeBuildInputs = [
          pythonToolsEnv
          pkgs.syft
        ];
      }
      ''
        python ${inventorySource}/infra/python/hass_image_sbom.py \
          --archive ${appImage} \
          --closure ${closure}/store-paths \
          --bundle ${inventorySource}/assets/licenses/bundle-hass.json \
          --output "$out" \
          --created "''${SOURCE_DATE_EPOCH:-1}"
        test -s "$out"
      '';
  app = pkgs.writeShellApplication {
    name = "hass-release";
    runtimeInputs = [
      pythonToolsEnv
      pkgs.git
      pkgs.nix
      dirspec
    ];
    text = ''
      exec python ${releaseSource}/infra/python/hass_release.py "$@"
    '';
  };
in
{
  inherit app sbom;
}
