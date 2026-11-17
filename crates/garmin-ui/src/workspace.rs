use egui::Ui;
use garmin_i18n::{Intl, format_message};
use garmin_service_api::DeviceSnapshot;

use crate::{device, icons, profile, shell};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Page {
    #[default]
    Activities,
    Routes,
    ProfileSettings,
    GarminConnect,
    Trends,
    Backup,
    Device(String),
    Maps(String),
}

impl Page {
    #[must_use]
    pub fn index(&self, devices: &[DeviceSnapshot]) -> Option<usize> {
        match self {
            Self::Activities => Some(0),
            Self::Routes => Some(1),
            Self::ProfileSettings => Some(2),
            Self::GarminConnect => Some(3 + devices.len()),
            Self::Trends => Some(4 + devices.len()),
            Self::Backup => None,
            Self::Device(key) | Self::Maps(key) => devices
                .iter()
                .position(|device| &device.key == key)
                .map(|index| index + 3),
        }
    }

    #[must_use]
    pub fn from_index(index: usize, devices: &[DeviceSnapshot]) -> Option<Self> {
        match index {
            0 => Some(Self::Activities),
            1 => Some(Self::Routes),
            2 => Some(Self::ProfileSettings),
            _ if index == 3 + devices.len() => Some(Self::GarminConnect),
            _ if index == 4 + devices.len() => Some(Self::Trends),
            _ => devices
                .get(index.checked_sub(3)?)
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
    pub backup_enabled: bool,
    pub garmin_connect_configured: bool,
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
    let routes = format_message!(props.intl, default_message: "Routes");
    let primary_destinations = [
        shell::Destination {
            label: &activities,
            icon: icons::ACTIVITY,
        },
        shell::Destination {
            label: &routes,
            icon: icons::MAP_TRIFOLD,
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
    let connect_label = format_message!(props.intl, default_message: "Garmin activities");
    let trends_label = format_message!(props.intl, default_message: "Trends");
    let connect_destinations = [
        shell::Destination {
            label: &connect_label,
            icon: icons::CLOUD,
        },
        shell::Destination {
            label: &trends_label,
            icon: icons::ACTIVITY,
        },
    ];
    let navigation_groups = [
        shell::NavigationGroup {
            label: None,
            separated: false,
            destinations: &primary_destinations,
        },
        shell::NavigationGroup {
            label: (!device_destinations.is_empty()).then_some(devices_label.as_str()),
            separated: false,
            destinations: &device_destinations,
        },
        shell::NavigationGroup {
            label: None,
            separated: false,
            destinations: if props.garmin_connect_configured {
                &connect_destinations
            } else {
                &[]
            },
        },
        shell::NavigationGroup {
            label: None,
            separated: true,
            destinations: &[shell::Destination {
                label: &settings,
                icon: icons::GEAR,
            }],
        },
    ];
    let navigation_indices =
        navigation_indices(props.devices.len(), props.garmin_connect_configured);
    let toggle_label = format_message!(props.intl, default_message: "Toggle navigation");
    let profile_label = format_message!(props.intl, default_message: "Choose a profile");
    let profile_selector = profile_selector(props);
    let show = if matches!(props.page, Page::Device(_)) {
        shell::show
    } else {
        shell::show_edge_to_edge
    };
    let output = show(
        ui,
        &shell::Props {
            product_name: props.product_name,
            navigation_groups: &navigation_groups,
            active: navigation_indices
                .iter()
                .position(|index| Some(*index) == props.page.index(props.devices)),
            navigation: props.navigation,
            profile_selector: Some(&profile_selector),
            toggle_label: &toggle_label,
            profile_label: &profile_label,
            window_controls: props.window_controls,
        },
        page,
    );
    Output {
        action: output.action.map(|action| match action {
            shell::Action::Navigate(index) => shell::Action::Navigate(navigation_indices[index]),
            shell::Action::NavigateAndCollapse(index) => {
                shell::Action::NavigateAndCollapse(navigation_indices[index])
            }
            other => other,
        }),
        inner: output.inner,
    }
}

fn navigation_indices(devices: usize, connect_configured: bool) -> Vec<usize> {
    let mut indices = vec![0, 1];
    indices.extend(3..3 + devices);
    if connect_configured {
        indices.push(3 + devices);
        indices.push(4 + devices);
    }
    indices.push(2);
    indices
}

fn profile_selector<'a>(props: &Props<'a>) -> profile::SelectorProps<'a> {
    profile::SelectorProps {
        intl: props.intl,
        profiles: props.profiles,
        selected: Some(props.selected_profile),
        expanded: props.profile_menu_expanded,
        backup_enabled: props.backup_enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::Page;
    use garmin_service_api::{DeviceSnapshot, InspectionState};

    #[test]
    fn settings_follow_cloud_and_devices_with_or_without_a_connect_key() {
        assert_eq!(super::navigation_indices(0, true), [0, 1, 3, 4, 2]);
        assert_eq!(super::navigation_indices(0, false), [0, 1, 2]);
        assert_eq!(super::navigation_indices(2, true), [0, 1, 3, 4, 5, 6, 2]);
        assert_eq!(super::navigation_indices(2, false), [0, 1, 3, 4, 2]);
    }

    fn device(key: &str) -> DeviceSnapshot {
        DeviceSnapshot {
            key: key.to_owned(),
            name: key.to_owned(),
            identifier: None,
            software_version: None,
            inspection: InspectionState::Ready,
            inspection_error: None,
            report: None,
            capabilities: Vec::new(),
            storages: Vec::new(),
        }
    }

    #[test]
    fn page_indices_keep_primary_destinations_before_devices() {
        let devices = [device("mock-cycle"), device("mock-watch")];

        assert_eq!(Page::Activities.index(&devices), Some(0));
        assert_eq!(Page::Routes.index(&devices), Some(1));
        assert_eq!(Page::ProfileSettings.index(&devices), Some(2));
        assert_eq!(
            Page::Device("mock-watch".to_owned()).index(&devices),
            Some(4)
        );
        assert_eq!(
            Page::from_index(3, &devices),
            Some(Page::Device("mock-cycle".to_owned()))
        );
        assert_eq!(Page::Backup.index(&devices), None);
        assert_eq!(Page::GarminConnect.index(&devices), Some(5));
        assert_eq!(Page::from_index(5, &devices), Some(Page::GarminConnect));
    }
}
