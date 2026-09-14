//! The guide's Stats Priorities as data: ranked stat groups with weights,
//! activation conditions and saturation rules. Loaded from
//! `core/data/guide_profile.json` (embedded) or any file the user points at.
//!
//! Nothing in this module knows about specific stats; all of that lives in
//! the JSON so a different guide only needs a different profile.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::character_state::{CharacterState, Phase};
use crate::game_data::Affix;

const PROFILE_JSON: &str = include_str!("../data/guide_profile.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatGroup {
    Offense,
    Defense,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntRange {
    #[serde(default)]
    pub min: Option<u32>,
    #[serde(default)]
    pub max: Option<u32>,
}

impl IntRange {
    pub fn contains(&self, value: u32) -> bool {
        self.min.map_or(true, |m| value >= m) && self.max.map_or(true, |m| value <= m)
    }
}

/// Every field that is set must hold. A stat lists any number of these and
/// all of them must hold for the stat to be active.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Condition {
    #[serde(default)]
    pub phase: Option<Vec<Phase>>,
    #[serde(default)]
    pub level: Option<IntRange>,
    #[serde(default)]
    pub heavens_bulwark_points: Option<IntRange>,
    #[serde(default)]
    pub healing_hands_specced: Option<bool>,
    #[serde(default)]
    pub solarum_plate_equipped: Option<bool>,
    #[serde(default)]
    pub nagasa_scymitar_equipped: Option<bool>,
    /// Build-specific yes/no facts (declared in the profile's `facts`), all must match.
    /// A flag missing from the character state counts as false.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub flags: HashMap<String, bool>,
    /// Build-specific counters (skill points, etc.); a missing counter counts as 0.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub counters: HashMap<String, IntRange>,
}

impl Condition {
    pub fn holds(&self, state: &CharacterState) -> bool {
        if let Some(phases) = &self.phase {
            if !phases.contains(&state.phase()) {
                return false;
            }
        }
        if let Some(range) = &self.level {
            if !range.contains(state.level) {
                return false;
            }
        }
        if let Some(range) = &self.heavens_bulwark_points {
            if !range.contains(state.heavens_bulwark_points) {
                return false;
            }
        }
        if self.healing_hands_specced.map_or(false, |v| v != state.healing_hands_specced) {
            return false;
        }
        if self.solarum_plate_equipped.map_or(false, |v| v != state.solarum_plate_equipped) {
            return false;
        }
        if self.nagasa_scymitar_equipped.map_or(false, |v| v != state.nagasa_scymitar_equipped) {
            return false;
        }
        for (key, wanted) in &self.flags {
            if state.flag(key) != *wanted {
                return false;
            }
        }
        for (key, range) in &self.counters {
            if !range.contains(state.counter(key)) {
                return false;
            }
        }
        true
    }

    /// Human-readable form for explanations.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(phases) = &self.phase {
            parts.push(format!("phase in {}", phases.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("/")));
        }
        if let Some(r) = &self.level {
            parts.push(format!("level {}-{}", r.min.unwrap_or(1), r.max.map(|m| m.to_string()).unwrap_or_else(|| "max".into())));
        }
        if let Some(r) = &self.heavens_bulwark_points {
            parts.push(match (r.min, r.max) {
                (Some(min), None) => format!("Heaven's Bulwark >= {min} points"),
                (None, Some(max)) => format!("Heaven's Bulwark <= {max} points"),
                (Some(min), Some(max)) => format!("Heaven's Bulwark {min}-{max} points"),
                (None, None) => "Heaven's Bulwark any".into(),
            });
        }
        if let Some(v) = self.healing_hands_specced {
            parts.push(format!("Healing Hands {}", if v { "specialised" } else { "not specialised" }));
        }
        if let Some(v) = self.solarum_plate_equipped {
            parts.push(format!("Solarum Plate {}", if v { "equipped" } else { "not equipped" }));
        }
        if let Some(v) = self.nagasa_scymitar_equipped {
            parts.push(format!("Nagasa Scymitar {}", if v { "equipped" } else { "not equipped" }));
        }
        let mut flags: Vec<_> = self.flags.iter().collect();
        flags.sort();
        for (key, wanted) in flags {
            parts.push(format!("{} {}", pretty_key(key), if *wanted { "yes" } else { "no" }));
        }
        let mut counters: Vec<_> = self.counters.iter().collect();
        counters.sort_by(|a, b| a.0.cmp(b.0));
        for (key, r) in counters {
            parts.push(match (r.min, r.max) {
                (Some(min), None) => format!("{} >= {min}", pretty_key(key)),
                (None, Some(max)) => format!("{} <= {max}", pretty_key(key)),
                (Some(min), Some(max)) => format!("{} {min}-{max}", pretty_key(key)),
                (None, None) => format!("{} any", pretty_key(key)),
            });
        }
        parts.join(", ")
    }
}

