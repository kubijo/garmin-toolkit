{
  lib,
  pkgs,
  pythonToolsEnv,
  workspaceSrc,
}:
let
  source = lib.fileset.toSource {
    root = workspaceSrc;
    fileset = lib.fileset.unions (
      map (path: workspaceSrc + "/${path}") [
        "infra/python/desktop_vm.py"
        "infra/python/desktop_vm_qmp.py"
        "infra/python/cli_output.py"
        "infra/python/json_data.py"
        "infra/python/processes.py"
        "infra/integration/desktop/cloud-init.yaml"
        "infra/integration/desktop/guest.py"
      ]
    );
  };
  pythonCommand = "${pythonToolsEnv}/bin/python ${source}/infra/python/desktop_vm.py";
  image = pkgs.fetchurl {
    url = "https://cloud-images.ubuntu.com/resolute/20261016/resolute-server-cloudimg-arm64.img";
    sha256 = "8eb876c2ccc4b6ff89e4a65d511815923b285e04503ba465809bf587f763da24";
  };
  app = pkgs.writeShellApplication {
    name = "garmin-desktop-vm-test";
    runtimeInputs = [
      pkgs.qemu
      pkgs.openssh
      pkgs.xorriso
    ];
    text = ''
      exec ${pythonCommand} \
        --image ${image} \
        --firmware ${pkgs.qemu}/share/qemu/edk2-aarch64-code.fd \
        --machine virt-${lib.versions.majorMinor pkgs.qemu.version} \
        "$@"
    '';
  };
in
{
  inherit app;
  startupCheck = pkgs.runCommand "check-desktop-vm-startup" { } ''
    ${pythonCommand} --help > "$out"
    test -f ${source}/infra/integration/desktop/cloud-init.yaml
    test -f ${source}/infra/integration/desktop/guest.py
  '';
}
