{
  lib,
  nix-tools,
  pkgs,
  system,
  toolchain,
  workspaceSrc,
}:
let
  sqlFluffConfig = ../sqlfluff/pyproject.toml;
  allFormatters = {
    html = true;
    javascript = true;
    json = true;
    justfile = true;
    markdown = true;
    nix = true;
    python = true;
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
      "LICENSE-AGPL"
      "LICENSE-APACHE"
      "LICENSE-MIT"
      "assets/licenses/**"
      "crates/garmin-ui/assets/fonts/**"
      "old/**"
      "infra/gallery/fonts/**"
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
      python = true;
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
