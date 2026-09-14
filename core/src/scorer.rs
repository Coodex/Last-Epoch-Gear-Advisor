//! Score one item against the guide profile for the current character.
//!
//! Every affix earns `weight * factor`:
//! * ordinary stats: `factor = min(tier / tier_target, 1.2)`, so a T5 roll is a
//!   full stat and exalted tiers get a small bonus;
//! * resistances: `factor = min(value, gap) / reference_roll`, where
//!   `gap = cap - (current - contribution of the item being replaced)`. A
//!   stat the character is capped on is worth nothing.
//! Inactive rules (conditions not met) use the rule's `inactive_weight`, which
//! is zero or "near zero" per the guide. Affixes not on the list score zero
//! and are reported, never dropped.

use serde::{Deserialize, Serialize};

use crate::character_state::{CharacterState, ELEMENTS};
use crate::game_data::GameData;
use crate::guide_profile::{GuideProfile, Saturation, StatGroup, StatRule};
use crate::item_parser::{ParsedAffix, ParsedItem};

pub const EXALTED_BONUS_CAP: f64 = 1.2;

pub struct ScoreContext<'a> {
    pub data: &'a GameData,
    pub profile: &'a GuideProfile,
    pub state: &'a CharacterState,
    /// Item currently equipped in the slot the scored item would take.
    pub replaced: Option<&'a ParsedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffixScore {
    pub name: String,
    pub tier: u8,
    pub value: Option<f64>,
    pub is_percent: bool,
    pub stat: Option<String>,
    pub stat_key: Option<String>,
    pub group: Option<StatGroup>,
    pub weight: f64,
    pub factor: f64,
    pub contribution: f64,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemScore {
    pub total: f64,
    pub offense: f64,
    pub defense: f64,
    pub affixes: Vec<AffixScore>,
    pub unrecognised: Vec<String>,
    pub notes: Vec<String>,
}

fn tier_factor(tier: u8, target: u8) -> f64 {
    (tier as f64 / target.max(1) as f64).min(EXALTED_BONUS_CAP)
}

fn format_value(value: f64, is_percent: bool) -> String {
    let number = if (value - value.round()).abs() < 1e-6 { format!("{}", value.round() as i64) } else { format!("{value:.1}") };
    if is_percent { format!("{number}%") } else { number }
}

/// Resistance percent an item contributes to one element (hybrid and
/// "all resistances" affixes included).
pub fn resistance_contribution(item: &ParsedItem, element: &str, ctx: &ScoreContext) -> f64 {
    let mut total = 0.0;
    for affix in &item.affixes {
        let Some(info) = ctx.data.affix(affix.affix_id) else { continue };
        for rule in ctx.profile.rules_for(info, item.item_type, ctx.state) {
            if let Some(Saturation::Resistance { element: rule_element }) = &rule.saturation {
                if rule_element == element || rule_element == "all" {
                    total += item.affix_value(affix, ctx.data);
                    break;
                }
            }
        }
    }
    total
}

/// Cap headroom for an element once the replaced item's contribution is removed.
pub fn resistance_gap(element: &str, ctx: &ScoreContext) -> f64 {
    let current = ctx.state.resistance(element);
    let replaced = ctx.replaced.map(|item| resistance_contribution(item, element, ctx)).unwrap_or(0.0);
    ctx.profile.resistance_cap - (current - replaced)
}

fn score_resistance(affix: &ParsedAffix, item: &ParsedItem, rule: &StatRule, element: &str, weight: f64, ctx: &ScoreContext) -> (f64, String) {
    let value = item.affix_value(affix, ctx.data);
    let elements: Vec<&str> = if element == "all" { ELEMENTS.to_vec() } else { vec![element] };
    let mut fractions = Vec::new();
    let mut notes = Vec::new();
    for e in &elements {
        let gap = resistance_gap(e, ctx).max(0.0);
        let usable = value.min(gap);
        fractions.push((usable / ctx.profile.resistance_reference_roll).min(1.0));
        if gap <= 0.0 {
            notes.push(format!("{e} capped"));
        } else if usable < value {
            notes.push(format!("{e}: only {usable:.0}% of {value:.0}% fits under the cap"));
        }
    }
    let factor = fractions.iter().sum::<f64>() / fractions.len() as f64;
    let mut note = notes.join("; ");
    if note.is_empty() {
        note = format!("{} {}", format_value(value, true), rule.name.to_lowercase());
    }
    (weight * factor, note)
}

