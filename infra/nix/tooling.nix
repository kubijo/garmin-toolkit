{
  dirspec,
  lib,
  nix-tools,
  nixOci,
  nodejs,
  pkgs,
  pythonToolsEnv,
  src,
  system,
  toolchain,
}:
let
  browserTypes = pkgs.importNpmLock.buildNodeModules {
    npmRoot = ../javascript;
    inherit nodejs;
    derivationArgs.npmRebuildFlags = [ "--ignore-scripts" ];
  };
  pythonConfig = ../../ruff.toml;
  sqlFluffConfig = ../sqlfluff/pyproject.toml;
  sqlfluff = pkgs.sqlfluff.override {
    python3Packages = pkgs.python3Packages.overrideScope (
      _final: prev: {
        diff-cover = prev.diff-cover.overridePythonAttrs (
          old:
          assert lib.assertMsg (old.version == "10.5.1" && prev.pygments.version == "2.21.0")
            "Revisit the diff-cover HTML fixture exception after dependency updates; see docs/architecture/build-system.md";
          {
            # Published sources remain unchanged. Only these six HTML fixture comparisons
            # disagree with Pygments 2.21; all other package tests and SQL checks still run.
            pytestFlags =
              (old.pytestFlags or [ ])
              ++ map (test: "--deselect=tests/test_integration.py::TestDiffQualityIntegration::${test}") [
                "test_added_file_pycodestyle_html"
                "test_all_reports"
                "test_added_file_pyflakes_html"
                "test_added_file_pylint_html"
                "test_fail_under_html"
                "test_fail_under_pass_html"
              ];
          }
        );
      }
    );
  };
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
      # The plain-text checksum template is rendered and checked by the release tests.
      "infra/release/*.j2"
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
    inherit src system;
    toolPkgs = pkgs // {
      inherit sqlfluff;
    };
    exclude = [
      ".python-version"
      "LICENSE-AGPL"
      "LICENSE-APACHE"
      "LICENSE-MIT"
      "assets/licenses/**"
      # Preserve the exact upstream text checked by the notice manifest's hashes.
      "infra/licenses/notices/**"
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
        dirspec-prescriptions.command = pkgs.writeShellScript "check-dirspec-prescriptions" ''
          set -eu
          empty_tree="$(mktemp -d)"
          trap 'rm -rf "$empty_tree"' EXIT
          for spec in \
            apps/garmin-desktop/*.dirspec.toml \
            apps/garmin-hass/*.dirspec.toml \
            infra/gallery/*.dirspec.toml \
            infra/release/*.dirspec.toml \
            infra/nix/*.dirspec.toml; do
            test -f "$spec"
            if ${lib.getExe dirspec} "$empty_tree" --spec "$spec" >/dev/null; then
              :
            else
              result=$?
              # Missing artifact paths are expected; configuration errors exit 2.
              test "$result" -eq 1 || exit "$result"
            fi
          done
        '';
        hass-types.command = pkgs.writeShellScript "hass-types" ''
          ${pkgs.typescript}/bin/tsc --project infra/integration/hass/tsconfig.json || exit $?
          exec ${pkgs.bash}/bin/bash infra/just/check-browser-types.sh \
            ${pkgs.typescript}/bin/tsc ${browserTypes}/node_modules
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
          ${lib.getExe nodejs} infra/javascript/initializer.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/loader-bootstrap.test.mjs || exit $?
          ${lib.getExe nodejs} infra/javascript/haos-install-links.test.mts --documentation || exit $?
          ${lib.getExe nodejs} infra/javascript/haos-workflow.test.mts || exit $?
          exec ${lib.getExe nodejs} infra/javascript/haos-devtools.test.mts
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
              pkgs.coreutils
              pkgs.git
              pkgs.gh
              pkgs.just
              pkgs.syft
              pkgs.skopeo
              pkgs.distribution
              nixOci
              dirspec
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
