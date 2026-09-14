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
///
/// Build-specific facts (skill points, "is X equipped") live in `flags` and
/// `counters`, declared by the active guide profile's `facts`. Files written
/// before 0.2 carried the built-in Paladin facts as top-level keys; those are
/// migrated into the maps on load (see `RawCharacterState`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, from = "RawCharacterState")]
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
    /// Build-specific yes/no facts declared by the active guide profile.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub flags: HashMap<String, bool>,
    /// Build-specific counters (skill points etc.) declared by the active guide profile.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub counters: HashMap<String, u32>,
}

/// Legacy top-level keys of the built-in Paladin profile's facts (pre-0.2
/// `character.json`), now ordinary counters / flags with the same names.
pub const LEGACY_COUNTER_KEYS: [&str; 1] = ["heavens_bulwark_points"];
pub const LEGACY_FLAG_KEYS: [&str; 3] = ["healing_hands_specced", "solarum_plate_equipped", "nagasa_scymitar_equipped"];

/// On-disk shape: the current fields plus the legacy top-level facts, so an
/// old file still loads and its facts land in `counters` / `flags`. A value
/// already present in the maps wins over the legacy key, and a legacy key at
/// its default (0 / false) is dropped: a missing counter reads as 0 and a
/// missing flag as false anyway.
#[derive(Deserialize)]
#[serde(default)]
struct RawCharacterState {
    level: u32,
    phase: Option<Phase>,
    resistances: HashMap<String, f64>,
    endurance: f64,
    health: f64,
    mana: f64,
    flags: HashMap<String, bool>,
    counters: HashMap<String, u32>,
    heavens_bulwark_points: Option<u32>,
    healing_hands_specced: Option<bool>,
    solarum_plate_equipped: Option<bool>,
    nagasa_scymitar_equipped: Option<bool>,
}

impl Default for RawCharacterState {
    fn default() -> Self {
        let base = CharacterState::default();
        RawCharacterState {
            level: base.level,
            phase: base.phase,
            resistances: base.resistances,
            endurance: base.endurance,
            health: base.health,
            mana: base.mana,
            flags: base.flags,
            counters: base.counters,
            heavens_bulwark_points: None,
            healing_hands_specced: None,
            solarum_plate_equipped: None,
            nagasa_scymitar_equipped: None,
        }
    }
}

impl From<RawCharacterState> for CharacterState {
    fn from(raw: RawCharacterState) -> Self {
        let mut flags = raw.flags;
        let mut counters = raw.counters;
        if let Some(v) = raw.heavens_bulwark_points.filter(|v| *v != 0) {
            counters.entry(LEGACY_COUNTER_KEYS[0].into()).or_insert(v);
        }
        for (key, value) in LEGACY_FLAG_KEYS.iter().zip([raw.healing_hands_specced, raw.solarum_plate_equipped, raw.nagasa_scymitar_equipped]) {
            if value == Some(true) {
                flags.entry((*key).into()).or_insert(true);
            }
        }
        CharacterState {
            level: raw.level,
            phase: raw.phase,
            resistances: raw.resistances,
            endurance: raw.endurance,
            health: raw.health,
            mana: raw.mana,
            flags,
            counters,
        }
    }
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
        assert!(!state.flag("healing_hands_specced"));
        let overridden: CharacterState = serde_json::from_str(r#"{"level": 49, "phase": "intermediate"}"#).unwrap();
        assert_eq!(overridden.phase(), Phase::Intermediate);
    }

    #[test]
    fn legacy_paladin_keys_migrate_into_facts() {
        let old = r#"{"level": 40, "heavens_bulwark_points": 5, "healing_hands_specced": true,
                      "solarum_plate_equipped": false, "nagasa_scymitar_equipped": true}"#;
        let state: CharacterState = serde_json::from_str(old).unwrap();
        assert_eq!(state.counter("heavens_bulwark_points"), 5);
        assert!(state.flag("healing_hands_specced"));
        assert!(!state.flag("solarum_plate_equipped"));
        assert!(!state.flags.contains_key("solarum_plate_equipped")); // default value: not carried over
        assert!(state.flag("nagasa_scymitar_equipped"));
        // saved again, the legacy keys are gone and the maps carry the values
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("\"healing_hands_specced\":true,\"solarum"));
        assert!(json.contains("\"counters\""));
        assert!(json.contains("\"flags\""));
        let reloaded: CharacterState = serde_json::from_str(&json).unwrap();
        assert_eq!(reloaded.counter("heavens_bulwark_points"), 5);

        // the maps win over a stale legacy key
        let mixed = r#"{"heavens_bulwark_points": 2, "counters": {"heavens_bulwark_points": 7}}"#;
        let state: CharacterState = serde_json::from_str(mixed).unwrap();
        assert_eq!(state.counter("heavens_bulwark_points"), 7);

        // an all-default legacy file migrates to empty maps
        let defaults = r#"{"heavens_bulwark_points": 0, "healing_hands_specced": false}"#;
        let state: CharacterState = serde_json::from_str(defaults).unwrap();
        assert!(state.flags.is_empty() && state.counters.is_empty());
    }
}
