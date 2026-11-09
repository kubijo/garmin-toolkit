import Toybox.Application;
import Toybox.WatchUi;

class ProbeInput extends WatchUi.BehaviorDelegate {
    function initialize() {
        BehaviorDelegate.initialize();
    }

    function onSelect() {
        Application.getApp().transport.start();
        return true;
    }

    function onTap(event) {
        Application.getApp().transport.start();
        return true;
    }
}
