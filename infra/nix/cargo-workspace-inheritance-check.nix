{
  fetchFromGitHub,
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage rec {
  pname = "cargo-workspace-inheritance-check";
  version = "1.3.0";

  src = fetchFromGitHub {
    owner = "RomarQ";
    repo = "cargo-workspace-inheritance-check";
    rev = "v${version}";
    hash = "sha256-yxtNdicx1HByFQ3Pv7Eb136IfIGbnef9tzZYJ5cxOyw=";
  };

  cargoHash = "sha256-L90ASy+HdgaqmNFDch0+sMjIt5pZr69vc5AzsyeJKzQ=";

  meta = {
    description = "Check Cargo workspace dependency inheritance";
    homepage = "https://github.com/RomarQ/cargo-workspace-inheritance-check";
    license = lib.licenses.mit;
    mainProgram = "cargo-workspace-inheritance-check";
  };
}
