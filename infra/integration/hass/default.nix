{
  nodejs,
  pkgs,
  package,
  testNamePattern ? null,
  testFile ? "routes.test.mjs",
}:
let
  inherit (pkgs) lib;
  source = lib.fileset.toSource {
    root = ../../..;
    fileset = lib.fileset.unions [
      ./.
      ../../../crates/garmin-gpx/tests/fixtures
    ];
  };
  runner = pkgs.writeShellApplication {
    name = "hass-integration";
    runtimeInputs = [ nodejs ];
    text = ''
      export GARMIN_INTEGRATION_VM=1
      export GARMIN_HASS_BINARY=${package}/bin/garmin-hass
      export GARMIN_PLAYWRIGHT=${pkgs.playwright-driver}
      export PLAYWRIGHT_BROWSERS_PATH=${pkgs.playwright-driver.browsers-chromium}
      export GARMIN_GPX_CANDIDATES=${source}/crates/garmin-gpx/tests/fixtures/candidates.gpx
      export GARMIN_E2E_STATE_ROOT=/var/lib/hass-integration/state
      export GARMIN_E2E_RESULTS=/var/lib/hass-integration/results
      if [[ ''${1:-} == --cleanup-probe ]]; then
        exec node ${source}/infra/integration/hass/cleanup-probe.mjs
      fi
      exec node --test --test-isolation=none --test-concurrency=1 --test-reporter=tap \
        ${
          lib.optionalString (
            testNamePattern != null
          ) "--test-name-pattern=${lib.escapeShellArg testNamePattern}"
        } \
        ${source}/infra/integration/hass/${lib.escapeShellArg testFile}
    '';
  };
  service = argument: {
    requires = [ "integration-display.service" ];
    wants = [ "network-online.target" ];
    after = [
      "integration-display.service"
      "network-online.target"
    ];
    environment = {
      DISPLAY = ":99";
      LIBGL_ALWAYS_SOFTWARE = "1";
      XDG_RUNTIME_DIR = "/run/hass-integration";
    };
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${lib.getExe runner}${argument}";
      StateDirectory = "hass-integration";
      RuntimeDirectory = "hass-integration";
      TimeoutStartSec = "15min";
      TimeoutStopSec = "15s";
      KillMode = "control-group";
      MemoryMax = "3G";
      TasksMax = 512;
    };
  };
in
pkgs.testers.runNixOSTest {
  name = "hass-routes-integration";
  globalTimeout = 1200;

  nodes.machine = {
    virtualisation = {
      memorySize = 4096;
      diskSize = 4096;
      restrictNetwork = true;
      cores = 2;
      qemu.forceAccel = lib.mkForce true;
    };
    fonts.packages = [ pkgs.dejavu_fonts ];
    hardware.graphics.enable = true;
    systemd.coredump.enable = false;
    environment.systemPackages = [
      pkgs.curl
      pkgs.procps
    ];
    systemd.services = {
      integration-display = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          ExecStart = "${pkgs.xorg-server}/bin/Xvfb :99 -screen 0 1280x960x24 -nolisten tcp -ac";
          KillMode = "control-group";
        };
      };
      hass-integration = service "";
      hass-integration-cleanup-probe = service " --cleanup-probe";
    };
  };

  testScript = ''
    from datetime import timedelta

    start_all()
    try:
        machine.wait_for_unit("integration-display.service")
        machine.wait_for_file("/tmp/.X11-unix/X99")
        with subtest("GPX browser assertions in fresh per-case state"):
            machine.succeed("systemctl start hass-integration.service", timeout=timedelta(minutes=15))
        with subtest("failed tests still tear down their state and processes"):
            machine.fail("systemctl start hass-integration-cleanup-probe.service", timeout=timedelta(minutes=2))
            machine.succeed(
                "journalctl -u hass-integration-cleanup-probe.service --no-pager "
                "| grep -F INTENTIONAL_FAILURE_CLEANUP_VERIFIED"
            )
        with subtest("no host, browser, worker, or test state survives"):
            machine.fail("curl --max-time 2 --fail http://127.0.0.1:8099/api/capabilities")
            machine.fail("pgrep -f '/bin/(garmin-hass|[.]garmin-hass-wrapped|garmin-gpx-worker)( |$)'")
            machine.fail("pgrep -x chrome")
            machine.fail("pgrep -x node")
            machine.succeed("test -z \"$(find /var/lib/hass-integration/state -mindepth 1 -print -quit)\"")
    finally:
        try:
            machine.execute(
                "systemctl stop hass-integration.service hass-integration-cleanup-probe.service",
                timeout=timedelta(seconds=30),
            )
            machine.execute(
                "journalctl -u hass-integration.service -u hass-integration-cleanup-probe.service "
                "--no-pager > /var/lib/hass-integration/results/journal.log",
                timeout=timedelta(seconds=10),
            )
            machine.execute(
                "journalctl -k --no-pager > /var/lib/hass-integration/results/kernel.log",
                timeout=timedelta(seconds=10),
            )
            machine.copy_from_machine("/var/lib/hass-integration/results", "artifacts")
        finally:
            machine.shutdown()
  '';
}
