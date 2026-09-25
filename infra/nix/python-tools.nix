{
  nixpkgs,
  pkgs,
  pyproject-build-systems,
  pyproject-nix,
  uv2nix,
}:
let
  python = pkgs.python314;
  workspace = uv2nix.lib.workspace.loadWorkspace { workspaceRoot = ../python; };
  overlay = workspace.mkPyprojectOverlay { sourcePreference = "wheel"; };
  pythonSet = (pkgs.callPackage pyproject-nix.build.packages { inherit python; }).overrideScope (
    nixpkgs.lib.composeManyExtensions [
      pyproject-build-systems.overlays.default
      overlay
    ]
  );
in
pythonSet.mkVirtualEnv "garmin-python-tools-env" workspace.deps.default
