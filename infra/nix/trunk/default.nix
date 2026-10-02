{ pkgs }:
pkgs.trunk.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [ ./cache.patch ];
  postPatch = (old.postPatch or "") + ''
    cp ${./bindgen_cache.rs} src/pipelines/rust/bindgen_cache.rs
  '';
  # This developer tool does not need a whole-program LTO rebuild for a patch.
  env = (old.env or { }) // {
    CARGO_PROFILE_RELEASE_LTO = "false";
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
    # Share dependency artifacts between the binary and its test harness.
    CARGO_PROFILE_RELEASE_PANIC = "unwind";
  };
})
