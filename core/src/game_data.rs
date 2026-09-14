//! Game data: affixes, item bases and uniques, embedded from `core/data/*.json`
//! (built by `tools/build_data.py` from Maxroll's `data.json`, the same source
//! LEBuildConverter's name tables come from). Everything is keyed by the
//! explicit ids in that file, never by list position.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::text::{normalize, squash};

const AFFIXES_JSON: &str = include_str!("../data/affixes.json");
const BASES_JSON: &str = include_str!("../data/bases.json");
const UNIQUES_JSON: &str = include_str!("../data/uniques.json");

/// Item types 25 and up are idols, blessings, lenses, materials.
pub const FIRST_NON_EQUIPMENT_TYPE: u32 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffixKind {
    Prefix,
    Suffix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Affix {
    pub id: u32,
    pub name: String,
    pub display_name: String,
    pub kind: AffixKind,
    /// (min, max) roll per tier, index 0 = T1. Percent stats are fractions (0.12 = 12 %).
    pub tiers: Vec<(f64, f64)>,
    pub can_roll_on: Vec<u32>,
    /// 0 = flat ("added"), 1 = increased (percent), None = complex/hybrid.
    pub modifier_type: Option<i32>,
    pub property: Option<i32>,
    #[serde(default)]
    pub tags: i64,
    #[serde(default)]
    pub special_tag: i64,
    #[serde(default)]
    pub level: u32,
    #[serde(default)]
    pub category: String,
}

impl Affix {
    pub fn max_tier(&self) -> u8 {
        self.tiers.len() as u8
    }

    pub fn can_roll_on_type(&self, item_type: u32) -> bool {
        self.can_roll_on.is_empty() || self.can_roll_on.contains(&item_type)
    }

    /// Tier whose roll range contains `value`; nearest tier otherwise.
    /// `is_percent` says the tooltip showed a `%`; fractional tier tables
    /// (max roll <= 1) are compared against value / 100.
    pub fn tier_for_value(&self, value: f64, is_percent: bool) -> Option<u8> {
        if self.tiers.is_empty() {
            return None;
        }
        let magnitude = value.abs();
        let fractional = is_percent || (self.is_fractional() && magnitude >= 1.0);
        let scaled = if fractional { magnitude / 100.0 } else { magnitude };
        for (index, (a, b)) in self.tiers.iter().enumerate() {
            let (lo, hi) = (a.min(*b), a.max(*b));
            if lo * 0.98 <= scaled && scaled <= hi * 1.02 {
                return Some(index as u8 + 1);
            }
        }
        let mut best = (1u8, f64::MAX);
        for (index, (a, b)) in self.tiers.iter().enumerate() {
            let (lo, hi) = (a.min(*b), a.max(*b));
            let dist = if scaled < lo { lo - scaled } else if scaled > hi { scaled - hi } else { 0.0 };
            if dist < best.1 {
                best = (index as u8 + 1, dist);
            }
        }
        Some(best.0)
    }

    /// Percent stats store their rolls as fractions (0.30 = 30 %). Judged on
    /// the first tier: top tiers of percent stats can exceed 1.0 (150 %).
    pub fn is_fractional(&self) -> bool {
        self.tiers.first().map_or(false, |t| t.0.max(t.1) <= 1.0)
    }

    /// Roll range of a tier in tooltip units (percent stats as percent).
    pub fn tier_range(&self, tier: u8) -> Option<(f64, f64)> {
        let (a, b) = *self.tiers.get(tier.saturating_sub(1) as usize)?;
        if self.is_fractional() {
            Some((a * 100.0, b * 100.0))
        } else {
            Some((a, b))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubType {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub level: u32,
    #[serde(default)]
    pub implicits: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseType {
    pub type_id: u32,
    pub type_name: String,
    #[serde(default)]
    pub is_weapon: bool,
    #[serde(default = "default_max_affixes")]
    pub max_affixes: u32,
    pub sub_types: Vec<SubType>,
}

fn default_max_affixes() -> u32 {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unique {
    pub id: u32,
    pub name: String,
    pub base_type: u32,
    #[serde(default)]
    pub sub_types: Vec<u32>,
    #[serde(default)]
    pub level: u32,
}

/// Equipment slots (LEBuildConverter / Maxroll planner naming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Weapon,
    Offhand,
    Head,
    Body,
    Waist,
    Hands,
    Feet,
    Neck,
    Ring1,
    Ring2,
    Relic,
}

impl Slot {
    pub const ALL: [Slot; 11] = [
        Slot::Weapon, Slot::Offhand, Slot::Head, Slot::Body, Slot::Waist, Slot::Hands,
        Slot::Feet, Slot::Neck, Slot::Ring1, Slot::Ring2, Slot::Relic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Slot::Weapon => "Weapon",
            Slot::Offhand => "Off-hand",
            Slot::Head => "Helmet",
            Slot::Body => "Body Armor",
            Slot::Waist => "Belt",
            Slot::Hands => "Gloves",
            Slot::Feet => "Boots",
            Slot::Neck => "Amulet",
            Slot::Ring1 => "Ring 1",
            Slot::Ring2 => "Ring 2",
            Slot::Relic => "Relic",
        }
    }

    /// Planner key used by Maxroll's JSON ("finger1"/"finger2" for rings).
    pub fn planner_key(self) -> &'static str {
        match self {
            Slot::Weapon => "weapon",
            Slot::Offhand => "offhand",
            Slot::Head => "head",
            Slot::Body => "body",
            Slot::Waist => "waist",
            Slot::Hands => "hands",
            Slot::Feet => "feet",
            Slot::Neck => "neck",
            Slot::Ring1 => "finger1",
            Slot::Ring2 => "finger2",
            Slot::Relic => "relic",
        }
    }

    pub fn parse(text: &str) -> Option<Slot> {
        let key = text.trim().to_lowercase().replace(['-', ' '], "_");
        Some(match key.as_str() {
            "weapon" | "main_hand" | "mainhand" => Slot::Weapon,
            "offhand" | "off_hand" | "shield" => Slot::Offhand,
            "head" | "helmet" | "helm" => Slot::Head,
            "body" | "chest" | "body_armor" | "armor" => Slot::Body,
            "waist" | "belt" => Slot::Waist,
            "hands" | "gloves" => Slot::Hands,
            "feet" | "boots" => Slot::Feet,
            "neck" | "amulet" => Slot::Neck,
            "ring1" | "finger1" | "ring_1" => Slot::Ring1,
            "ring2" | "finger2" | "ring_2" => Slot::Ring2,
            "relic" => Slot::Relic,
            _ => return None,
        })
    }

    /// Slots an item type can go into (rings have two).
    pub fn for_item_type(item_type: u32) -> &'static [Slot] {
        match item_type {
            0 => &[Slot::Head],
            1 => &[Slot::Body],
            2 => &[Slot::Waist],
            3 => &[Slot::Feet],
            4 => &[Slot::Hands],
            5..=16 | 23 | 24 => &[Slot::Weapon],
            17..=19 => &[Slot::Offhand],
            20 => &[Slot::Neck],
            21 => &[Slot::Ring1, Slot::Ring2],
            22 => &[Slot::Relic],
            _ => &[],
        }
    }
}

pub struct GameData {
    affixes: HashMap<u32, Affix>,
    bases: HashMap<u32, BaseType>,
    uniques: HashMap<u32, Unique>,
    /// normalised type name -> type id (equipment only)
    type_names: Vec<(String, u32)>,
    /// (type id, sub type id, squashed base name)
    base_names: Vec<(u32, u32, String)>,
    unique_names: Vec<(u32, String)>,
    affix_text: HashMap<u32, (String, String)>, // id -> (spaced, squashed) match text
}

static EMBEDDED: Lazy<GameData> = Lazy::new(|| {
    GameData::from_json(AFFIXES_JSON, BASES_JSON, UNIQUES_JSON).expect("embedded game data is valid")
});

impl GameData {
    /// The data compiled into the binary.
    pub fn embedded() -> &'static GameData {
        &EMBEDDED
    }

    pub fn from_json(affixes: &str, bases: &str, uniques: &str) -> anyhow::Result<GameData> {
        let affix_list: Vec<Affix> = serde_json::from_str(affixes)?;
        let base_list: Vec<BaseType> = serde_json::from_str(bases)?;
        let unique_list: Vec<Unique> = serde_json::from_str(uniques)?;
        let mut data = GameData {
            affixes: HashMap::new(),
            bases: HashMap::new(),
            uniques: HashMap::new(),
            type_names: Vec::new(),
            base_names: Vec::new(),
            unique_names: Vec::new(),
            affix_text: HashMap::new(),
        };
        for affix in affix_list {
            let equipment_only = affix.can_roll_on.iter().any(|t| *t < FIRST_NON_EQUIPMENT_TYPE);
            if affix.can_roll_on.is_empty() || equipment_only {
                let spaced = crate::text::strip_stop_words(&normalize(&affix.display_name));
                data.affix_text.insert(affix.id, (spaced.clone(), spaced.replace(' ', "")));
            }
            data.affixes.insert(affix.id, affix);
        }
        for base in base_list {
            if base.type_id < FIRST_NON_EQUIPMENT_TYPE && !base.type_name.is_empty() {
                data.type_names.push((normalize(&base.type_name), base.type_id));
                for sub in &base.sub_types {
                    data.base_names.push((base.type_id, sub.id, squash(&sub.name)));
                }
            }
            data.bases.insert(base.type_id, base);
        }
        for unique in unique_list {
            if unique.base_type < FIRST_NON_EQUIPMENT_TYPE {
                data.unique_names.push((unique.id, squash(&unique.name)));
            }
            data.uniques.insert(unique.id, unique);
        }
        Ok(data)
    }

    pub fn affix(&self, id: u32) -> Option<&Affix> {
        self.affixes.get(&id)
    }

    pub fn affixes(&self) -> impl Iterator<Item = &Affix> {
        self.affixes.values()
    }

    pub fn affix_name(&self, id: u32) -> String {
        self.affixes.get(&id).map(|a| a.display_name.clone()).unwrap_or_else(|| format!("#a{id}"))
    }

    /// Match text (spaced, squashed) for affixes that can appear on gear.
    pub fn affix_match_text(&self, id: u32) -> Option<&(String, String)> {
        self.affix_text.get(&id)
    }

    pub fn base_type(&self, type_id: u32) -> Option<&BaseType> {
        self.bases.get(&type_id)
    }

    pub fn type_name(&self, type_id: u32) -> String {
        self.bases.get(&type_id).map(|b| b.type_name.clone()).unwrap_or_else(|| format!("#type{type_id}"))
    }

    pub fn sub_type(&self, type_id: u32, sub_type: u32) -> Option<&SubType> {
        self.bases.get(&type_id)?.sub_types.iter().find(|s| s.id == sub_type)
    }

    pub fn base_name(&self, type_id: u32, sub_type: u32) -> String {
        self.sub_type(type_id, sub_type)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("#{type_id}/{sub_type}"))
    }

    pub fn unique(&self, id: u32) -> Option<&Unique> {
        self.uniques.get(&id)
    }

    pub fn type_names(&self) -> &[(String, u32)] {
        &self.type_names
    }

    pub fn base_names(&self) -> &[(u32, u32, String)] {
        &self.base_names
    }

    pub fn unique_names(&self) -> &[(u32, String)] {
        &self.unique_names
    }

    /// Affixes with the same stat property/tags/modifier (a class-specific
    /// variant of a generic affix). Hybrid affixes never count.
    pub fn same_stat(&self, a: u32, b: u32) -> bool {
        let (Some(x), Some(y)) = (self.affixes.get(&a), self.affixes.get(&b)) else { return false };
        if x.id == y.id || x.property.is_none() || y.property.is_none() {
            return false;
        }
        x.property == y.property && x.tags == y.tags && x.special_tag == y.special_tag
            && x.modifier_type == y.modifier_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_loads_and_is_keyed_by_id() {
        let data = GameData::embedded();
        assert_eq!(data.affix_name(25), "Added Health");
        assert_eq!(data.affix(45).unwrap().display_name, "Physical Resistance");
        assert_eq!(data.affix(45).unwrap().kind, AffixKind::Suffix);
        assert_eq!(data.base_name(3, 4), "Brigandine Boots");
        assert_eq!(data.sub_type(3, 4).unwrap().level, 30);
        assert_eq!(data.unique(131).unwrap().name, "Avarice");
        assert_eq!(data.type_name(22), "Relic");
    }

    #[test]
    fn tier_inference_from_values() {
        let data = GameData::embedded();
        let movement = data.affix(28).unwrap(); // Increased Movement Speed, fractional tiers
        let (lo, hi) = movement.tiers[2];
        assert_eq!(movement.tier_for_value((lo + hi) / 2.0 * 100.0, true), Some(3));
        let health = data.affix(25).unwrap(); // Added Health, flat tiers
        let (lo, _) = health.tiers[3];
        assert_eq!(health.tier_for_value(lo, false), Some(4));
        assert_eq!(health.tier_for_value(5000.0, false), Some(health.max_tier()));
        let physical = data.affix(45).unwrap(); // T8 rolls above 100 %: still a percent stat
        assert!(physical.is_fractional());
        assert_eq!(physical.tier_range(5), Some((30.0, 45.0)));
        assert_eq!(physical.tier_for_value(39.0, true), Some(5));
    }

    #[test]
    fn slots_for_types() {
        assert_eq!(Slot::for_item_type(21), &[Slot::Ring1, Slot::Ring2]);
        assert_eq!(Slot::for_item_type(16), &[Slot::Weapon]);
        assert_eq!(Slot::for_item_type(18), &[Slot::Offhand]);
        assert!(Slot::for_item_type(25).is_empty());
        assert_eq!(Slot::parse("Off-hand"), Some(Slot::Offhand));
        assert_eq!(Slot::parse("ring 2"), Some(Slot::Ring2));
    }
}