fn pretty_key(key: &str) -> String {
    key.replace('_', " ")
}

/// A build-specific fact the user can set (shown in the overlay's Builds
/// window); referenced by `Condition::flags` / `Condition::counters`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub key: String,
    pub label: String,
    /// "flag" (yes/no) or "counter" (a number such as skill points)
    pub kind: String,
    #[serde(default)]
    pub default_on: bool,
    #[serde(default)]
    pub default_count: u32,
    #[serde(default)]
    pub note: String,
}

/// Level brackets that define the build's phases when they differ from the
/// Paladin guide's (1-26 early, 27-37 intermediate, 38+ final).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseLevels {
    pub intermediate_from: u32,
    pub final_from: u32,
}

impl PhaseLevels {
    pub fn phase_for(&self, level: u32) -> Phase {
        if level >= self.final_from {
            Phase::Final
        } else if level >= self.intermediate_from {
            Phase::Intermediate
        } else {
            Phase::Early
        }
    }
}

/// How a stat saturates. Resistances count only the part of the roll that
/// still fits under the cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Saturation {
    /// `element` is one of fire/cold/lightning/physical/necrotic/void/poison or "all".
    Resistance { element: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatRule {
    pub key: String,
    pub name: String,
    pub group: StatGroup,
    pub rank: u32,
    /// Full weight while the conditions hold.
    pub weight: f64,
    /// Weight while they do not ("near zero" cases); default 0.
    #[serde(default)]
    pub inactive_weight: f64,
    /// Regexes matched (case-insensitive search) against affix name and display name.
    pub patterns: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Restrict to these item type ids (e.g. spell damage only on the sword).
    #[serde(default)]
    pub only_item_types: Vec<u32>,
    /// Tier at which the stat counts fully; defaults to the profile's tier_target.
    #[serde(default)]
    pub tier_target: Option<u8>,
    #[serde(default)]
    pub saturation: Option<Saturation>,
    #[serde(default)]
    pub note: String,
    #[serde(skip)]
    compiled: Vec<regex::Regex>,
}

impl StatRule {
    fn compile(&mut self) -> anyhow::Result<()> {
        self.compiled = self
            .patterns
            .iter()
            .map(|p| regex::RegexBuilder::new(p).case_insensitive(true).build())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(())
    }

    pub fn matches(&self, affix: &Affix) -> bool {
        self.compiled.iter().any(|re| re.is_match(&affix.display_name) || re.is_match(&affix.name))
    }

    pub fn is_active(&self, state: &CharacterState) -> bool {
        self.conditions.iter().all(|c| c.holds(state))
    }

    /// Weight given the character state (full or inactive weight).
    pub fn effective_weight(&self, state: &CharacterState) -> f64 {
        if self.is_active(state) { self.weight } else { self.inactive_weight }
    }

    pub fn why_inactive(&self, state: &CharacterState) -> Option<String> {
        let failed: Vec<String> = self.conditions.iter().filter(|c| !c.holds(state)).map(|c| c.describe()).collect();
        if failed.is_empty() { None } else { Some(format!("needs {}", failed.join(" and "))) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuideProfile {
    pub name: String,
    #[serde(default)]
    pub planner_id: String,
    #[serde(default)]
    pub source: String,
    /// Resistance cap in percent (75 in Last Epoch).
    #[serde(default = "default_cap")]
    pub resistance_cap: f64,
    /// A roll of this many resistance percent counts as a full stat.
    #[serde(default = "default_res_reference")]
    pub resistance_reference_roll: f64,
    /// Tier at which a non-saturating stat counts fully.
    #[serde(default = "default_tier_target")]
    pub tier_target: u8,
    /// Build-specific facts the conditions may reference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<Fact>,
    /// Phase brackets for this build; the Paladin guide's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_levels: Option<PhaseLevels>,
    pub stats: Vec<StatRule>,
}

fn default_cap() -> f64 {
    75.0
}
fn default_res_reference() -> f64 {
    30.0
}
fn default_tier_target() -> u8 {
    5
}

/// The embedded profile's JSON text (the worked example given to the AI).
pub fn embedded_json() -> &'static str {
    PROFILE_JSON
}

impl GuideProfile {
    pub fn embedded() -> GuideProfile {
        Self::from_json(PROFILE_JSON).expect("embedded guide profile is valid")
    }

    pub fn from_json(json: &str) -> anyhow::Result<GuideProfile> {
        let mut profile: GuideProfile = serde_json::from_str(json)?;
        for stat in &mut profile.stats {
            stat.compile().map_err(|e| anyhow::anyhow!("stat {:?}: bad pattern: {e}", stat.key))?;
        }
        profile.validate()?;
        Ok(profile)
    }

    pub fn from_file(path: &std::path::Path) -> anyhow::Result<GuideProfile> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    /// All rules matching an affix, best (highest effective weight) first.
    pub fn rules_for(&self, affix: &Affix, item_type: Option<u32>, state: &CharacterState) -> Vec<&StatRule> {
        let mut rules: Vec<&StatRule> = self
            .stats
            .iter()
            .filter(|s| s.matches(affix))
            .filter(|s| s.only_item_types.is_empty() || item_type.map_or(true, |t| s.only_item_types.contains(&t)))
            .collect();
        rules.sort_by(|a, b| b.effective_weight(state).partial_cmp(&a.effective_weight(state)).unwrap());
        rules
    }

    pub fn active_stats(&self, state: &CharacterState) -> Vec<&StatRule> {
        self.stats.iter().filter(|s| s.is_active(state)).collect()
    }

    /// True when any rule's conditions use the built-in Paladin fields
    /// (Heaven's Bulwark points, Healing Hands, Solarum Plate, Nagasa Scymitar).
    pub fn uses_paladin_facts(&self) -> bool {
        self.stats.iter().flat_map(|s| s.conditions.iter()).any(|c| {
            c.heavens_bulwark_points.is_some() || c.healing_hands_specced.is_some() || c.solarum_plate_equipped.is_some() || c.nagasa_scymitar_equipped.is_some()
        })
    }

    /// Derive the phase from this profile's level brackets when the state
    /// does not pin one, and seed missing facts with their defaults.
    pub fn apply_to_state(&self, state: &mut CharacterState) {
        if state.phase.is_none() {
            if let Some(levels) = &self.phase_levels {
                state.phase = Some(levels.phase_for(state.level));
            }
        }
        for fact in &self.facts {
            match fact.kind.as_str() {
                "flag" => {
                    state.flags.entry(fact.key.clone()).or_insert(fact.default_on);
                }
                "counter" => {
                    state.counters.entry(fact.key.clone()).or_insert(fact.default_count);
                }
                _ => {}
            }
        }
    }

    /// Check a profile beyond serde: keys are unique, every referenced fact
    /// is declared, weights are sane.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut keys = std::collections::HashSet::new();
        if self.stats.is_empty() {
            anyhow::bail!("profile has no stats");
        }
        for stat in &self.stats {
            if !keys.insert(stat.key.as_str()) {
                anyhow::bail!("duplicate stat key {:?}", stat.key);
            }
            if stat.patterns.is_empty() {
                anyhow::bail!("stat {:?} has no patterns", stat.key);
            }
            if !(0.0..=5.0).contains(&stat.weight) || !(0.0..=5.0).contains(&stat.inactive_weight) {
                anyhow::bail!("stat {:?}: weights must be within 0..=5", stat.key);
            }
            for cond in &stat.conditions {
                for key in cond.flags.keys() {
                    if !self.facts.iter().any(|f| f.key == *key && f.kind == "flag") {
                        anyhow::bail!("stat {:?} references undeclared flag {:?}", stat.key, key);
                    }
                }
                for key in cond.counters.keys() {
                    if !self.facts.iter().any(|f| f.key == *key && f.kind == "counter") {
                        anyhow::bail!("stat {:?} references undeclared counter {:?}", stat.key, key);
                    }
                }
            }
        }
        for fact in &self.facts {
            if fact.kind != "flag" && fact.kind != "counter" {
                anyhow::bail!("fact {:?}: kind must be \"flag\" or \"counter\"", fact.key);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_profile_compiles_and_has_both_groups() {
        let profile = GuideProfile::embedded();
        assert!(profile.stats.iter().any(|s| s.group == StatGroup::Offense));
        assert!(profile.stats.iter().any(|s| s.group == StatGroup::Defense));
        assert!(profile.stats.iter().all(|s| !s.compiled.is_empty()));
        assert_eq!(profile.resistance_cap, 75.0);
    }

    #[test]
    fn conditions_hold_and_describe() {
        let mut state = CharacterState::default();
        state.level = 40;
        state.heavens_bulwark_points = 3;
        let until_bulwark = Condition { heavens_bulwark_points: Some(IntRange { min: None, max: Some(4) }), ..Default::default() };
        assert!(until_bulwark.holds(&state));
        state.heavens_bulwark_points = 5;
        assert!(!until_bulwark.holds(&state));
        assert!(until_bulwark.describe().contains("<= 4"));
        let final_only = Condition { phase: Some(vec![Phase::Final]), ..Default::default() };
        assert!(final_only.holds(&state)); // level 40 = final
        state.level = 20;
        assert!(!final_only.holds(&state));
    }

    #[test]
    fn generic_facts_and_phase_levels() {
        let json = r#"{
          "name": "t", "stats": [
            {"key": "a", "name": "A", "group": "offense", "rank": 1, "weight": 1.0, "patterns": ["^Health$"],
             "conditions": [{"flags": {"has_wand": true}, "counters": {"pts": {"min": 5}}}]}
          ],
          "facts": [
            {"key": "has_wand", "label": "Wand equipped", "kind": "flag"},
            {"key": "pts", "label": "Points", "kind": "counter", "default_count": 2}
          ],
          "phase_levels": {"intermediate_from": 20, "final_from": 50}
        }"#;
        let profile = GuideProfile::from_json(json).unwrap();
        let mut state = CharacterState::default();
        state.level = 30;
        profile.apply_to_state(&mut state);
        assert_eq!(state.phase(), Phase::Intermediate);
        assert_eq!(state.counter("pts"), 2);
        let rule = &profile.stats[0];
        assert!(!rule.is_active(&state));
        state.flags.insert("has_wand".into(), true);
        state.counters.insert("pts".into(), 5);
        assert!(rule.is_active(&state));
        assert!(rule.why_inactive(&state).is_none());
        let bad = json.replace("\"has_wand\": true", "\"other\": true");
        assert!(GuideProfile::from_json(&bad).is_err());
    }
}
