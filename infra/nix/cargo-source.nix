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
      (workspaceSrc + "/crates/garmin-brand/assets")
      (workspaceSrc + "/crates/garmin-gpx/tests/fixtures")
      (workspaceSrc + "/crates/garmin-i18n/catalogs")
      (workspaceSrc + "/crates/garmin-i18n/translations")
      (workspaceSrc + "/crates/garmin-simulator/fixtures")
      (workspaceSrc + "/crates/garmin-ui/assets")
      (workspaceSrc + "/crates/garmin-update/tests/fixtures")
      (workspaceSrc + "/apps/garmin-hass/web/index.html")
      (workspaceSrc + "/apps/garmin-hass/web/initializer.js")
      (lib.fileset.maybeMissing (workspaceSrc + "/.sqlx"))
      (lib.fileset.fileFilter (file: file.hasExt "sql") workspaceSrc)
    ]
    ++ lib.optionals includeGallery [ (craneLib.fileset.commonCargoSources gallery) ]
    ++ extraFilesets
  );
}
