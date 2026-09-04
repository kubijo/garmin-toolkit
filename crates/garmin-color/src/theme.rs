//! Semantic themes derived from Carbon; see the third-party notices.

use thiserror::Error;

use crate::{Color, swatch};

/// Surface nesting level.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Level {
    One,
    Two,
    Three,
}

impl Level {
    /// Parses a one-based layer number.
    /// # Errors
    /// [`LevelError`] unless `value` is within `1..=3`.
    pub const fn from_u8(value: u8) -> Result<Self, LevelError> {
        match value {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            3 => Ok(Self::Three),
            _ => Err(LevelError(value)),
        }
    }

    #[must_use]
    pub const fn as_u8(&self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
        }
    }

    #[must_use]
    pub const fn into_u8(self) -> u8 {
        self.as_u8()
    }

    const fn index(self) -> usize {
        match self {
            Self::One => 0,
            Self::Two => 1,
            Self::Three => 2,
        }
    }
}

/// Invalid surface level.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("{0} is not a theme surface level")]
pub struct LevelError(u8);

/// Surface and input colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surfaces {
    chrome: Color,
    background: Color,
    background_hover: Color,
    layers: [Color; 3],
    layer_hovers: [Color; 3],
    fields: [Color; 3],
    field_hovers: [Color; 3],
}

impl Surfaces {
    #[must_use]
    pub const fn chrome(&self) -> Color {
        self.chrome
    }

    #[must_use]
    pub const fn background(&self) -> Color {
        self.background
    }

    #[must_use]
    pub const fn background_hover(&self) -> Color {
        self.background_hover
    }

    #[must_use]
    pub const fn layer(&self, level: Level) -> Color {
        self.layers[level.index()]
    }

    #[must_use]
    pub const fn layer_hover(&self, level: Level) -> Color {
        self.layer_hovers[level.index()]
    }

    #[must_use]
    pub const fn field(&self, level: Level) -> Color {
        self.fields[level.index()]
    }

    #[must_use]
    pub const fn field_hover(&self, level: Level) -> Color {
        self.field_hovers[level.index()]
    }
}

/// Text and icon colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Content {
    text_primary: Color,
    text_secondary: Color,
    text_helper: Color,
    text_disabled: Color,
    icon_primary: Color,
    icon_secondary: Color,
    icon_disabled: Color,
}

impl Content {
    #[must_use]
    pub const fn text_primary(&self) -> Color {
        self.text_primary
    }

    #[must_use]
    pub const fn text_secondary(&self) -> Color {
        self.text_secondary
    }

    #[must_use]
    pub const fn text_helper(&self) -> Color {
        self.text_helper
    }

    #[must_use]
    pub const fn text_disabled(&self) -> Color {
        self.text_disabled
    }

    #[must_use]
    pub const fn icon_primary(&self) -> Color {
        self.icon_primary
    }

    #[must_use]
    pub const fn icon_secondary(&self) -> Color {
        self.icon_secondary
    }

    #[must_use]
    pub const fn icon_disabled(&self) -> Color {
        self.icon_disabled
    }
}

/// Interactive and linked-content colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interaction {
    interactive: Color,
    brand: Color,
    focus: Color,
    link: Color,
    link_hover: Color,
    link_visited: Color,
}

impl Interaction {
    #[must_use]
    pub const fn interactive(&self) -> Color {
        self.interactive
    }

    #[must_use]
    pub const fn brand(&self) -> Color {
        self.brand
    }

    #[must_use]
    pub const fn focus(&self) -> Color {
        self.focus
    }

    #[must_use]
    pub const fn link(&self) -> Color {
        self.link
    }

    #[must_use]
    pub const fn link_hover(&self) -> Color {
        self.link_hover
    }

    #[must_use]
    pub const fn link_visited(&self) -> Color {
        self.link_visited
    }
}

/// Status colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Support {
    error: Color,
    warning: Color,
    success: Color,
    information: Color,
}

impl Support {
    #[must_use]
    pub const fn error(&self) -> Color {
        self.error
    }

