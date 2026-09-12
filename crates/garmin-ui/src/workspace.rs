use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::DeviceSnapshot;

use crate::{device, icons, profile, shell};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Page {
    #[default]
    Activities,
    ProfileSettings,
    Device(String),
}

impl Page {
    #[must_use]
    pub fn index(&self, devices: &[DeviceSnapshot]) -> Option<usize> {
        match self {
            Self::Activities => Some(0),
            Self::ProfileSettings => Some(1),
            Self::Device(key) => devices
                .iter()
                .position(|device| &device.key == key)
                .map(|index| index + 2),
        }
    }

    #[must_use]
    pub fn from_index(index: usize, devices: &[DeviceSnapshot]) -> Option<Self> {
        match index {
            0 => Some(Self::Activities),
            1 => Some(Self::ProfileSettings),
            _ => devices
                .get(index - 2)
                .map(|device| Self::Device(device.key.clone())),
        }
    }
}

pub struct Props<'a> {
    pub product_name: &'a str,
    pub intl: &'a Intl,
    pub profiles: &'a [profile::ProfileProps<'a>],
    pub selected_profile: usize,
    pub profile_menu_expanded: bool,
    pub page: &'a Page,
    pub navigation: shell::Navigation,
    pub devices: &'a [DeviceSnapshot],
    pub window_controls: Option<&'a shell::WindowControls<'a>>,
}

pub struct Output<R> {
    pub action: Option<shell::Action>,
    pub inner: R,
}

#[must_use]
pub fn show<R>(ui: &mut Ui, props: &Props<'_>, page: impl FnOnce(&mut Ui) -> R) -> Output<R> {
    let activities = format_message!(props.intl, default_message: "Activities");
    let settings = format_message!(props.intl, default_message: "Profile settings");
    let primary_destinations = [
        shell::Destination {
            label: &activities,
            icon: icons::ACTIVITY,
        },
        shell::Destination {
            label: &settings,
            icon: icons::GEAR,
        },
    ];
    let device_destinations = props
        .devices
        .iter()
        .map(|snapshot| shell::Destination {
            label: &snapshot.name,
            icon: device::snapshot_icon(snapshot),
        })
        .collect::<Vec<_>>();
    let devices_label = format_message!(props.intl, default_message: "Attached devices");
    let navigation_groups = [
        shell::NavigationGroup {
            label: None,
            destinations: &primary_destinations,
        },
        shell::NavigationGroup {
            label: (!device_destinations.is_empty()).then_some(devices_label.as_str()),
            destinations: &device_destinations,
        },
    ];
    let toggle_label = format_message!(props.intl, default_message: "Toggle navigation");
    let profile_label = format_message!(props.intl, default_message: "Choose a profile");
    let profile_selector = profile::SelectorProps {
        intl: props.intl,
        profiles: props.profiles,
        selected: Some(props.selected_profile),
        expanded: props.profile_menu_expanded,
    };
    let output = shell::show(
        ui,
        &shell::Props {
            product_name: props.product_name,
            navigation_groups: &navigation_groups,
            active: props.page.index(props.devices),
            navigation: props.navigation,
            profile_selector: Some(&profile_selector),
            toggle_label: &toggle_label,
            profile_label: &profile_label,
            window_controls: props.window_controls,
        },
        page,
    );
    Output {
        action: output.action,
        inner: output.inner,
    }
}

#[cfg(test)]
mod tests {
    use super::Page;
    use garmin_service_api::{DeviceSnapshot, InspectionState};

    fn device(key: &str) -> DeviceSnapshot {
        DeviceSnapshot {
            key: key.to_owned(),
            name: key.to_owned(),
            identifier: None,
            software_version: None,
            inspection: InspectionState::Ready,
            capabilities: Vec::new(),
            storages: Vec::new(),
        }
    }

    #[test]
    fn page_indices_keep_primary_destinations_before_devices() {
        let devices = [device("edge"), device("fenix")];

        assert_eq!(Page::Activities.index(&devices), Some(0));
        assert_eq!(Page::ProfileSettings.index(&devices), Some(1));
        assert_eq!(Page::Device("fenix".to_owned()).index(&devices), Some(3));
        assert_eq!(
            Page::from_index(2, &devices),
            Some(Page::Device("edge".to_owned()))
        );
    }
}
