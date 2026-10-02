//! Developer tools rendered through the desktop window implementation.
use super::state;
use crate::window::{Event, NativeWindow, Spec, WindowHost};
use egui::Context;
use std::sync::{Arc, Mutex};

#[cfg(any(test, feature = "automation"))]
type Command = super::AutomationRequest;
#[cfg(not(any(test, feature = "automation")))]
type Command = ();
type Host = Arc<Mutex<NativeWindow<Command, ()>>>;

pub(super) fn show(context: &Context, intl: &garmin_i18n::Intl) {
    let host = context.data_mut(|data| {
        data.get_temp_mut_or_default::<Host>(egui::Id::new("developer-window-host"))
            .clone()
    });
    let mut host = host
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let handle = state(context);
    let focus_requested = {
        let mut state = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.native = true;
        let size = context.viewport_rect().size();
        state.viewport = [size.x, size.y];
        state.scale = context.pixels_per_point();
        if !state.open {
            host.close(context);
            return;
        }
        #[cfg(any(test, feature = "automation"))]
        {
            state.remote_automation = Some(super::automation::snapshot(context));
        }
        std::mem::take(&mut state.focus_requested)
    };
    if !host.is_open() || focus_requested {
        // Native window creation is infallible; the host reports subsequent close events.
        let _ = host.open(
            context,
            Spec {
                id: "developer-tools-window".into(),
                kind: "developer-tools".into(),
                title: "Developer tools".into(),
                size: [680.0, 640.0],
            },
        );
    }
    let events = host.present(
        context,
        intl,
        || (),
        |ui| {
            let mut state = handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            super::contents(ui, &mut state);
            #[cfg(any(test, feature = "automation"))]
            {
                state
                    .remote_automation
                    .as_mut()
                    .and_then(|view| view.requests.pop())
            }
            #[cfg(not(any(test, feature = "automation")))]
            {
                None
            }
        },
    );
    for event in events {
        match event {
            Event::Closed => {
                handle
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .open = false;
            }
            Event::Command { command, .. } => {
                #[cfg(any(test, feature = "automation"))]
                super::automation::dispatch(context, command);
                #[cfg(not(any(test, feature = "automation")))]
                let () = command;
            }
        }
        context.request_repaint();
    }
}