    #[must_use]
    pub const fn warning(&self) -> Color {
        self.warning
    }

    #[must_use]
    pub const fn success(&self) -> Color {
        self.success
    }

    #[must_use]
    pub const fn information(&self) -> Color {
        self.information
    }
}

/// Border colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Borders {
    subtle: Color,
    strong: Color,
    interactive: Color,
}

/// Button colors for one interaction state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonState {
    background: Color,
    foreground: Color,
    border: Color,
}

impl ButtonState {
    #[must_use]
    pub const fn background(&self) -> Color {
        self.background
    }

    #[must_use]
    pub const fn foreground(&self) -> Color {
        self.foreground
    }

    #[must_use]
    pub const fn border(&self) -> Color {
        self.border
    }
}

/// Interaction states for one button kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonStates {
    rest: ButtonState,
    hover: ButtonState,
    active: ButtonState,
    disabled: ButtonState,
}

impl ButtonStates {
    #[must_use]
    pub const fn rest(&self) -> &ButtonState {
        &self.rest
    }

    #[must_use]
    pub const fn hover(&self) -> &ButtonState {
        &self.hover
    }

    #[must_use]
    pub const fn active(&self) -> &ButtonState {
        &self.active
    }

    #[must_use]
    pub const fn disabled(&self) -> &ButtonState {
        &self.disabled
    }
}

/// Semantic button colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Buttons {
    primary: ButtonStates,
    secondary: ButtonStates,
    tertiary: ButtonStates,
    ghost: ButtonStates,
    danger: ButtonStates,
}

/// Action colors used by modal footers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModalActions {
    cancel: ButtonStates,
    confirm: ButtonStates,
    danger: ButtonStates,
}

impl ModalActions {
    #[must_use]
    pub const fn cancel(&self) -> &ButtonStates {
        &self.cancel
    }

    #[must_use]
    pub const fn confirm(&self) -> &ButtonStates {
        &self.confirm
    }

    #[must_use]
    pub const fn danger(&self) -> &ButtonStates {
        &self.danger
    }
}

impl Buttons {
    #[must_use]
    pub const fn primary(&self) -> &ButtonStates {
        &self.primary
    }

    #[must_use]
    pub const fn secondary(&self) -> &ButtonStates {
        &self.secondary
    }

    #[must_use]
    pub const fn tertiary(&self) -> &ButtonStates {
        &self.tertiary
    }

    #[must_use]
    pub const fn ghost(&self) -> &ButtonStates {
        &self.ghost
    }

    #[must_use]
    pub const fn danger(&self) -> &ButtonStates {
        &self.danger
    }
}

impl Borders {
    #[must_use]
    pub const fn subtle(&self) -> Color {
        self.subtle
    }

    #[must_use]
    pub const fn strong(&self) -> Color {
        self.strong
    }

    #[must_use]
    pub const fn interactive(&self) -> Color {
        self.interactive
    }
}

/// Backend-neutral semantic theme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    dark: bool,
    surfaces: Surfaces,
    content: Content,
    interaction: Interaction,
    support: Support,
    borders: Borders,
    buttons: Buttons,
    modal_actions: ModalActions,
    overlay: Color,
}

impl Theme {
    #[must_use]
    pub const fn is_dark(&self) -> bool {
        self.dark
    }

    #[must_use]
    pub const fn surfaces(&self) -> &Surfaces {
        &self.surfaces
    }

    #[must_use]
    pub const fn content(&self) -> &Content {
        &self.content
    }

    #[must_use]
    pub const fn interaction(&self) -> &Interaction {
        &self.interaction
    }

    #[must_use]
    pub const fn support(&self) -> &Support {
        &self.support
    }

    #[must_use]
    pub const fn borders(&self) -> &Borders {
        &self.borders
    }

    #[must_use]
    pub const fn buttons(&self) -> &Buttons {
        &self.buttons
    }

    #[must_use]
    pub const fn modal_actions(&self) -> &ModalActions {
        &self.modal_actions
    }

    #[must_use]
    pub const fn overlay(&self) -> Color {
        self.overlay
    }
}

