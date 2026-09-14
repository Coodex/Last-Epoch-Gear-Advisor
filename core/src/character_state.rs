//! Character state (`character.json`) and the local equipped-gear profile.
//!
//! No online data: the character API only exposes equipped gear, and what we
//! need compared is the inventory. The user maintains a small JSON file with
//! the few facts the guide's conditions depend on, and stores equipped items
//! per slot by pasting or capturing their tooltips.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use crate::game_data::Slot;
use crate::item_parser::ParsedItem;

pub const ELEMENTS: [&str; 7] = ["fire", "cold", "lightning", "physical", "necrotic", "void", "poison"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Early,
    Intermediate,
    Final,
}

impl Phase {
    /// Phase from level, following the guide's brackets: 1-26 early,
    /// 27-37 intermediate, 38+ final.
    pub fn from_level(level: u32) -> Phase {
        if level >= 38 {
            Phase::Final
        } else if level >= 27 {
            Phase::Intermediate
        } else {
            Phase::Early
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Phase::Early => "early",
            Phase::Intermediate => "intermediate",
            Phase::Final => "final",
        })
    }
}

/// `character.json`. Every field has a default so a partial file works.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterState {
    pub level: u32,
    /// Overrides the level-derived phase when set.
    pub phase: Option<Phase>,
    /// Current resistances in percent, per element (fire, cold, lightning,
    /// physical, necrotic, void, poison), as shown on the character sheet.
    pub resistances: HashMap<String, f64>,
    pub endurance: f64,
    /// maximum Health and Mana from the sheet (0 = not read yet)
    pub health: f64,
    pub mana: f64,
    pub heavens_bulwark_points: u32,
    pub healing_hands_specced: bool,
    pub solarum_plate_equipped: bool,
    pub nagasa_scymitar_equipped: bool,
    /// Build-specific yes/no facts declared by the active guide profile.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub flags: HashMap<String, bool>,
    /// Build-specific counters (skill points etc.) declared by the active guide profile.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub counters: HashMap<String, u32>,
}

impl Default for CharacterState {
    fn default() -> Self {
        CharacterState {
            level: 1,
            phase: None,
            resistances: ELEMENTS.iter().map(|e| (e.to_string(), 0.0)).collect(),
            endurance: 0.0,
            health: 0.0,
            mana: 0.0,
            heavens_bulwark_points: 0,
            healing_hands_specced: false,
            solarum_plate_equipped: false,
            nagasa_scymitar_equipped: false,
            flags: HashMap::new(),
            counters: HashMap::new(),
        }
    }
}

impl CharacterState {
    pub fn load(path: &Path) -> anyhow::Result<CharacterState> {
        let text = std::fs::read_to_string(path)?;
        let state: CharacterState = serde_json::from_str(&text)?;
        Ok(state)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn phase(&self) -> Phase {
        self.phase.unwrap_or_else(|| Phase::from_level(self.level))
    }

    pub fn flag(&self, key: &str) -> bool {
        self.flags.get(key).copied().unwrap_or(false)
    }

    pub fn counter(&self, key: &str) -> u32 {
        self.counters.get(key).copied().unwrap_or(0)
    }

    pub fn resistance(&self, element: &str) -> f64 {
        self.resistances.get(element).copied().unwrap_or(0.0)
    }

    /// Ordered list of (element, value) for display.
    pub fn resistance_list(&self) -> Vec<(&'static str, f64)> {
        ELEMENTS.iter().map(|e| (*e, self.resistance(e))).collect()
    }
}

/// One stored equipped item: the parsed form plus the text it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquippedItem {
    pub item: ParsedItem,
    #[serde(default)]
    pub source_text: String,
}

/// Equipped gear per slot (`profile/equipped.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EquippedProfile {
    #[serde(default)]
    pub slots: HashMap<Slot, EquippedItem>,
}

impl EquippedProfile {
    pub fn load(path: &Path) -> anyhow::Result<EquippedProfile> {
        if !path.exists() {
            return Ok(EquippedProfile::default());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn get(&self, slot: Slot) -> Option<&ParsedItem> {
        self.slots.get(&slot).map(|e| &e.item)
    }

    pub fn set(&mut self, slot: Slot, item: ParsedItem, source_text: String) {
        self.slots.insert(slot, EquippedItem { item, source_text });
    }

    pub fn remove(&mut self, slot: Slot) -> bool {
        self.slots.remove(&slot).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_from_level_follows_the_brackets() {
        assert_eq!(Phase::from_level(1), Phase::Early);
        assert_eq!(Phase::from_level(26), Phase::Early);
        assert_eq!(Phase::from_level(27), Phase::Intermediate);
        assert_eq!(Phase::from_level(37), Phase::Intermediate);
        assert_eq!(Phase::from_level(38), Phase::Final);
    }

    #[test]
    fn partial_character_json_uses_defaults() {
        let state: CharacterState = serde_json::from_str(r#"{"level": 49, "resistances": {"fire": 62}}"#).unwrap();
        assert_eq!(state.level, 49);
        assert_eq!(state.phase(), Phase::Final);
        assert_eq!(state.resistance("fire"), 62.0);
        assert_eq!(state.resistance("cold"), 0.0);
        assert!(!state.healing_hands_specced);
        let overridden: CharacterState = serde_json::from_str(r#"{"level": 49, "phase": "intermediate"}"#).unwrap();
        assert_eq!(overridden.phase(), Phase::Intermediate);
    }
}
