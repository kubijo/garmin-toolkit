import Toybox.Application;

class ProbeApp extends Application.AppBase {
    var transport;
    var probeView;

    function initialize() {
        AppBase.initialize();
        transport = new ProbeTransport();
        probeView = new ProbeView();
    }

    function getInitialView() {
        return [probeView, new ProbeInput()];
    }

    function onStop(state) {
        transport.stop();
    }
}
