{ pkgs, toolchain }:
let
  inherit (builtins) fromJSON readFile;
  pin = fromJSON (readFile ../garmin-connect/input.json);
  descriptor = pkgs.fetchurl {
    url = "https://raw.githubusercontent.com/Likenttt/connect-personal-api-ts/${pin.revision}/spec/openapi.json";
    inherit (pin) sha256;
  };
  rustPlatform = pkgs.makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  };
in
rustPlatform.buildRustPackage {
  pname = "garmin-connect-contract";
  version = "0.0.0";
  src = ../garmin-connect;
  cargoLock.lockFile = ../garmin-connect/Cargo.lock;
  SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
  preBuild = ''
    cmp openapi.json ${descriptor}
  '';
  doCheck = true;
  installPhase = ''
    mkdir -p "$out"
    cp input.json "$out/"
  '';
}
