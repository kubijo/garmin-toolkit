{ lib, workspaceSrc }:

{
  # Registry dependencies use these patched crates' APIs, so Crane must compile
  # their real sources even while workspace crates are replaced with stubs.
  extraDummyScript =
    lib.concatMapStringsSep "\n"
      (
        name:
        let
          source = builtins.path {
            path = workspaceSrc + "/vendor/${name}";
            name = "${name}-source";
          };
        in
        ''
          rm -rf "$out/vendor/${name}"
          mkdir -p "$out/vendor"
          cp -R ${source} "$out/vendor/${name}"
        ''
      )
      [
        "fast-mvt"
        "winit"
      ];
}
