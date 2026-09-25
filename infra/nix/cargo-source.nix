{
  craneLib,
  extraFilesets ? [ ],
  includeGallery ? false,
  lib,
  workspaceSrc,
}:

let
  gallery = workspaceSrc + "/infra/gallery";
  workspaceCargoSources = lib.fileset.difference (craneLib.fileset.commonCargoSources workspaceSrc) gallery;
in
lib.fileset.toSource {
  root = workspaceSrc;
  fileset = lib.fileset.unions (
    [
      workspaceCargoSources
      (workspaceSrc + "/vendor/fast-mvt")
      (workspaceSrc + "/vendor/winit")
      (workspaceSrc + "/crates/garmin-brand/assets")
      (workspaceSrc + "/crates/garmin-diagnostics/templates")
      (workspaceSrc + "/crates/garmin-diagnostics/view.js")
      (workspaceSrc + "/crates/garmin-gpx/tests/fixtures")
      (workspaceSrc + "/crates/garmin-i18n/catalogs")
      (workspaceSrc + "/crates/garmin-i18n/translations")
      (workspaceSrc + "/crates/garmin-simulator/fixtures")
      (workspaceSrc + "/crates/garmin-ui/assets")
      (workspaceSrc + "/infra/fixtures/fit/development-activities/recordings")
      (workspaceSrc + "/apps/garmin-hass/web/Trunk.toml")
      (workspaceSrc + "/apps/garmin-hass/web/index.html")
      (workspaceSrc + "/apps/garmin-hass/web/initializer.js")
      (workspaceSrc + "/apps/garmin-hass/web/map-worker.js")
      (workspaceSrc + "/apps/garmin-hass/web/worker-codec.js")
      (lib.fileset.fileFilter (file: file.hasExt "js") (workspaceSrc + "/apps/garmin-hass/web"))
      (workspaceSrc + "/infra/javascript")
      (lib.fileset.maybeMissing (workspaceSrc + "/.sqlx"))
      (lib.fileset.fileFilter (file: file.hasExt "sql") workspaceSrc)
      (lib.fileset.fileFilter (file: file.hasExt "wgsl") workspaceSrc)
    ]
    ++ lib.optionals includeGallery [ (craneLib.fileset.commonCargoSources gallery) ]
    ++ extraFilesets
  );
}
