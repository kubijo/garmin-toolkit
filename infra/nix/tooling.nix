{
  lib,
  nix-tools,
  nodejs,
  pkgs,
  pythonToolsEnv,
  system,
  toolchain,
  workspaceSrc,
}:
let
  pythonConfig = ../../ruff.toml;
  sqlFluffConfig = ../sqlfluff/pyproject.toml;
  allFormatters = {
    # The pinned formatter set has no WGSL formatter; Naga validates this shader when WGPU builds it.
    exclude = [
      "crates/garmin-ui/src/activity/gpu_map.wgsl"
      "crates/garmin-ui/src/activity/map_composition.wgsl"
      "infra/javascript/fixtures/*.pbf.hex"
      "infra/javascript/fixtures/*.pbf"
      # Biome's HTML parser rewrites Askama block delimiters;
      # Askama compiles these templates.
      "crates/garmin-diagnostics/templates/*.html"
      # Jinja owns the build report's template syntax.
      "infra/python/templates/*.html"
    ];
    html = true;
    javascript = true;
    typescript = true;
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
      "infra/fixtures/fit/development-activities/LICENSE-DogWalkGPS"
      "infra/fixtures/fit/development-activities/recordings/**"
      "old/**"
      "infra/gallery/fonts/**"
      # Preserve upstream formatting and tooling conventions in vendored dependencies.
      "vendor/**"
    ];
    format = allFormatters;
    inherit nodejs;
    outdated = {
      uv.projects.tooling.root = "infra/python";
      githubActions = true;
    };
    lint = {
      basedpyright.projects.tooling = {
        configFile = "infra/python/pyproject.toml";
        python = pythonToolsEnv;
        reporter = "rich";
      };
      deptry.projects.tooling = {
        root = ".";
        configFile = "infra/python/pyproject.toml";
        sourceRoots = [
          "infra/python"
          "infra/licenses"
          "infra/fixtures/fit/development-activities"
        ];
      };
      grit.profiles = {
        extractable-messages = {
          patterns = ../grit/extractable-messages;
          paths = [
            "apps"
            "crates"
          ];
        };
        presentation-inputs = {
          patterns = ../grit/presentation-inputs;
          paths = [ "crates/garmin-ui/src" ];
        };
        map-client-errors = {
          patterns = ../grit/map-client-errors;
          paths = [
            "apps/garmin-desktop/src/view"
            "apps/garmin-hass/web/src"
          ];
        };
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
      typescript = true;
      extraProjectCheckers = {
        hass-types.command = pkgs.writeShellScript "hass-types" ''
          exec ${pkgs.typescript}/bin/tsc --project infra/integration/hass/tsconfig.json
        '';
        integration-harness-tests.command = pkgs.writeShellScript "integration-harness-tests" ''
          exec ${lib.getExe nodejs} infra/integration/hass/faults.test.mjs
        '';
        map-worker-tests.command = pkgs.writeShellScript "map-worker-tests" ''
          ${lib.getExe nodejs} infra/javascript/test-directory.test.mjs || exit $?
          ESBUILD=${lib.getExe pkgs.esbuild} ${lib.getExe nodejs} infra/javascript/fingerprint-web.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/map-worker.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/map-composition.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/ui-automation.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/diagnostics-view.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/logging.test.mjs || exit $?
          exec ${lib.getExe nodejs} infra/javascript/initializer.test.mjs
        '';
        python-lock.command = pkgs.writeShellScript "python-lock-check" ''
          exec ${lib.getExe pkgs.uv} lock --check --offline \
            --python ${pythonToolsEnv}/bin/python \
            --project infra/python
        '';
        python-tests.command = pkgs.writeShellScript "python-tests" ''
          export PATH=${
            lib.makeBinPath [
              pkgs.bash
              pkgs.just
            ]
          }:$PATH
          exec ${pythonToolsEnv}/bin/python -m unittest discover -q \
            --start-directory infra/python \
            --pattern 'test_*.py'
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
