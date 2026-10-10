//! Native desktop interactions and the boundary to host executables.

use std::{ffi::OsString, process::Command};

use eframe::egui;

const HOST_VARIABLES: &[&str] = &["LD_LIBRARY_PATH", "GIO_EXTRA_MODULES", "XDG_DATA_DIRS"];

#[derive(Clone, Default)]
struct Headless {
    attempted: Option<&'static str>,
}

fn allowed(context: &egui::Context, operation: &'static str) -> bool {
    context.data_mut(|data| {
        let id = egui::Id::new("native-interactions");
        let Some(mut headless) = data.get_temp::<Headless>(id) else {
            return true;
        };
        headless.attempted.get_or_insert(operation);
        data.insert_temp(id, headless);
        false
    })
}

pub(crate) fn file_dialog(context: &egui::Context) -> Option<rfd::FileDialog> {
    allowed(context, "file dialog").then(rfd::FileDialog::new)
}

pub(crate) fn folder_command(context: &egui::Context) -> Option<Command> {
    if !allowed(context, "open folder") {
        return None;
    }
    let mut command = Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    restore_host_environment(&mut command, |key| std::env::var_os(key));
    Some(command)
}

fn restore_host_environment(command: &mut Command, value: impl Fn(&str) -> Option<OsString>) {
    for variable in HOST_VARIABLES {
        let saved = format!("NIX_APPIMAGE_HOST_{variable}");
        let present = format!("{saved}_SET");
        match value(&present).as_deref() {
            Some(marker) if marker == "x" => {
                command.env(variable, value(&saved).unwrap_or_default());
            }
            Some(_) => {
                command.env_remove(variable);
            }
            None => continue, // Ordinary native execution has no AppImage snapshot.
        }
        command.env_remove(saved).env_remove(present);
    }
}

#[cfg(feature = "render-probe")]
pub(crate) fn forbid(context: &egui::Context) {
    context.data_mut(|data| {
        data.insert_temp(egui::Id::new("native-interactions"), Headless::default());
    });
}

#[cfg(feature = "render-probe")]
pub(crate) fn check(context: &egui::Context) -> std::io::Result<()> {
    let attempted = context.data(|data| {
        data.get_temp::<Headless>(egui::Id::new("native-interactions"))
            .and_then(|headless| headless.attempted)
    });
    match attempted {
        Some(operation) => Err(std::io::Error::other(format!(
            "headless probe does not support native interaction: {operation}"
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_environment_preserves_unset_empty_and_custom_values() {
        let mut command = Command::new("unused");
        restore_host_environment(&mut command, |key| {
            match key {
                "NIX_APPIMAGE_HOST_LD_LIBRARY_PATH_SET" | "NIX_APPIMAGE_HOST_GIO_EXTRA_MODULES" => {
                    Some("")
                }
                "NIX_APPIMAGE_HOST_GIO_EXTRA_MODULES_SET"
                | "NIX_APPIMAGE_HOST_XDG_DATA_DIRS_SET" => Some("x"),
                "NIX_APPIMAGE_HOST_XDG_DATA_DIRS" => Some("/host/share"),
                _ => None,
            }
            .map(OsString::from)
        });
        let changes: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(changes[std::ffi::OsStr::new("LD_LIBRARY_PATH")], None);
        assert_eq!(
            changes[std::ffi::OsStr::new("GIO_EXTRA_MODULES")],
            Some(std::ffi::OsStr::new(""))
        );
        assert_eq!(
            changes[std::ffi::OsStr::new("XDG_DATA_DIRS")],
            Some(std::ffi::OsStr::new("/host/share"))
        );
    }

    #[test]
    fn ordinary_execution_keeps_the_environment() {
        let mut command = Command::new("unused");
        restore_host_environment(&mut command, |_| None);
        assert_eq!(command.get_envs().count(), 0);
    }

    #[cfg(feature = "render-probe")]
    #[test]
    fn headless_interactions_fail_without_opening_a_dialog_or_process() {
        let context = egui::Context::default();
        forbid(&context);
        assert!(file_dialog(&context).is_none());
        assert!(folder_command(&context).is_none());
        assert!(
            check(&context)
                .unwrap_err()
                .to_string()
                .contains("file dialog")
        );
    }
}
