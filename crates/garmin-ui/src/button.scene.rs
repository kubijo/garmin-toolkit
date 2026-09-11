use gallery::prelude::*;
use garmin_ui::button::{GroupChoice, GroupProps, IconProps, Kind, Props, Width};
use garmin_ui::{Size, button, icons};

scene_meta! { title: "Components / Actions / Buttons" }

const KINDS: &[&str] = &["primary", "secondary", "tertiary", "ghost", "danger"];
const SIZES: &[&str] = &["small", "medium", "large"];

struct SceneProps {
    kind: Kind,
    size: Size,
    icon: bool,
    enabled: bool,
}

#[scene(default)]
fn playground(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let props = SceneProps {
        kind: kind(ctx.buttons("kind", KINDS, 0)),
        size: size(ctx.buttons("size", SIZES, 1)),
        icon: ctx.toggle("icon", true),
        enabled: ctx.toggle("enabled", true),
    };

    stage!(ctx, ui, |ui| {
        Props {
            label: "Import activities",
            icon: props.icon.then_some(icons::UPLOAD_SIMPLE),
            kind: props.kind,
            size: props.size,
            width: Width::Fit,
            enabled: props.enabled,
        }
        .show(ui);
    });
}

#[scene]
fn states(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        for (label, kind) in [
            ("Primary", Kind::Primary),
            ("Secondary", Kind::Secondary),
            ("Tertiary", Kind::Tertiary),
            ("Ghost", Kind::Ghost),
            ("Delete", Kind::Danger),
        ] {
            ui.horizontal(|ui| {
                for enabled in [true, false] {
                    Props {
                        label,
                        icon: Some(if kind == Kind::Danger {
                            icons::TRASH
                        } else {
                            icons::CHECK
                        }),
                        kind,
                        size: Size::Medium,
                        width: Width::Fit,
                        enabled,
                    }
                    .show(ui);
                }
            });
        }
    });
}

#[scene]
fn sizes(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    stage!(ctx, ui, |ui| {
        for (label, size) in [
            ("Small", Size::Small),
            ("Medium", Size::Medium),
            ("Large", Size::Large),
        ] {
            Props {
                label,
                icon: Some(icons::PLUS),
                kind: Kind::Primary,
                size,
                width: Width::Fit,
                enabled: true,
            }
            .show(ui);
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            for size in [Size::Small, Size::Medium, Size::Large] {
                IconProps {
                    label: "Add",
                    icon: icons::PLUS,
                    kind: Kind::Ghost,
                    size,
                    enabled: true,
                }
                .show(ui);
            }
        });
    });
}

#[scene]
fn group(ctx: &mut SceneCtx<'_>, ui: &mut Ui) {
    garmin_ui::theme::apply(ui.style_mut());
    let selected = ctx.buttons("selected", &["auto", "dark", "light"], 0);
    stage!(ctx, ui, |ui| {
        let choices = [
            GroupChoice::new("Auto", icons::DESKTOP, 0),
            GroupChoice::new("Dark", icons::MOON, 1),
            GroupChoice::new("Light", icons::SUN, 2),
        ];
        let _ = button::group(
            ui,
            selected,
            &choices,
            GroupProps {
                size: Size::Medium,
                enabled: true,
            },
        );
    });
}

const fn kind(index: usize) -> Kind {
    match index {
        1 => Kind::Secondary,
        2 => Kind::Tertiary,
        3 => Kind::Ghost,
        4 => Kind::Danger,
        _ => Kind::Primary,
    }
}

const fn size(index: usize) -> Size {
    match index {
        0 => Size::Small,
        2 => Size::Large,
        _ => Size::Medium,
    }
}
