{
  lib,
  nix-tools,
  pkgs,
  pythonToolsEnv,
  system,
  toolchain,
  workspaceSrc,
}:
let
  pythonConfig = ../python/pyproject.toml;
  sqlFluffConfig = ../sqlfluff/pyproject.toml;
  allFormatters = {
    # The pinned formatter set has no WGSL formatter; Naga validates this shader when WGPU builds it.
    exclude = [
      "crates/garmin-ui/src/activity/gpu_map.wgsl"
      "infra/javascript/fixtures/*.pbf.hex"
      "infra/javascript/fixtures/*.pbf"
    ];
    html = true;
    javascript = true;
    json = true;
    justfile = true;
    markdown = true;
    nix = true;
    python.configFile = pythonConfig;
    rust.exe = lib.getExe' toolchain "rustfmt";
    shell = true;
    sql.configFile = sqlFluffConfig;
    svg = true;
    toml = true;
    yaml = true;
    xml = true;
  };

  project = nix-tools.lib.configure {
    inherit system;
    src = workspaceSrc;
    exclude = [
      ".python-version"
      "LICENSE-AGPL"
      "LICENSE-APACHE"
      "LICENSE-MIT"
      "assets/licenses/**"
      "crates/garmin-ui/assets/fonts/**"
      "infra/fixtures/fit/development-activities/LICENSE"
      "infra/fixtures/fit/development-activities/recordings/**"
      "old/**"
      "infra/gallery/fonts/**"
      # Preserve upstream formatting and tooling conventions in vendored dependencies.
      "vendor/**"
    ];
    format = allFormatters;
    inherit (pkgs) nodejs;
    lint = {
      grit.profiles = {
        postcard-compatible-models = {
          patterns = ../grit/postcard-compatible-models;
          paths = [ "crates/garmin-model" ];
        };
        reviewed-service-models = {
          patterns = ../grit/reviewed-service-models;
          paths = [ "crates/garmin-service-api" ];
        };
        separate-self-imports = {
          patterns = ../grit/separate-self-imports;
          paths = [ "crates" ];
        };
      };
      links = {
        enable = true;
        ignoreLinks = [ "^https?://" ];
      };
      nix = true;
      python.configFile = pythonConfig;
      extraProjectCheckers = {
        map-worker-tests.command = pkgs.writeShellScript "map-worker-tests" ''
          ${lib.getExe pkgs.nodejs} infra/javascript/map-worker.test.mjs || exit $?
          exec ${lib.getExe pkgs.nodejs} infra/javascript/initializer.test.mjs
        '';
        python-lock.command = pkgs.writeShellScript "python-lock-check" ''
          exec ${lib.getExe pkgs.uv} lock --check --offline \
            --python ${pythonToolsEnv}/bin/python \
            --project infra/python
        '';
        python-tests.command = pkgs.writeShellScript "python-tests" ''
          exec ${pythonToolsEnv}/bin/python -m unittest discover -q \
            --start-directory infra/python \
            --pattern 'test_*.py'
        '';
        python-types.command = pkgs.writeShellScript "python-types" ''
          exec ${lib.getExe pkgs.ty} check \
            --project infra/python \
            --python ${pythonToolsEnv} \
            infra/python
        '';
      };
      shell = true;
      sql.configFile = sqlFluffConfig;
      workflows = true;
      xml = true;
      yaml = true;
    };
  };
in
{
  inherit (project)
    apps
    checks
    formatter
    packages
    ;
}
