//! Feature-owned appearance preferences, independent of activity scheduling.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndicatorStyle {
    Ring,
    #[default]
    Edge,
    Context,
}

impl IndicatorStyle {
    pub const ALL: [Self; 3] = [Self::Ring, Self::Edge, Self::Context];

    pub fn setting(self) -> &'static str {
        match self {
            Self::Ring => "ring",
            Self::Edge => "edge",
            Self::Context => "context",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ring => "Dynamic Ring",
            Self::Edge => "Edge Progress",
            Self::Context => "Context Panel",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaLayout {
    Compact,
    #[default]
    Standard,
    Informative,
}

impl MediaLayout {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Standard, Self::Informative];

    pub fn setting(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Standard => "standard",
            Self::Informative => "informative",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Standard => "Standard",
            Self::Informative => "Informative",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentationPreferences {
    pub indicator_style: IndicatorStyle,
    pub media_layout: MediaLayout,
}
