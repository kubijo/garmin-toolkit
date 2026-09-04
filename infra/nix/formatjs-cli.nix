{
  fetchurl,
  lib,
  stdenvNoCC,
}:

let
  version = "1.6.0";
  assets = {
    aarch64-darwin = {
      name = "darwin-arm64";
      hash = "sha256-XHIKEvRN6OlyQLE2EL/1wDUURsl4hVoFnAgBg+X8DZ0=";
    };
    aarch64-linux = {
      name = "linux-arm64";
      hash = "sha256-ZI/qtTdHmlEAPMe1dwDAlO5PLXmSvtB0XlJLsbDD2/8=";
    };
    x86_64-linux = {
      name = "linux-x64";
      hash = "sha256-54rO9l+2TvzwMcSincqVNazwyZQ5xTQ/oBbRp/fbEyY=";
    };
  };
  asset = assets.${stdenvNoCC.hostPlatform.system};
in
stdenvNoCC.mkDerivation {
  pname = "formatjs-cli";
  inherit version;

  src = fetchurl {
    url = "https://github.com/formatjs/formatjs/releases/download/formatjs_cli_v${version}/formatjs_cli-${asset.name}";
    inherit (asset) hash;
  };
  dontUnpack = true;

  installPhase = ''
    runHook preInstall
    install -Dm755 "$src" "$out/bin/formatjs"
    runHook postInstall
  '';

  meta = {
    description = "Rust CLI for extracting and validating FormatJS messages";
    homepage = "https://formatjs.io/";
    license = lib.licenses.mit;
    mainProgram = "formatjs";
    platforms = builtins.attrNames assets;
  };
}
