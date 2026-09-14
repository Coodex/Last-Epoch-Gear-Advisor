//! Candidate item vs. what is equipped in the same slot.

use serde::{Deserialize, Serialize};

use crate::character_state::{CharacterState, EquippedProfile, ELEMENTS};
use crate::game_data::{GameData, Slot};
use crate::guide_profile::GuideProfile;
use crate::item_parser::ParsedItem;
use crate::scorer::{resistance_contribution, score_item, ItemScore, ScoreContext};

/// Score difference below which two items are called a sidegrade.
pub const SIDEGRADE_MARGIN: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerdictLabel {
    Upgrade,
    Sidegrade,
    Worse,
    Review,
    Unknown,
}

impl VerdictLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictLabel::Upgrade => "UPGRADE",
            VerdictLabel::Sidegrade => "SIDEGRADE",
            VerdictLabel::Worse => "WORSE",
            VerdictLabel::Review => "REVIEW",
            VerdictLabel::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub label: VerdictLabel,
    pub slot: Option<Slot>,
    pub candidate_name: String,
    pub candidate_score: f64,
    pub equipped_name: Option<String>,
    pub equipped_score: f64,
    pub delta: f64,
    /// Top reasons, best first ("+0.85 Health T5", "-0.55 Fire Resistance lost").
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub candidate: ItemScore,
    pub equipped: Option<ItemScore>,
}

fn fmt_delta(value: f64) -> String {
    if value >= 0.0 { format!("+{value:.2}") } else { format!("{value:.2}") }
}