/// Garmin Toolkit's Gray 100 color baseline.
pub const GRAY_100: Theme = Theme {
    dark: true,
    surfaces: Surfaces {
        chrome: swatch::BLACK,
        background: swatch::gray::G100,
        background_hover: swatch::gray::G50.with_alpha(0x29),
        layers: [swatch::gray::G90, swatch::gray::G80, swatch::gray::G70],
        layer_hovers: [
            Color::from_rgb(0x33, 0x33, 0x33),
            Color::from_rgb(0x47, 0x47, 0x47),
            Color::from_rgb(0x63, 0x63, 0x63),
        ],
        fields: [swatch::gray::G90, swatch::gray::G80, swatch::gray::G70],
        field_hovers: [
            Color::from_rgb(0x33, 0x33, 0x33),
            Color::from_rgb(0x47, 0x47, 0x47),
            Color::from_rgb(0x63, 0x63, 0x63),
        ],
    },
    content: Content {
        text_primary: swatch::gray::G10,
        text_secondary: swatch::gray::G30,
        text_helper: swatch::gray::G40,
        text_disabled: swatch::gray::G10.with_alpha(0x40),
        icon_primary: swatch::gray::G10,
        icon_secondary: swatch::gray::G30,
        icon_disabled: swatch::gray::G10.with_alpha(0x40),
    },
    interaction: Interaction {
        interactive: swatch::ACTION,
        brand: swatch::ACTION,
        focus: swatch::WHITE,
        link: swatch::blue::G40,
        link_hover: swatch::blue::G30,
        link_visited: swatch::purple::G40,
    },
    support: Support {
        error: swatch::red::G50,
        warning: swatch::yellow::G30,
        success: swatch::green::G40,
        information: swatch::blue::G50,
    },
    borders: Borders {
        subtle: swatch::gray::G70.with_alpha(0x80),
        strong: swatch::gray::G60,
        interactive: swatch::ACTION,
    },
    buttons: Buttons {
        primary: ButtonStates {
            rest: ButtonState {
                background: swatch::ACTION,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::ACTION_HOVER,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::ACTION_ACTIVE,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G50.with_alpha(0x4d),
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        secondary: ButtonStates {
            rest: ButtonState {
                background: swatch::gray::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: Color::from_rgb(0x5e, 0x5e, 0x5e),
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G50.with_alpha(0x4d),
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        tertiary: ButtonStates {
            rest: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::WHITE,
                border: swatch::WHITE,
            },
            hover: ButtonState {
                background: swatch::gray::G10,
                foreground: swatch::gray::G100,
                border: swatch::gray::G10,
            },
            active: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::gray::G50.with_alpha(0x4d),
            },
        },
        ghost: ButtonStates {
            rest: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::blue::G40,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::gray::G50.with_alpha(0x29),
                foreground: swatch::blue::G30,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::gray::G50.with_alpha(0x66),
                foreground: swatch::blue::G30,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        danger: ButtonStates {
            rest: ButtonState {
                background: swatch::red::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: Color::from_rgb(0xb8, 0x19, 0x21),
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::red::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G50.with_alpha(0x4d),
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
    },
    modal_actions: ModalActions {
        cancel: ButtonStates {
            rest: ButtonState {
                background: swatch::gray::G70,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::gray::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        confirm: ButtonStates {
            rest: ButtonState {
                background: swatch::ACTION,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::ACTION_HOVER,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::ACTION_ACTIVE,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        danger: ButtonStates {
            rest: ButtonState {
                background: swatch::red::G70,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::red::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::red::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
    },
    overlay: swatch::BLACK.with_alpha(0x99),
};

/// Garmin Toolkit's Gray 10 color baseline.
pub const GRAY_10: Theme = Theme {
    dark: false,
    surfaces: Surfaces {
        chrome: swatch::gray::G20,
        background: swatch::gray::G10,
        background_hover: swatch::gray::G50.with_alpha(0x1f),
        layers: [swatch::WHITE, swatch::gray::G10, swatch::WHITE],
        layer_hovers: [
            Color::from_rgb(0xe8, 0xe8, 0xe8),
            Color::from_rgb(0xe8, 0xe8, 0xe8),
            Color::from_rgb(0xe8, 0xe8, 0xe8),
        ],
        fields: [swatch::WHITE, swatch::gray::G10, swatch::WHITE],
        field_hovers: [
            Color::from_rgb(0xe8, 0xe8, 0xe8),
            Color::from_rgb(0xe8, 0xe8, 0xe8),
            Color::from_rgb(0xe8, 0xe8, 0xe8),
        ],
    },
    content: Content {
        text_primary: swatch::gray::G100,
        text_secondary: swatch::gray::G70,
        text_helper: swatch::gray::G60,
        text_disabled: swatch::gray::G100.with_alpha(0x40),
        icon_primary: swatch::gray::G100,
        icon_secondary: swatch::gray::G70,
        icon_disabled: swatch::gray::G100.with_alpha(0x40),
    },
    interaction: Interaction {
        interactive: swatch::ACTION,
        brand: swatch::ACTION,
        focus: swatch::ACTION,
        link: swatch::ACTION,
        link_hover: swatch::ACTION_ACTIVE,
        link_visited: swatch::purple::G60,
    },
    support: Support {
        error: swatch::red::G60,
        warning: swatch::yellow::G30,
        success: swatch::green::G50,
        information: swatch::blue::G70,
    },
    borders: Borders {
        subtle: swatch::gray::G30.with_alpha(0x80),
        strong: swatch::gray::G50,
        interactive: swatch::ACTION,
    },
    buttons: Buttons {
        primary: ButtonStates {
            rest: ButtonState {
                background: swatch::ACTION,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::ACTION_HOVER,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::ACTION_ACTIVE,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        secondary: ButtonStates {
            rest: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::gray::G70,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::gray::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        tertiary: ButtonStates {
            rest: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::ACTION,
                border: swatch::ACTION,
            },
            hover: ButtonState {
                background: swatch::ACTION,
                foreground: swatch::WHITE,
                border: swatch::ACTION,
            },
            active: ButtonState {
                background: swatch::ACTION_ACTIVE,
                foreground: swatch::WHITE,
                border: swatch::ACTION_ACTIVE,
            },
            disabled: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::gray::G50.with_alpha(0x40),
            },
        },
        ghost: ButtonStates {
            rest: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::ACTION,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::gray::G50.with_alpha(0x1f),
                foreground: swatch::ACTION_ACTIVE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::gray::G50.with_alpha(0x52),
                foreground: swatch::ACTION_ACTIVE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::TRANSPARENT,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        danger: ButtonStates {
            rest: ButtonState {
                background: swatch::red::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::red::G70,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::red::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
    },
    modal_actions: ModalActions {
        cancel: ButtonStates {
            rest: ButtonState {
                background: swatch::gray::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::gray::G70,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::gray::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        confirm: ButtonStates {
            rest: ButtonState {
                background: swatch::ACTION,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::ACTION_HOVER,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::ACTION_ACTIVE,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
        danger: ButtonStates {
            rest: ButtonState {
                background: swatch::red::G60,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            hover: ButtonState {
                background: swatch::red::G70,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            active: ButtonState {
                background: swatch::red::G80,
                foreground: swatch::WHITE,
                border: swatch::TRANSPARENT,
            },
            disabled: ButtonState {
                background: swatch::gray::G30,
                foreground: swatch::gray::G100.with_alpha(0x40),
                border: swatch::TRANSPARENT,
            },
        },
    },
    overlay: swatch::BLACK.with_alpha(0x99),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_roles_share_one_base_color() {
        for theme in [&GRAY_100, &GRAY_10] {
            let action = theme.interaction().interactive();

            assert_eq!(theme.interaction().brand(), action);
            assert_eq!(theme.borders().interactive(), action);
            assert_eq!(theme.buttons().primary().rest().background(), action);
            assert_eq!(theme.modal_actions().confirm().rest().background(), action);
        }
    }
}
