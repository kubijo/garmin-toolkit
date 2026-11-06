{
  pkgs,
  package,
  arch,
  version,
  sourceRevision,
  configFile,
}:
let
  config = (pkgs.formats.yaml { }).generate "dev-overrides.yaml" {
    name = "Garmin Toolkit Dev";
    slug = "garmin_toolkit-dev";
    inherit version;
    arch = [ arch ];
    boot = "manual";
    init = false;
    description = "Development build ${sourceRevision}";
  };
  dockerfile = pkgs.writeText "Dockerfile" ''
    FROM scratch
    ADD rootfs.tar.gz /
    ENV HOME=/data TMPDIR=/tmp
    ENV SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
    ENV GARMIN_TOOLKIT_HASS_ADDRESS=0.0.0.0:8099
    ENV GARMIN_TOOLKIT_HASS_DEVICE_SOURCE=raw-mtp
    ENV GARMIN_TOOLKIT_HASS_INGRESS_GATEWAY=172.30.32.2
    WORKDIR /data
    EXPOSE 8099
    LABEL io.hass.version="${version}" io.hass.type="app" io.hass.arch="${arch}"
    ENTRYPOINT ["${package}/bin/garmin-hass"]
  '';
in
pkgs.runCommand "garmin-toolkit-dev-${arch}.tar.gz"
  {
    nativeBuildInputs = [ pkgs.yq-go ];
  }
  ''
    mkdir -p addon root/tmp root/data
    chmod 1777 root/tmp
    tar czf addon/rootfs.tar.gz --owner=0 --group=0 --hard-dereference -C / \
      $(sed 's:^/::' ${
        pkgs.closureInfo {
          rootPaths = [
            package
            pkgs.cacert
          ];
        }
      }/store-paths)
    tar cf root.tar --owner=0 --group=0 -C root tmp data
    # Append writable runtime directories to the otherwise immutable closure archive.
    gzip -dc addon/rootfs.tar.gz > filesystem.tar
    tar --concatenate --file=filesystem.tar root.tar
    gzip -c filesystem.tar > addon/rootfs.tar.gz
    yq eval-all '. as $item ireduce ({}; . * $item) | del(.image)' \
      ${configFile} ${config} > addon/config.yaml
    cp ${dockerfile} addon/Dockerfile
    cp ${package}/share/garmin-hass/{icon,logo}.png addon/
    tar czf "$out" -C addon .
  ''
