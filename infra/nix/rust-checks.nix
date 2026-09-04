{
  craneLib,
  coverageMinimum,
  formatjsCli,
  galleryRuntimeLibraries,
  inheritanceCheck,
  licenseChecker,
  lib,
  nixCargoTargetDir,
  pkgs,
  toolchain,
  workspaceSrc,
}:

let
  cargo = lib.getExe' toolchain "cargo";
  formatjs = lib.getExe formatjsCli;
  sqlx = lib.getExe pkgs.sqlx-cli;
  # Gallery snapshots exercise immediate-mode rendering outside llvm-cov.
  coverageArgs = [
    "--ignore-filename-regex"
    "crates/garmin-ui/|apps/garmin-desktop/src/view\\.rs"
  ];
  workspaceArgs = [
    "--locked"
    "--workspace"
    "--all-features"
  ];
  clippyArgs = [
    "clippy"
  ]
  ++ workspaceArgs
  ++ [
    "--all-targets"
    "--"
    "--deny"
    "warnings"
  ];
  testArgs = [
    "nextest"
    "run"
  ]
  ++ workspaceArgs;
  docArgs = [ "doc" ] ++ workspaceArgs ++ [ "--no-deps" ];
  denyArgs = [
    "deny"
    "check"
    "bans"
    "licenses"
    "sources"
  ];
  galleryArgs = [
    "--manifest-path"
    "infra/gallery/Cargo.toml"
    "--locked"
    "--all-features"
  ];
  galleryClippyArgs = [
    "clippy"
  ]
  ++ galleryArgs
  ++ [
    "--all-targets"
    "--"
    "--deny"
    "warnings"
  ];
  galleryDocArgs = [ "doc" ] ++ galleryArgs ++ [ "--no-deps" ];
  galleryTestArgs = [
    "nextest"
    "run"
    "--no-tests"
    "pass"
  ]
  ++ galleryArgs;
  galleryDenyArgs = [
    "deny"
    "--manifest-path"
    "infra/gallery/Cargo.toml"
    "--config"
    "infra/gallery/deny.toml"
    "check"
    "--allow"
    "no-license-field"
    "licenses"
    "sources"
  ];
  cargoCommand = args: "${cargo} ${lib.escapeShellArgs args}";
  coverageReport =
    args:
    cargoCommand (
      [
        "llvm-cov"
        "report"
      ]
      ++ coverageArgs
      ++ args
    );
  craneArgs = args: lib.filter (argument: argument != "--locked") args;
  checkOutput = checkName: ''
    section_color=
    success_color=
    failure_color=
    reset_color=

    if [[ -t 1 && -z ''${NO_COLOR:-} ]]; then
      section_color=$'\033[1;36m'
      success_color=$'\033[1;32m'
      failure_color=$'\033[1;31m'
      reset_color=$'\033[0m'
    fi

    run_step() {
      local label=$1
      shift
      printf '\n%s━━ %s%s\n' "$section_color" "$label" "$reset_color"

      if "$@"; then
        return 0
      else
        local status=$?
        printf '\n%s✗ ${checkName} failed%s (%s, exit %d)\n' \
          "$failure_color" "$reset_color" "$label" "$status" >&2
        exit "$status"
      fi
    }

    finish_check() {
      printf '\n%s✓ ${checkName} passed%s\n' "$success_color" "$reset_color"
    }
  '';
  mkApp =
    name: runtimeInputs: text:
    pkgs.writeShellApplication {
      inherit name runtimeInputs text;
      runtimeEnv = {
        CARGO_TARGET_DIR = nixCargoTargetDir;
      }
      // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
        LD_LIBRARY_PATH = lib.makeLibraryPath galleryRuntimeLibraries;
      };
    };
  src = import ./cargo-source.nix {
    inherit craneLib lib workspaceSrc;
  };
  commonArgs = {
    inherit src;
    CARGO_TARGET_DIR = "target";
    SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.glib ];
    cargoLock = workspaceSrc + "/Cargo.lock";
    nativeBuildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.pkg-config ];
    strictDeps = true;
  }
  // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
    LD_LIBRARY_PATH = lib.makeLibraryPath galleryRuntimeLibraries;
  };
  cargoArtifacts = craneLib.buildDepsOnly (
    commonArgs
    // {
      cargoExtraArgs = "--workspace --all-features";
      doCheck = false;
    }
  );
  extractSourceCatalog = destination: ''
    ${formatjs} extract \
      'apps/**/*.rs' \
      'crates/**/*.rs' \
      --format crowdin \
      --out-file "${destination}" \
      --id-interpolation-pattern '[sha512:contenthash:base64:10]' \
      --throws
  '';
  compileCatalogs = directory: ''
    ${formatjs} compile \
      --format crowdin \
      --out-file "${directory}/en.json" \
      "${directory}/en-source.json"
    ${formatjs} compile \
      --format lokalise \
      --out-file "${directory}/cs.json" \
      crates/garmin-i18n/translations/cs.json
  '';
  i18nCheckCommand = ''
    i18n_dir=$(mktemp -d)
    trap 'rm -rf "$i18n_dir"' EXIT

    ${extractSourceCatalog "$i18n_dir/en-source.json"}
    ${lib.getExe pkgs.jq} --exit-status \
      --slurpfile translation crates/garmin-i18n/translations/cs.json \
      '(length == ($translation[0] | length)) and all(to_entries[];
        $translation[0][.key].source == .value.message and
        ($translation[0][.key].description // null) == (.value.description // null) and
        ($translation[0][.key].translation | type == "string" and length > 0))' \
      "$i18n_dir/en-source.json" > /dev/null
    message_count=$(${lib.getExe pkgs.jq} length "$i18n_dir/en-source.json")
    echo "FormatJS catalog completeness: $message_count/$message_count Czech messages"
    ${compileCatalogs "$i18n_dir"}
    cmp crates/garmin-i18n/catalogs/cs.json "$i18n_dir/cs.json"
    ${formatjs} verify "$i18n_dir/en.json" "$i18n_dir/cs.json" \
      --source-locale en \
      --missing-keys \
      --extra-keys \
      --structural-equality
  '';
  i18nCheck = mkApp "i18n-check" [
    formatjsCli
    pkgs.coreutils
    pkgs.jq
  ] i18nCheckCommand;
  i18nSync =
    mkApp "i18n-sync"
      [
        formatjsCli
        pkgs.coreutils
        pkgs.jq
      ]
      ''
        i18n_dir=$(mktemp -d)
        trap 'rm -rf "$i18n_dir"' EXIT

        ${extractSourceCatalog "$i18n_dir/en-source.json"}
        ${lib.getExe pkgs.jq} \
          --slurpfile translation crates/garmin-i18n/translations/cs.json \
          'def previous($source):
            [$translation[0][] | select(.source == $source) | .translation | select(type == "string")]
            | if length == 1 then .[0] else null end;
          with_entries(.value as $message | .value = ({
            source: $message.message,
            translation: ($translation[0][.key].translation // previous($message.message))
          } + if $message.description then { description: $message.description } else {} end))' \
          "$i18n_dir/en-source.json" > "$i18n_dir/cs-source.json"
        cp "$i18n_dir/cs-source.json" crates/garmin-i18n/translations/cs.json
        ${formatjs} compile \
          --format lokalise \
          --out-file crates/garmin-i18n/catalogs/cs.json \
          crates/garmin-i18n/translations/cs.json
      '';
  projectLint =
    mkApp "project-lint"
      [
        sourceShapeCheck
        inheritanceCheck
        i18nCheck
        licenseChecker
        pkgs.cargo-deny
        pkgs.cargo-machete
        pkgs.cargo-nextest
        sqlxCheck
        toolchain
      ]
      ''
        ${checkOutput "project lint"}
        run_step "Rust source shape" ${lib.getExe sourceShapeCheck}
        run_step "cargo workspace-inheritance-check" ${lib.getExe inheritanceCheck} --path .
        run_step "FormatJS catalogs" ${lib.getExe i18nCheck}
        run_step "cargo sqlx prepare --check" ${lib.getExe sqlxCheck}
        run_step "cargo clippy" ${cargoCommand clippyArgs}
        run_step "cargo doc" env RUSTDOCFLAGS='-D warnings' ${cargoCommand docArgs}
        run_step "cargo nextest" ${cargoCommand testArgs}
        run_step "license bundles" ${lib.getExe licenseChecker}
        run_step "cargo deny" ${cargoCommand denyArgs}
        run_step "cargo machete" ${cargoCommand [ "machete" ]}
        run_step "gallery clippy" ${cargoCommand galleryClippyArgs}
        run_step "gallery doc" env RUSTDOCFLAGS='-D warnings' ${cargoCommand galleryDocArgs}
        run_step "gallery nextest" ${cargoCommand galleryTestArgs}
        run_step "gallery deny" ${cargoCommand galleryDenyArgs}
        finish_check
      '';
  projectCheck =
    name: nativeBuildInputs: command:
    pkgs.runCommand name { inherit nativeBuildInputs; } ''
      cp -R ${workspaceSrc} source
      chmod -R u+w source
      cd source
      ${command}
      touch "$out"
    '';
  sqlxPrepareCommand = check: ''
    sqlx_prepare_dir=$(mktemp -d)
    trap 'rm -rf "$sqlx_prepare_dir"' EXIT
    sqlx_database_url="sqlite://$sqlx_prepare_dir/query.sqlite3"

    ${sqlx} database create --database-url "$sqlx_database_url"
    ${sqlx} migrate run \
      --source crates/garmin-storage/migrations \
      --database-url "$sqlx_database_url"
    ${cargo} sqlx prepare ${lib.optionalString check "--check"} \
      --workspace \
      --no-dotenv \
      --database-url "$sqlx_database_url" \
      -- \
      --all-features \
      --all-targets
  '';
  sqlxPrepare = mkApp "sqlx-prepare" [
    pkgs.coreutils
    pkgs.sqlx-cli
    toolchain
  ] (sqlxPrepareCommand false);
  sqlxCheck = mkApp "sqlx-check" [
    pkgs.coreutils
    pkgs.sqlx-cli
    toolchain
  ] (sqlxPrepareCommand true);
  sourceShapeCommands = ''
    if rg --line-number \
      --glob '*.md' \
      --glob '*.toml' \
      --glob '*.yaml' \
      --glob '*.yml' \
      '\bjust[[:space:]]+(cli|desktop|dev|gallery|hass|qa)[[:space:]]+'; then
      echo 'Use just namespace::recipe for user-facing module commands.' >&2
      exit 1
    fi
  '';
  sourceShapeCheck = pkgs.writeShellApplication {
    name = "source-shape-check";
    runtimeInputs = [ pkgs.ripgrep ];
    text = sourceShapeCommands;
  };
in
{
  apps = {
    audit =
      mkApp "audit"
        [
          pkgs.cargo-deny
          pkgs.gitleaks
          pkgs.gitMinimal
        ]
        ''
          ${checkOutput "audit"}
          run_step "RustSec advisories" ${
            cargoCommand [
              "deny"
              "check"
              "advisories"
            ]
          }
          if git rev-parse --verify HEAD >/dev/null 2>&1; then
            run_step "Git history secrets" ${lib.getExe pkgs.gitleaks} git --config infra/gitleaks.toml --redact --no-banner .
          fi
          run_step "working tree secrets" ${lib.getExe pkgs.gitleaks} dir --config infra/gitleaks.toml --redact --no-banner .
          run_step "staged secrets" ${lib.getExe pkgs.gitleaks} git --config infra/gitleaks.toml --redact --no-banner --pre-commit --staged .
          finish_check
        '';
    coverage =
      mkApp "coverage"
        [
          pkgs.cargo-llvm-cov
          pkgs.cargo-nextest
          toolchain
        ]
        ''
          ${cargoCommand [
            "llvm-cov"
            "clean"
            "--workspace"
          ]}
          ${cargoCommand (
            [
              "llvm-cov"
              "nextest"
            ]
            ++ workspaceArgs
            ++ [ "--no-report" ]
          )}
          ${coverageReport [
            "--html"
            "--output-dir"
            ".tmp/coverage"
          ]}
          ${coverageReport [
            "--lcov"
            "--output-path"
            ".tmp/coverage/lcov.info"
          ]}
          ${coverageReport [
            "--cobertura"
            "--output-path"
            ".tmp/coverage/cobertura.xml"
          ]}
          ${coverageReport [
            "--summary-only"
            "--fail-under-lines"
            (toString coverageMinimum)
          ]}
        '';
    dedupe = mkApp "dedupe" [ pkgs.cargo-deny ] ''
      ${cargoCommand [
        "deny"
        "check"
        "bans"
      ]}
    '';
    docs = mkApp "docs" [ toolchain ] ''
      env RUSTDOCFLAGS='-D warnings' ${cargoCommand docArgs} --open "$@"
    '';
    project-lint = projectLint;
    i18n-check = i18nCheck;
    i18n-sync = i18nSync;
    outdated =
      mkApp "outdated"
        [
          pkgs.cargo-outdated
          toolchain
        ]
        ''
          ${cargoCommand [
            "outdated"
            "--workspace"
            "--root-deps-only"
          ]}
        '';
    sqlx-prepare = sqlxPrepare;
    sqlx-check = sqlxCheck;
    test =
      mkApp "test"
        [
          pkgs.cargo-nextest
          toolchain
        ]
        ''
          ${cargoCommand testArgs} "$@"
        '';
  };
  checks = {
    rust-clippy = craneLib.cargoClippy (
      commonArgs
      // {
        inherit cargoArtifacts;
        cargoClippyExtraArgs = lib.escapeShellArgs (craneArgs (lib.tail clippyArgs));
      }
    );
    rust-coverage = craneLib.cargoNextest (
      commonArgs
      // {
        inherit cargoArtifacts;
        CARGO_PROFILE = "dev";
        cargoExtraArgs = lib.escapeShellArgs (lib.drop 2 testArgs);
        cargoLlvmCovExtraArgs = lib.escapeShellArgs (
          coverageArgs
          ++ [
            "--summary-only"
            "--fail-under-lines"
            (toString coverageMinimum)
          ]
        );
        doCheck = true;
        preCheck = "mkdir -p $out";
        withLlvmCov = true;
      }
    );
    rust-deny = craneLib.cargoDeny (
      commonArgs
      // {
        src = workspaceSrc;
        cargoDenyChecks = lib.concatStringsSep " " (lib.drop 2 denyArgs);
      }
    );
    rust-doc = craneLib.cargoDoc (
      commonArgs
      // {
        inherit cargoArtifacts;
        cargoDocExtraArgs = lib.escapeShellArgs (craneArgs (lib.tail docArgs));
        RUSTDOCFLAGS = "-D warnings";
      }
    );
    rust-inheritance = projectCheck "rust-workspace-inheritance" [ inheritanceCheck ] ''
      ${lib.getExe inheritanceCheck} --path .
    '';
    rust-i18n =
      pkgs.runCommand "rust-i18n"
        {
          nativeBuildInputs = [
            formatjsCli
            pkgs.coreutils
            pkgs.jq
          ];
        }
        ''
          cp -R ${src} source
          chmod -R u+w source
          cd source
          ${i18nCheckCommand}
          touch "$out"
        '';
    rust-machete =
      projectCheck "rust-unused-dependencies"
        [
          pkgs.cargo-machete
          toolchain
        ]
        ''
          ${cargoCommand [ "machete" ]}
        '';
    rust-sqlx = craneLib.mkCargoDerivation (
      commonArgs
      // {
        inherit cargoArtifacts;
        pname = "rust-sqlx";
        nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
          pkgs.coreutils
          pkgs.sqlx-cli
        ];
        buildPhaseCargoCommand = sqlxPrepareCommand true;
        doCheck = false;
        doInstallCargoArtifacts = false;
      }
    );
    rust-source-shape = projectCheck "rust-source-shape" [ pkgs.ripgrep ] sourceShapeCommands;
    rust-source-closure = pkgs.runCommandLocal "rust-source-closure" { } ''
      diff --recursive --brief \
        ${workspaceSrc}/crates/garmin-brand/assets \
        ${src}/crates/garmin-brand/assets
      diff --recursive --brief \
        ${workspaceSrc}/crates/garmin-ui/assets \
        ${src}/crates/garmin-ui/assets
      test ! -e ${src}/infra/gallery
      touch "$out"
    '';
    rust-tests = craneLib.cargoNextest (
      commonArgs
      // {
        inherit cargoArtifacts;
        cargoExtraArgs = lib.escapeShellArgs (lib.drop 2 testArgs);
        doCheck = true;
      }
    );
  };
}
