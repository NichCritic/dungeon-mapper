use serde::{Deserialize, Serialize};

/// The most cover an obstacle can grant (2024 rules): the cap applied after the
/// blocked-line count is turned into a level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CoverKind {
    /// Does not block lines at all (rugs, stairs, floor stains).
    #[default]
    None,
    /// Low obstacle you can see over: at most half cover (tables, chests, creatures).
    Half,
    /// Mostly blocking with gaps: at most three-quarters (portcullis, arrow slit, brazier).
    ThreeQuarters,
    /// Solid and tall: can grant total cover (pillar, statue, bookshelf, walls).
    Full,
}

impl CoverKind {
    pub const ALL: [CoverKind; 4] = [CoverKind::None, CoverKind::Half, CoverKind::ThreeQuarters, CoverKind::Full];

    pub fn label(self) -> &'static str {
        match self {
            CoverKind::None => "None",
            CoverKind::Half => "Half",
            CoverKind::ThreeQuarters => "Three-quarters",
            CoverKind::Full => "Full",
        }
    }

    /// Highest cover level this obstacle can be responsible for.
    pub fn max_level(self) -> CoverLevel {
        match self {
            CoverKind::None => CoverLevel::None,
            CoverKind::Half => CoverLevel::Half,
            CoverKind::ThreeQuarters => CoverLevel::ThreeQuarters,
            CoverKind::Full => CoverLevel::Total,
        }
    }
}

/// Cover a target actually has from a given attacker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CoverLevel {
    #[default]
    None,
    /// +2 AC and Dexterity saves.
    Half,
    /// +5 AC and Dexterity saves; can Hide.
    ThreeQuarters,
    /// Cannot be targeted directly.
    Total,
}

impl CoverLevel {
    pub fn label(self) -> &'static str {
        match self {
            CoverLevel::None => "No Cover",
            CoverLevel::Half => "Half Cover (+2 AC)",
            CoverLevel::ThreeQuarters => "Three-Quarters Cover (+5 AC, can Hide)",
            CoverLevel::Total => "Total Cover (can't target)",
        }
    }

    /// AC / Dex save bonus granted.
    pub fn ac_bonus(self) -> i32 {
        match self {
            CoverLevel::None => 0,
            CoverLevel::Half => 2,
            CoverLevel::ThreeQuarters | CoverLevel::Total => 5,
        }
    }
}
