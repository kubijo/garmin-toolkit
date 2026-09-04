/// Shared component size scale.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Size {
    Small,
    /// Standard application controls.
    #[default]
    Medium,
    Large,
}
