{
  appImage,
  brandAssets,
  dirspec,
  package,
  lib,
  nodejs,
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
      "hass_attestations"
      "hass_release_build"
      "hass_release_source"
      "hass_release_contract"
      "hass_registry"
      "hass_repository"
      "hass_sources"
      "hass_source_parts"
      "hass_store"
      "hass_publish"
      "hass_preflight"
      "hass_ci"
      "hass_recovery"
      "hass_recovery_git"
      "processes"
    ])
    ++ (map (name: "infra/release/${name}") [
      "hass-release.schema.json"
      "hass-files.toml"
      "SHA256SUMS.j2"
      "hass-release.dirspec.toml"
      "hass-preflight.dirspec.toml"
      "hass-native-build.dirspec.toml"
      "hass-registry.dirspec.toml"
      "hass-sources.dirspec.toml"
      "hass-app-repository.dirspec.toml"
    ])
  );
  repositorySource = source (
    (map (name: "infra/python/${name}.py") [
      "hass_repository"
      "hass_release_contract"
      "json_data"
    ])
    ++ [
      "Cargo.toml"
      "infra/release/hass-app-repository.dirspec.toml"
    ]
    ++ (map (name: "apps/garmin-hass/addon/${name}") [
      "repository.yaml"
      "config.yaml"
      "README.md"
      "DOCS.md"
      "CHANGELOG.md"
    ])
  );
  repositoryFor =
    architectures:
    pkgs.runCommand "garmin-hass-app-repository"
      {
        nativeBuildInputs = [
          pythonToolsEnv
          dirspec
        ];
      }
      ''
        python ${repositorySource}/infra/python/hass_repository.py \
          --source ${repositorySource} \
          --branding ${brandAssets} \
          --architectures ${lib.escapeShellArgs architectures} \
          --output "$out"
      '';
  repository = repositoryFor [ "amd64" ];
  arm64TestRepository = repositoryFor [ "aarch64" ];
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
  releaseApp =
    name: module: extraInputs:
    pkgs.writeShellApplication {
      inherit name;
      runtimeInputs = [
        pythonToolsEnv
        pkgs.git
        pkgs.gh
        pkgs.nix
        pkgs.skopeo
        dirspec
      ]
      ++ extraInputs;
      text = ''
        exec python ${releaseSource}/infra/python/${module}.py "$@"
      '';
    };
in
{
  inherit repository arm64TestRepository sbom;
  app = releaseApp "hass-release" "hass_release" [ ];
  ci = releaseApp "hass-ci" "hass_ci" [ nodejs ];
  recovery = releaseApp "hass-recovery" "hass_recovery" [ ];
}