pub fn score_item(item: &ParsedItem, ctx: &ScoreContext) -> ItemScore {
    let mut score = ItemScore { unrecognised: item.unmatched.clone(), ..Default::default() };
    for affix in &item.affixes {
        let Some(info) = ctx.data.affix(affix.affix_id) else {
            score.affixes.push(AffixScore {
                name: affix.name.clone(), tier: affix.tier, value: affix.value, is_percent: affix.is_percent,
                stat: None, stat_key: None, group: None, weight: 0.0, factor: 0.0, contribution: 0.0,
                note: "unknown affix id".into(),
            });
            continue;
        };
        let rules = ctx.profile.rules_for(info, item.item_type, ctx.state);
        let Some(rule) = rules.first() else {
            score.affixes.push(AffixScore {
                name: affix.name.clone(), tier: affix.tier, value: affix.value, is_percent: affix.is_percent,
                stat: None, stat_key: None, group: None, weight: 0.0, factor: 0.0, contribution: 0.0,
                note: "not in the guide's priorities".into(),
            });
            continue;
        };
        let weight = rule.effective_weight(ctx.state);
        let (contribution, factor, note) = if weight <= 0.0 {
            (0.0, 0.0, format!("inactive: {}", rule.why_inactive(ctx.state).unwrap_or_default()))
        } else {
            match &rule.saturation {
                Some(Saturation::Resistance { element }) => {
                    let (c, note) = score_resistance(affix, item, rule, element, weight, ctx);
                    let factor = if weight > 0.0 { c / weight } else { 0.0 };
                    (c, factor, note)
                }
                None => {
                    let target = rule.tier_target.unwrap_or(ctx.profile.tier_target);
                    let factor = tier_factor(affix.tier, target);
                    let mut note = format!("T{} of target T{}", affix.tier, target);
                    if !rule.is_active(ctx.state) {
                        note = format!("reduced: {}", rule.why_inactive(ctx.state).unwrap_or_default());
                    }
                    (weight * factor, factor, note)
                }
            }
        };
        match rule.group {
            StatGroup::Offense => score.offense += contribution,
            StatGroup::Defense => score.defense += contribution,
        }
        score.total += contribution;
        score.affixes.push(AffixScore {
            name: affix.name.clone(), tier: affix.tier, value: affix.value, is_percent: affix.is_percent,
            stat: Some(rule.name.clone()), stat_key: Some(rule.key.clone()), group: Some(rule.group),
            weight, factor, contribution, note,
        });
    }
    if item.is_unique() {
        score.notes.push("unique item: its mods are not scored, judge it manually".into());
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_data::AffixKind;
    use crate::item_parser::{ParsedAffix, TierSource};

    /// Build an item directly from affix ids so the tests do not depend on tooltip parsing.
    pub(crate) fn item_with(item_type: u32, sub_type: u32, affixes: &[(u32, u8, Option<f64>)]) -> ParsedItem {
        let data = GameData::embedded();
        let mut item = ParsedItem { item_type: Some(item_type), sub_type: Some(sub_type), ..Default::default() };
        item.base_name = data.base_name(item_type, sub_type);
        item.type_name = data.type_name(item_type);
        for (id, tier, value) in affixes {
            let info = data.affix(*id).expect("known affix id");
            let is_percent = info.tiers.iter().map(|t| t.1).fold(0.0, f64::max) <= 1.0;
            item.affixes.push(ParsedAffix {
                affix_id: *id, name: info.display_name.clone(), kind: info.kind, tier: *tier,
                tier_source: if value.is_some() { TierSource::Value } else { TierSource::Label },
                value: *value, is_percent, line: String::new(), match_score: 100.0,
            });
        }
        item
    }

    const FIRE_RES: u32 = 13;
    const HEALTH: u32 = 25;
    const HEALTH_REGEN: u32 = 22;
    const VITALITY: u32 = 505;

    fn state_at(level: u32) -> CharacterState {
        CharacterState { level, ..Default::default() }
    }

    #[test]
    fn capped_resistance_is_worth_nothing() {
        let data = GameData::embedded();
        let profile = GuideProfile::embedded();
        let item = item_with(3, 4, &[(VITALITY, 5, Some(8.0)), (FIRE_RES, 5, Some(30.0))]);
        let mut state = state_at(49);
        state.resistances.insert("fire".into(), 75.0);
        let capped = score_item(&item, &ScoreContext { data, profile: &profile, state: &state, replaced: None });
        let fire = capped.affixes.iter().find(|a| a.name == "Fire Resistance").unwrap();
        assert_eq!(fire.contribution, 0.0);
        assert!(fire.note.contains("capped"));

        state.resistances.insert("fire".into(), 40.0);
        let open = score_item(&item, &ScoreContext { data, profile: &profile, state: &state, replaced: None });
        let fire = open.affixes.iter().find(|a| a.name == "Fire Resistance").unwrap();
        assert!(fire.contribution > 0.0);
        assert!(open.total > capped.total);
        assert_eq!(item.affixes[1].kind, AffixKind::Suffix);
    }

    #[test]
    fn replaced_item_frees_up_gap() {
        let data = GameData::embedded();
        let profile = GuideProfile::embedded();
        let candidate = item_with(3, 4, &[(FIRE_RES, 5, Some(30.0))]);
        let equipped = item_with(3, 2, &[(FIRE_RES, 4, Some(25.0))]);
        let mut state = state_at(49);
        state.resistances.insert("fire".into(), 75.0); // capped, but 25 of it comes from the equipped boots
        let ctx = ScoreContext { data, profile: &profile, state: &state, replaced: Some(&equipped) };
        assert_eq!(resistance_gap("fire", &ctx), 25.0);
        let score = score_item(&candidate, &ctx);
        let fire = score.affixes.iter().find(|a| a.name == "Fire Resistance").unwrap();
        assert!(fire.contribution > 0.0);
        assert!(fire.note.contains("only 25%"), "{}", fire.note);
    }

    #[test]
    fn conditions_zero_out_stats() {
        let data = GameData::embedded();
        let profile = GuideProfile::embedded();
        let item = item_with(22, 40, &[(HEALTH_REGEN, 5, None), (HEALTH, 3, None)]);
        let mut state = state_at(49);
        let before = score_item(&item, &ScoreContext { data, profile: &profile, state: &state, replaced: None });
        state.flags.insert("healing_hands_specced".into(), true);
        let after = score_item(&item, &ScoreContext { data, profile: &profile, state: &state, replaced: None });
        let regen_before = before.affixes.iter().find(|a| a.stat_key.as_deref() == Some("health_regen")).unwrap().contribution;
        let regen_after = after.affixes.iter().find(|a| a.stat_key.as_deref() == Some("health_regen")).unwrap().contribution;
        assert!(regen_before > 0.5);
        assert!(regen_after < 0.1 && regen_after > 0.0); // "near zero"
        assert!(after.affixes.iter().any(|a| a.note.starts_with("reduced")));
    }

    #[test]
    fn unlisted_affixes_score_zero_and_are_reported() {
        let data = GameData::embedded();
        let profile = GuideProfile::embedded();
        let mut item = item_with(3, 4, &[(28, 5, Some(20.0))]); // Movement Speed: not on the list
        item.unmatched.push("+3 Some Weird Line".into());
        let score = score_item(&item, &ScoreContext { data, profile: &profile, state: &state_at(49), replaced: None });
        assert_eq!(score.total, 0.0);
        assert_eq!(score.affixes[0].note, "not in the guide's priorities");
        assert_eq!(score.unrecognised, vec!["+3 Some Weird Line".to_string()]);
    }
}