/// Compare a candidate against the equipped item of its slot. Rings are
/// compared against the weaker ring. Never panics on an unrecognised item.
pub fn compare(
    candidate: &ParsedItem,
    equipped: &EquippedProfile,
    data: &GameData,
    profile: &GuideProfile,
    state: &CharacterState,
) -> Verdict {
    let name = candidate.display_name();
    let unknown = |reason: String| Verdict {
        label: VerdictLabel::Unknown, slot: None, candidate_name: name.clone(), candidate_score: 0.0,
        equipped_name: None, equipped_score: 0.0, delta: 0.0, reasons: vec![reason],
        warnings: candidate.warnings.clone(), candidate: ItemScore::default(), equipped: None,
    };
    if !candidate.recognised() {
        return unknown("item not recognised".into());
    }
    let slots = candidate.slots();
    if slots.is_empty() {
        return unknown(format!("{} is not an equipment slot", candidate.type_name));
    }

    let mut best: Option<Verdict> = None;
    for slot in slots {
        let equipped_item = equipped.get(*slot);
        let ctx = ScoreContext { data, profile, state, replaced: equipped_item };
        let candidate_score = score_item(candidate, &ctx);
        let equipped_score = equipped_item.map(|e| score_item(e, &ctx));
        let equipped_total = equipped_score.as_ref().map(|s| s.total).unwrap_or(0.0);
        let verdict = Verdict {
            label: VerdictLabel::Sidegrade,
            slot: Some(*slot),
            candidate_name: name.clone(),
            candidate_score: candidate_score.total,
            equipped_name: equipped_item.map(|e| e.display_name()),
            equipped_score: equipped_total,
            delta: candidate_score.total - equipped_total,
            reasons: Vec::new(),
            warnings: Vec::new(),
            candidate: candidate_score,
            equipped: equipped_score,
        };
        if best.as_ref().map_or(true, |b| verdict.delta > b.delta) {
            best = Some(verdict);
        }
    }
    let mut verdict = best.expect("at least one slot");
    let slot = verdict.slot.expect("slot set");
    let equipped_item = equipped.get(slot);

    // label
    verdict.label = if candidate.is_unique() {
        VerdictLabel::Review
    } else if equipped_item.is_none() {
        VerdictLabel::Upgrade
    } else if verdict.delta >= SIDEGRADE_MARGIN {
        VerdictLabel::Upgrade
    } else if verdict.delta <= -SIDEGRADE_MARGIN {
        VerdictLabel::Worse
    } else {
        VerdictLabel::Sidegrade
    };

    // reasons: biggest contributions gained and lost
    let mut diffs: Vec<(f64, String)> = Vec::new();
    for a in &verdict.candidate.affixes {
        if a.contribution > 0.0 {
            let stat = a.stat.clone().unwrap_or_else(|| a.name.clone());
            diffs.push((a.contribution, format!("{} {} T{} ({})", fmt_delta(a.contribution), stat, a.tier, a.note)));
        }
    }
    if let Some(eq) = &verdict.equipped {
        for a in &eq.affixes {
            if a.contribution > 0.0 {
                let stat = a.stat.clone().unwrap_or_else(|| a.name.clone());
                diffs.push((-a.contribution, format!("{} losing {} T{}", fmt_delta(-a.contribution), stat, a.tier)));
            }
        }
    }
    diffs.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap());
    verdict.reasons = diffs.into_iter().take(3).map(|d| d.1).collect();
    if verdict.reasons.len() < 3 {
        // listed stats that earned nothing (capped resistance, inactive rule) explain themselves
        for a in verdict.candidate.affixes.iter().filter(|a| a.stat.is_some() && a.contribution <= 0.0) {
            if verdict.reasons.len() >= 3 {
                break;
            }
            verdict.reasons.push(format!("+0.00 {} T{}: {}", a.stat.clone().unwrap_or_default(), a.tier, a.note));
        }
    }
    if verdict.reasons.is_empty() {
        verdict.reasons.push("no affix on this item is on the guide's priority list".into());
    }

    // warnings
    if equipped_item.is_none() {
        verdict.warnings.push(format!("{} slot is empty in your profile", slot.label()));
    }
    if candidate.level_requirement > state.level {
        verdict.warnings.push(format!("requires level {}, you are {}", candidate.level_requirement, state.level));
    }
    let ctx = ScoreContext { data, profile, state, replaced: equipped_item };
    for element in ELEMENTS {
        let current = state.resistance(element);
        let lost = equipped_item.map(|e| resistance_contribution(e, element, &ctx)).unwrap_or(0.0);
        let gained = resistance_contribution(candidate, element, &ctx);
        let after = current - lost + gained;
        let cap = profile.resistance_cap;
        if current >= cap && after < cap - 0.01 {
            verdict.warnings.push(format!("swapping drops {} Res below cap by {:.0}%", capitalise(element), cap - after));
        } else if after < current - 0.5 {
            verdict.warnings.push(format!("{} Res drops {:.0}% -> {:.0}%", capitalise(element), current, after));
        }
    }
    for note in &candidate.warnings {
        verdict.warnings.push(format!("parser: {note}"));
    }
    for line in &candidate.unmatched {
        verdict.warnings.push(format!("not in affix data, scored 0: {line}"));
    }
    verdict
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Every equipped slot scored against the guide, weakest first, with the
/// guide bracket's example base for reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotStanding {
    pub slot: Slot,
    pub item_name: Option<String>,
    pub score: f64,
    pub guide_base: Option<String>,
    pub top_affixes: Vec<String>,
}

pub fn weak_slots(
    equipped: &EquippedProfile,
    data: &GameData,
    profile: &GuideProfile,
    state: &CharacterState,
    plan: Option<&crate::planner::GearPlan>,
) -> Vec<SlotStanding> {
    let bracket = plan.and_then(|p| p.bracket_for_level(state.level));
    let mut standings: Vec<SlotStanding> = Slot::ALL
        .iter()
        .map(|slot| {
            let item = equipped.get(*slot);
            let ctx = ScoreContext { data, profile, state, replaced: item };
            let score = item.map(|i| score_item(i, &ctx));
            SlotStanding {
                slot: *slot,
                item_name: item.map(|i| i.display_name()),
                score: score.as_ref().map(|s| s.total).unwrap_or(0.0),
                guide_base: bracket.and_then(|b| b.items.get(slot)).map(|g| match g.unique_id {
                    Some(u) => data.unique(u).map(|u| u.name.clone()).unwrap_or_else(|| data.base_name(g.item_type, g.sub_type)),
                    None => data.base_name(g.item_type, g.sub_type),
                }),
                top_affixes: score
                    .map(|s| s.affixes.iter().filter(|a| a.contribution > 0.0).map(|a| format!("{} T{}", a.stat.clone().unwrap_or(a.name.clone()), a.tier)).collect())
                    .unwrap_or_default(),
            }
        })
        .collect();
    standings.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
    standings
}
