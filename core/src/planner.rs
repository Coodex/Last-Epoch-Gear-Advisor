//! The guide's gear plan: the Maxroll planner embedded in the guide page.
//!
//! The planner document comes from `https://planners.maxroll.gg/profiles/le/<id>`;
//! its `data` field holds `profiles` (one per level bracket, the name carries
//! the range: "Early Setup (lvl 5 - 26)") and an `items` pool of
//! `{itemType, subType, uniqueID?, affixes: [{id, tier, roll}]}` (the format
//! LEBuildConverter documents). A copy is embedded; `fetch` refreshes it.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::character_state::Phase;
use crate::game_data::Slot;

const PLANNER_JSON: &str = include_str!("../data/planner_lp4lq0i3.json");
pub const PLANNER_API: &str = "https://planners.maxroll.gg/profiles/le/";
static RANGE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:lvl|level|lv)\.?\s*(\d+)\s*(?:-|to)\s*(\d+)").unwrap());
static PLUS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:lvl|level|lv)\.?\s*(\d+)\s*\+").unwrap());
static PLANNER_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"maxroll\.gg/last-epoch/planner/([A-Za-z0-9]+)").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerAffix {
    pub id: u32,
    pub tier: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerItem {
    pub item_type: u32,
    pub sub_type: u32,
    pub unique_id: Option<u32>,
    pub affixes: Vec<PlannerAffix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bracket {
    pub name: String,
    pub level_min: u32,
    pub level_max: u32,
    pub phase: Phase,
    pub items: HashMap<Slot, PlannerItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GearPlan {
    pub planner_id: String,
    pub name: String,
    pub author: String,
    pub brackets: Vec<Bracket>,
}

impl GearPlan {
    pub fn embedded() -> GearPlan {
        Self::from_json(PLANNER_JSON).expect("embedded planner is valid")
    }

    /// Parse a planner API document (`data` may be a JSON string or object).
    pub fn from_json(json: &str) -> anyhow::Result<GearPlan> {
        let root: Value = serde_json::from_str(json)?;
        let data: Value = match &root["data"] {
            Value::String(s) => serde_json::from_str(s)?,
            other => other.clone(),
        };
        let pool = data["items"].as_object().cloned().unwrap_or_default();
        let mut brackets = Vec::new();
        let mut previous_max = 0u32;
        for profile in data["profiles"].as_array().cloned().unwrap_or_default() {
            let name = profile["name"].as_str().unwrap_or("").to_string();
            let level = profile["level"].as_u64().unwrap_or(0) as u32;
            let (level_min, level_max) = parse_range(&name, previous_max + 1, level.max(previous_max + 1));
            let mut items = HashMap::new();
            if let Some(map) = profile["items"].as_object() {
                for (key, reference) in map {
                    let Some(slot) = Slot::ALL.iter().copied().find(|s| s.planner_key() == key) else { continue };
                    let reference = match reference {
                        Value::Number(n) => n.to_string(),
                        Value::String(s) => s.clone(),
                        _ => continue,
                    };
                    let Some(entry) = pool.get(&reference) else { continue };
                    let Some(item_type) = entry["itemType"].as_u64() else { continue };
                    items.insert(slot, PlannerItem {
                        item_type: item_type as u32,
                        sub_type: entry["subType"].as_u64().unwrap_or(0) as u32,
                        unique_id: entry["uniqueID"].as_u64().map(|u| u as u32),
                        affixes: entry["affixes"].as_array().cloned().unwrap_or_default().iter().filter_map(|a| {
                            Some(PlannerAffix { id: a["id"].as_u64()? as u32, tier: a["tier"].as_u64().unwrap_or(1) as u8 })
                        }).collect(),
                    });
                }
            }
            let phase = phase_for_name(&name).unwrap_or_else(|| Phase::from_level(level_max.min(level_min.max(level))));
            brackets.push(Bracket { name, level_min, level_max, phase, items });
            previous_max = level_max;
        }
        brackets.sort_by_key(|b| (b.level_min, b.level_max));
        Ok(GearPlan {
            planner_id: root["id"].as_str().unwrap_or("").to_string(),
            name: root["name"].as_str().unwrap_or("").to_string(),
            author: root["user"]["username"].as_str().unwrap_or("").to_string(),
            brackets,
        })
    }

    /// Highest bracket the level has entered (levels in a gap map downwards).
    pub fn bracket_for_level(&self, level: u32) -> Option<&Bracket> {
        let mut chosen = self.brackets.first()?;
        for bracket in &self.brackets {
            if bracket.level_min <= level {
                chosen = bracket;
            }
        }
        Some(chosen)
    }

    /// Bracket for a phase (the last one carrying that phase).
    pub fn bracket_for_phase(&self, phase: Phase) -> Option<&Bracket> {
        self.brackets.iter().filter(|b| b.phase == phase).last()
    }

    /// Planner id from a guide page's HTML (most frequent embed wins).
    pub fn planner_id_from_html(html: &str) -> Option<String> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for caps in PLANNER_ID_RE.captures_iter(html) {
            *counts.entry(caps.get(1).unwrap().as_str()).or_default() += 1;
        }
        counts.into_iter().max_by_key(|(_, n)| *n).map(|(id, _)| id.to_string())
    }

    /// Download a planner document (returns the raw JSON and the parsed plan).
    pub fn fetch(planner_id: &str) -> anyhow::Result<(String, GearPlan)> {
        let url = format!("{PLANNER_API}{planner_id}");
        let body = ureq::get(&url).set("User-Agent", "LE-Gear-Advisor/0.1").call()?.into_string()?;
        let plan = Self::from_json(&body)?;
        Ok((body, plan))
    }

    /// Download a guide page and resolve its planner id.
    pub fn fetch_planner_id(guide_url: &str) -> anyhow::Result<String> {
        let html = ureq::get(guide_url)
            .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) LE-Gear-Advisor/0.1")
            .call()?
            .into_string()?;
        Self::planner_id_from_html(&html).ok_or_else(|| anyhow::anyhow!("no Maxroll planner embed found on {guide_url}"))
    }
}

fn parse_range(name: &str, fallback_min: u32, fallback_max: u32) -> (u32, u32) {
    if let Some(c) = RANGE_RE.captures(name) {
        return (c[1].parse().unwrap_or(fallback_min), c[2].parse().unwrap_or(fallback_max));
    }
    if let Some(c) = PLUS_RE.captures(name) {
        return (c[1].parse().unwrap_or(fallback_min), 100);
    }
    (fallback_min, fallback_max)
}

fn phase_for_name(name: &str) -> Option<Phase> {
    let lower = name.to_lowercase();
    if lower.contains("final") || lower.contains("endgame") {
        Some(Phase::Final)
    } else if lower.contains("intermediate") {
        Some(Phase::Intermediate)
    } else if lower.contains("early") || lower.contains("start") {
        Some(Phase::Early)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_plan_has_four_brackets() {
        let plan = GearPlan::embedded();
        assert_eq!(plan.planner_id, "lp4lq0i3");
        let ranges: Vec<(u32, u32)> = plan.brackets.iter().map(|b| (b.level_min, b.level_max)).collect();
        assert_eq!(ranges, vec![(1, 4), (5, 26), (27, 36), (38, 70)]);
        assert_eq!(plan.bracket_for_level(37).unwrap().phase, Phase::Intermediate);
        assert_eq!(plan.bracket_for_level(99).unwrap().phase, Phase::Final);
        let final_bracket = plan.bracket_for_phase(Phase::Final).unwrap();
        assert!(final_bracket.items.contains_key(&Slot::Offhand));
        assert_eq!(final_bracket.items[&Slot::Weapon].item_type, 9); // one-handed sword
    }

    #[test]
    fn planner_id_from_html_prefers_the_most_frequent() {
        let html = "https://maxroll.gg/last-epoch/planner/lp4lq0i3#1 https://maxroll.gg/last-epoch/planner/lp4lq0i3#2 https://maxroll.gg/last-epoch/planner/other001#1";
        assert_eq!(GearPlan::planner_id_from_html(html).as_deref(), Some("lp4lq0i3"));
        assert!(GearPlan::planner_id_from_html("nothing").is_none());
    }
}
