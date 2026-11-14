{ pkgs }:
{
  package,
  program ? pkgs.lib.getExe package,
}:
let
  inherit (pkgs) lib;
  launchScript = pkgs.runCommand "garmin-toolkit-host-graphics-launcher" {
    src = ./launch.sh;
    meta.license = lib.licenses.agpl3Plus;
  } ''cp "$src" "$out"'';
  closure = pkgs.closureInfo { rootPaths = [ package ]; };
  libraryManifest =
    pkgs.runCommand "${package.name}-library-directories" { meta.license = lib.licenses.agpl3Plus; }
      ''
        # The loader's own libc must win, including over other closure outputs.
        for root in ${lib.getLib pkgs.stdenv.cc.libc} $(cat ${closure}/store-paths); do
          for directory in "$root/lib" "$root/lib64"; do
            if test -d "$directory"; then
              printf '%s\n' "$directory"
            fi
          done
        done > "$out"
      '';
in
pkgs.runCommand "${package.name}-host-graphics"
  {
    nativeBuildInputs = [ pkgs.makeShellWrapper ];
    passthru.runtimeInputs = [
      package
      libraryManifest
      launchScript
      pkgs.glibc
    ];
    meta = (package.meta or { }) // {
      mainProgram = baseNameOf program;
    };
  }
  ''
    mkdir -p "$out/bin"
    makeWrapper ${pkgs.bash}/bin/bash "$out/bin/${baseNameOf program}" \
      --add-flags ${
        lib.escapeShellArg (
          lib.escapeShellArgs [
            launchScript
            libraryManifest
            "${lib.getBin pkgs.glibc}/bin/ldconfig"
            "/etc/ld.so.cache"
            program
          ]
        )
      }
    if test -d ${package}/share; then
      ln -s ${package}/share "$out/share"
    fi
  ''
