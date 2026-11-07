//! Research mode presets (ADR-0017): one knob mapping to depth/breadth/
//! fetch-top starting points (tvly's mini/pro/auto, hsearch mode presets).
//! Presets are defaults, not caps: an explicit flag overrides its
//! component, and the preset overrides only config defaults.

use crate::DepthSpec;
use vygr_core::VygrError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchMode {
    /// One pass, small breadth: "is this even worth researching?"
    Quick,
    /// The default loop.
    Standard,
    /// More breadth, wider fetch, deeper range.
    Deep,
    /// Maximum coverage within the same budget machinery.
    Exhaustive,
}

impl ResearchMode {
    pub fn parse(s: &str) -> Result<Self, VygrError> {
        match s.trim().to_lowercase().as_str() {
            "quick" => Ok(Self::Quick),
            "standard" => Ok(Self::Standard),
            "deep" => Ok(Self::Deep),
            "exhaustive" => Ok(Self::Exhaustive),
            other => Err(VygrError::Config(format!(
                "invalid mode '{other}' (use quick|standard|deep|exhaustive)"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Standard => "standard",
            Self::Deep => "deep",
            Self::Exhaustive => "exhaustive",
        }
    }

    /// `(depth, breadth, fetch_top)` starting points.
    pub fn preset(&self) -> (DepthSpec, u32, usize) {
        match self {
            Self::Quick => (DepthSpec { min: 1, max: 1 }, 3, 3),
            Self::Standard => (DepthSpec { min: 2, max: 3 }, 3, 4),
            Self::Deep => (DepthSpec { min: 2, max: 4 }, 4, 6),
            Self::Exhaustive => (DepthSpec { min: 3, max: 5 }, 5, 8),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_modes_case_insensitively() {
        for (s, expected) in [
            ("quick", ResearchMode::Quick),
            ("STANDARD", ResearchMode::Standard),
            (" deep ", ResearchMode::Deep),
            ("exhaustive", ResearchMode::Exhaustive),
        ] {
            assert_eq!(ResearchMode::parse(s).unwrap(), expected);
        }
        assert!(ResearchMode::parse("turbo").is_err());
    }

    #[test]
    fn presets_are_monotonic() {
        let (_, b1, f1) = ResearchMode::Quick.preset();
        let (_, b2, f2) = ResearchMode::Exhaustive.preset();
        assert!(b2 > b1 && f2 > f1);
        let (d1, _, _) = ResearchMode::Quick.preset();
        let (d2, _, _) = ResearchMode::Exhaustive.preset();
        assert!(d2.max > d1.max);
    }
}
