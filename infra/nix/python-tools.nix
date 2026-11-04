{
  nixpkgs,
  pkgs,
  pyproject-build-systems,
  pyproject-nix,
  uv2nix,
  workspaceRoot ? ../python,
  environmentName ? "garmin-python-tools-env",
}:
let
  python = pkgs.python314;
  workspace = uv2nix.lib.workspace.loadWorkspace { inherit workspaceRoot; };
  overlay = workspace.mkPyprojectOverlay { sourcePreference = "wheel"; };
  pythonSet = (pkgs.callPackage pyproject-nix.build.packages { inherit python; }).overrideScope (
    nixpkgs.lib.composeManyExtensions [
      pyproject-build-systems.overlays.default
      overlay
    ]
  );
in
# Tooling includes development groups so strict checks can load their type stubs.
pythonSet.mkVirtualEnv environmentName workspace.deps.all
