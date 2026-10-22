{
  fetchFromGitHub,
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage rec {
  pname = "cargo-workspace-inheritance-check";
  version = "1.4.0";

  src = fetchFromGitHub {
    owner = "RomarQ";
    repo = "cargo-workspace-inheritance-check";
    rev = "v${version}";
    hash = "sha256-jovH2qKCZaC3uihly5410EQ8Qv/8V+LEfjpYajoIXSk=";
  };

  cargoHash = "sha256-VjelV8nkqEaE0NlOUUJezlSJeraHk0KBiFMlZZWme9Q=";

  meta = {
    description = "Check Cargo workspace dependency inheritance";
    homepage = "https://github.com/RomarQ/cargo-workspace-inheritance-check";
    license = lib.licenses.mit;
    mainProgram = "cargo-workspace-inheritance-check";
  };
}
