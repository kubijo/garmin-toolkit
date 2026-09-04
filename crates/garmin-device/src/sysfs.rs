use crate::GARMIN_USB_VENDOR_ID;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const USB_DEVICES_PATH: &str = "/sys/bus/usb/devices";

/// A Garmin USB device visible to the Linux kernel, whether or not userspace
/// currently has permission to open its MTP interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GarminUsbDevice {
    pub sysfs_path: PathBuf,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: Option<String>,
}

#[must_use]
pub fn discover_garmin_usb_sysfs() -> Vec<GarminUsbDevice> {
    discover_under(Path::new(USB_DEVICES_PATH))
}

fn discover_under(root: &Path) -> Vec<GarminUsbDevice> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut devices = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let vendor_id = read_usb_id(&path.join("idVendor"))?;
            if vendor_id != GARMIN_USB_VENDOR_ID {
                return None;
            }
            let product_id = read_usb_id(&path.join("idProduct"))?;
            let product = read_trimmed(&path.join("product"));
            Some(GarminUsbDevice {
                sysfs_path: path,
                vendor_id,
                product_id,
                product,
            })
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.sysfs_path.cmp(&right.sysfs_path));
    devices
}

fn read_usb_id(path: &Path) -> Option<u16> {
    let value = read_trimmed(path)?;
    u16::from_str_radix(&value, 16).ok()
}

fn read_trimmed(path: &Path) -> Option<String> {
    let value = fs::read_to_string(path).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn discovers_only_garmin_usb_devices() {
        let root = tempfile::tempdir().expect("create sysfs fixture");
        let garmin = root.path().join("5-2");
        let unrelated = root.path().join("7-1");
        fs::create_dir_all(&garmin).expect("create Garmin fixture");
        fs::create_dir_all(&unrelated).expect("create unrelated fixture");
        write(&garmin.join("idVendor"), "091e\n");
        write(&garmin.join("idProduct"), "5158\n");
        write(&garmin.join("product"), "Garmin Example Device\n");
        write(&unrelated.join("idVendor"), "1234\n");
        write(&unrelated.join("idProduct"), "5678\n");

        let devices = discover_under(root.path());

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].vendor_id, 0x091e);
        assert_eq!(devices[0].product_id, 0x5158);
        assert_eq!(devices[0].product.as_deref(), Some("Garmin Example Device"));
    }

    fn write(path: &Path, contents: &str) {
        let mut file = fs::File::create(path).expect("create fixture file");
        file.write_all(contents.as_bytes())
            .expect("write fixture file");
    }
}
