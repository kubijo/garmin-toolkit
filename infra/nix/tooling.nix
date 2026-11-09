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
  monkeyCGrammarSource = pkgs.fetchFromGitHub {
    owner = "bombsimon";
    repo = "tree-sitter-monkey-c";
    rev = "6454acab6ff46cb6d1ae60b262caab0910c37a4a";
    hash = "sha256-g/UCcg96n/1HaLvFrlg88d0EaolCpjfxN6kmyCb9nVM=";
  };
  wasiCc = pkgs.pkgsCross.wasi32.stdenv.cc;
  zedMonkeyCGrammar =
    pkgs.runCommand "zed-monkey-c-grammar"
      {
        nativeBuildInputs = [
          pkgs.tree-sitter
          pkgs.stdenv.cc
        ];
      }
      ''
        export XDG_CACHE_HOME="$TMPDIR/tree-sitter-cache"
        mkdir -p "$out"
        tree-sitter build --output "$out/monkey_c${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}" ${monkeyCGrammarSource}
        ${lib.getExe' zedWasiSdk "clang"} -fPIC -shared -Os \
          -Wl,--export=tree_sitter_monkey_c -Wl,--max-memory=4294967296 \
          -I${monkeyCGrammarSource}/src ${monkeyCGrammarSource}/src/parser.c -o "$out/monkey_c.wasm"
      '';
  zedWasiSdk = pkgs.writeShellScriptBin "clang" ''
    export PATH=${lib.makeBinPath [ pkgs.llvmPackages.lld ]}:"$PATH"
    # Tree-sitter supplies the grammar's C imports; do not link WASI startup/TLS runtime code.
    exec ${lib.getExe' wasiCc "${wasiCc.targetPrefix}cc"} -nostdlib -Wl,--no-entry,--allow-undefined "$@"
  '';
  rafiki = pkgs.rustPlatform.buildRustPackage {
    pname = "rafiki";
    version = "0.3.0-unstable-2026-10-27";
    src = pkgs.fetchFromGitHub {
      owner = "bombsimon";
      repo = "monkey-c-rs";
      rev = "afdd15d2b8c481a0c25fc74a767b4835a6443305";
      hash = "sha256-hRFejH7y+XiZnTHwlv9/ynUwo3TgrfvGSdi96/FFCwU=";
    };
    cargoHash = "sha256-fFLin6itPkw42CVNwDydGkTzEDU2RXHTDcwgeVvbSN8=";
    cargoBuildFlags = [
      "-p"
      "rafiki"
    ];
    meta = {
      description = "Monkey C formatter, linter and language server";
      homepage = "https://bombsimon.github.io/monkey-c-rs/";
      license = lib.licenses.mit;
      mainProgram = "rafiki";
    };
  };
  browserTypes = pkgs.importNpmLock.buildNodeModules {
    npmRoot = ../javascript;
    inherit nodejs;
    derivationArgs.npmRebuildFlags = [ "--ignore-scripts" ];
  };
  pythonConfig = ../../ruff.toml;
  sqlFluffConfig = ../sqlfluff/pyproject.toml;
  allFormatters = {
    # The pinned formatter set has no WGSL formatter; Naga validates this shader when WGPU builds it.
    exclude = [
      # Jungle build definitions are not Monkey C source.
      "apps/garmin-connect-iq/monkey.jungle"
      # Zed queries are validated against the pinned grammar by the editor tooling check.
      "infra/zed/monkey-c/languages/**/*.scm"
      # The compatibility image is checked by its owned build/library-resolution workflow.
      "infra/connect-iq/Dockerfile"
      "infra/connect-iq/sources.list"
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
      # Preserve byte-exact upstream compatibility inputs and plain license/dependency text.
      "crates/datapackage/tests/upstream/**"
      "crates/datapackage/LICENSE"
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
    extraFormatters.monkey-c = {
      command = lib.getExe rafiki;
      options = [
        "fmt"
        "--no-config"
      ];
      includes = [ "*.mc" ];
      cacheInputs = [ ];
    };
  };

  project = nix-tools.lib.configure {
    inherit src system;
    toolPkgs = pkgs // {
      inherit (pkgs) sqlfluff;
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
      # Byte-exact upstream fixtures, tests, and licenses are checked by their manifest.
      "crates/datapackage/tests/upstream/**"
      "crates/datapackage/LICENSE"
      # Preserve upstream formatting and tooling conventions in vendored dependencies.
      "vendor/**"
    ];
    format = allFormatters;
    inherit nodejs;
    outdated = {
      npm.projects.browser.root = "infra/javascript";
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
        monkey-c-editor.command = pkgs.writeShellScript "monkey-c-editor-check" ''
          set -eu
          for query in infra/zed/monkey-c/languages/monkey-c/*.scm; do
            ${lib.getExe pkgs.tree-sitter} query --quiet \
              --lib-path ${zedMonkeyCGrammar}/monkey_c${pkgs.stdenv.hostPlatform.extensions.sharedLibrary} \
              --lang-name monkey_c "$query" apps/garmin-connect-iq/source/*.mc
          done
        '';
        monkey-c.command = pkgs.writeShellScript "monkey-c-lint" ''
          exec ${lib.getExe rafiki} lint --no-config apps/garmin-connect-iq/source
        '';
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
          ${lib.getExe pkgs.uv} lock --check --offline \
            --python ${pythonToolsEnv}/bin/python \
            --project infra/python || exit $?
          exec ${lib.getExe pkgs.uv} lock --check --offline \
            --python ${pythonToolsEnv}/bin/python \
            --project crates/datapackage/compat
        '';
        python-tests.command = pkgs.writeShellScript "python-tests" ''
          export PATH=${
            lib.makeBinPath [
              pkgs.bash
              pkgs.coreutils
              pkgs.git
              pkgs.gh
              pkgs.just
              pkgs.dbus
              pkgs.syft
              pkgs.skopeo
              pkgs.distribution
              pkgs.zsync
              pkgs.squashfsTools
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
  inherit rafiki zedWasiSdk zedMonkeyCGrammar;
  inherit (project)
    apps
    checks
    formatter
    packages
    ;
}
