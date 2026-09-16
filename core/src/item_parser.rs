//! Turn tooltip text (pasted or OCR'd) into a [`ParsedItem`].
//!
//! Real Last Epoch tooltip layout, verified on screen captures:
//!
//! ```text
//! ASSASSIN'S INSCRIBED            title: affix titles + base name, may wrap
//! TABLET OF REGROWTH
//! SENTINEL RELIC                  type line: [class] item type
//! +25 Mana                        implicits, one per base implicit
//! -3 Melee Attack Mana Cost
//! CORRUPTED - UNMODIFIABLE        optional status line
//! +23% Increased Poison Damage    affixes (tier label "T3" only when shown)
//! +18 Health
//! Requires: Level 39 . Sentinel   end of the item block
//! ...                             compare-with-equipped section: ignored
//! ```
//!
//! Tiers come from a `T<n>` label when present, otherwise from the rolled
//! value and the affix's per-tier roll ranges. Unrecognised affix lines are
//! listed in `unmatched` and never abort parsing.

use serde::{Deserialize, Serialize};

use crate::game_data::{AffixKind, GameData, Slot};
use crate::text::{extract_tier, extract_value, normalize, partial_ratio, ratio, squash, strip_stop_words, token_sort_ratio};

const NOISE_PREFIXES: &[&str] = &[
    "requires", "forging potential", "sell", "value", "item level", "weaver", "legendary potential",
    "right click", "left click", "shift", "ctrl", "alt", "durability", "sockets", "bound", "hold", "press",
    "compare", "equip", "unequip", "level ", "lvl ", "corrupted", "sealed", "unmodifiable", "explanations",
    "mod ranges", "drop item", "crafting materials", "items", "resources", "transfer", "sort",
];
const CLASS_WORDS: &[&str] = &["primalist", "mage", "sentinel", "acolyte", "rogue"];
pub const MIN_BASE_SCORE: f64 = 80.0;
pub const MIN_AFFIX_SCORE: f64 = 72.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierSource {
    Label,
    Value,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedAffix {
    pub affix_id: u32,
    pub name: String,
    pub kind: AffixKind,
    pub tier: u8,
    pub tier_source: TierSource,
    pub value: Option<f64>,
    pub is_percent: bool,
    pub line: String,
    pub match_score: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedItem {
    pub title: String,
    pub type_line: String,
    pub item_type: Option<u32>,
    pub sub_type: Option<u32>,
    pub base_name: String,
    pub type_name: String,
    pub unique_id: Option<u32>,
    pub unique_name: Option<String>,
    pub level_requirement: u32,
    pub implicit_lines: Vec<String>,
    pub affixes: Vec<ParsedAffix>,
    pub unmatched: Vec<String>,
    pub warnings: Vec<String>,
}

impl ParsedItem {
    pub fn recognised(&self) -> bool {
        self.item_type.is_some() && self.sub_type.is_some()
    }

    pub fn is_unique(&self) -> bool {
        self.unique_id.is_some()
    }

    pub fn display_name(&self) -> String {
        if let Some(name) = &self.unique_name {
            return name.clone();
        }
        if !self.base_name.is_empty() {
            return self.base_name.clone();
        }
        self.title.clone()
    }

    pub fn rarity(&self) -> &'static str {
        if self.is_unique() {
            return "Unique";
        }
        if self.affixes.iter().any(|a| a.tier >= 6) {
            return "Exalted";
        }
        match self.affixes.len() {
            0 => "Normal",
            1 | 2 => "Magic",
            _ => "Rare",
        }
    }

    pub fn slots(&self) -> &'static [Slot] {
        self.item_type.map(Slot::for_item_type).unwrap_or(&[])
    }

    /// The affix's rolled value (percent stats as percent) when known, else
    /// the middle of the tier's roll range.
    pub fn affix_value(&self, affix: &ParsedAffix, data: &GameData) -> f64 {
        if let Some(value) = affix.value {
            return value;
        }
        data.affix(affix.affix_id)
            .and_then(|a| a.tier_range(affix.tier))
            .map(|(lo, hi)| (lo + hi) / 2.0)
            .unwrap_or(0.0)
    }
}

/// Phrases that mark a non-affix line wherever they appear (icons in front of
/// "Forging Potential" or "Requires" are OCR'd as stray characters).
const NOISE_ANYWHERE: &[&str] = &[
    "forging potential", "legendary potential", "weaver", "mod explanations", "mod ranges",
    "compare items", "drop item", "equip item", "crafting materials", "unmodifiable", "corrupted",
    "base attack rate", "attack rate", "cast rate", "damage per second",
];
/// Weapon stat lines printed above the implicits ("Range 1.7m", "Base Attack Rate 1.02 - Average").
const NOISE_EXACT_PREFIXES: &[&str] = &["range", "attack speed", "average", "slow", "fast"];

pub fn is_noise(line: &str) -> bool {
    let norm = normalize(line);
    if norm.len() < 3 {
        return true;
    }
    NOISE_PREFIXES.iter().any(|p| norm.starts_with(p.trim()))
        || NOISE_ANYWHERE.iter().any(|p| norm.contains(p))
        || NOISE_EXACT_PREFIXES.iter().any(|p| norm == *p || norm.starts_with(&format!("{p} ")))
}

/// "+1 to Smite" is how the game prints "Level of Smite" affixes.
fn skill_level_query(line: &str) -> Option<String> {
    let norm = normalize(line);
    let rest = norm.strip_prefix("to ")?;
    if rest.len() < 3 || rest.contains("level") {
        return None;
    }
    Some(format!("level of {rest}"))
}

pub fn is_requirement(line: &str) -> bool {
    let norm = normalize(line);
    norm.split(' ').any(|w| w.len() >= 6 && ratio(w, "requires") >= 75.0)
        && (norm.contains("level") || line.chars().any(|c| c.is_ascii_digit()))
}

/// Footer lines of the tooltip (key hints); anything after them is not the item.
pub fn is_footer(line: &str) -> bool {
    let norm = normalize(line);
    ["mod explanations", "compare items", "mod ranges", "drop item", "equip item"].iter().any(|p| norm.contains(p))
}

fn requirement_level(line: &str) -> Option<u32> {
    let digits: String = line.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Item type from a "[CLASS] TYPE" line; whole-line match only.
pub fn match_type_line(line: &str, data: &GameData) -> Option<(u32, f64)> {
    let mut norm = normalize(line);
    for word in CLASS_WORDS {
        norm = norm.replace(word, "").trim().to_string();
    }
    if norm.is_empty() || norm.len() > 24 {
        return None;
    }
    let squashed = norm.replace(' ', "");
    let threshold = if squashed.len() <= 6 { 74.0 } else { 85.0 };
    let mut best: Option<(u32, f64)> = None;
    for (name, type_id) in data.type_names() {
        let score = ratio(&squashed, &name.replace(' ', ""));
        if score >= threshold && best.map_or(true, |b| score > b.1) {
            best = Some((*type_id, score));
        }
    }
    best
}

/// Longest base name (of `item_type` when known) found inside the title.
pub fn match_base(title: &str, item_type: Option<u32>, data: &GameData) -> Option<(u32, u32, f64)> {
    let haystack = squash(title);
    if haystack.len() < 3 {
        return None;
    }
    let mut best: Option<(u32, u32, f64, usize)> = None;
    for (type_id, sub_type, name) in data.base_names() {
        if item_type.map_or(false, |t| t != *type_id) || name.len() < 3 {
            continue;
        }
        let score = if name.len() <= haystack.len() { partial_ratio(name, &haystack) } else { ratio(name, &haystack) };
        if score < MIN_BASE_SCORE {
            continue;
        }
        if best.map_or(true, |b| (score, name.len()) > (b.2, b.3)) {
            best = Some((*type_id, *sub_type, score, name.len()));
        }
    }
    best.map(|b| (b.0, b.1, b.2))
}

/// Base whose whole name matches the whole line (used when no type line exists).
pub fn match_base_exact(line: &str, data: &GameData) -> Option<(u32, u32, f64)> {
    let squashed = squash(line);
    if squashed.len() < 4 {
        return None;
    }
    let mut best: Option<(u32, u32, f64)> = None;
    for (type_id, sub_type, name) in data.base_names() {
        let score = ratio(name, &squashed);
        if score >= 88.0 && best.map_or(true, |b| score > b.2) {
            best = Some((*type_id, *sub_type, score));
        }
    }
    best
}

/// Long base name (7+ letters) found inside a title, any item type; used only
/// when the type line is missing. Strict score so affix text never matches.
pub fn match_base_long(title: &str, data: &GameData) -> Option<(u32, u32, f64)> {
    let haystack = squash(title);
    if haystack.len() < 7 {
        return None;
    }
    let mut best: Option<(u32, u32, f64, usize)> = None;
    for (type_id, sub_type, name) in data.base_names() {
        if name.len() < 7 || name.len() > haystack.len() {
            continue;
        }
        let score = partial_ratio(name, &haystack);
        if score < 92.0 {
            continue;
        }
        if best.map_or(true, |b| (score, name.len()) > (b.2, b.3)) {
            best = Some((*type_id, *sub_type, score, name.len()));
        }
    }
    best.map(|b| (b.0, b.1, b.2))
}

/// Unique whose name appears in the title (uniques show name + base).
pub fn match_unique(title: &str, item_type: Option<u32>, data: &GameData) -> Option<(u32, f64)> {
    let haystack = squash(title);
    if haystack.len() < 4 {
        return None;
    }
    let mut best: Option<(u32, f64, usize)> = None;
    for (unique_id, name) in data.unique_names() {
        if name.len() < 5 {
            continue;
        }
        if let Some(t) = item_type {
            if data.unique(*unique_id).map_or(true, |u| u.base_type != t) {
                continue;
            }
        }
        let score = if name.len() <= haystack.len() { partial_ratio(name, &haystack) } else { ratio(name, &haystack) };
        if score < 90.0 {
            continue;
        }
        if best.map_or(true, |b| (score, name.len()) > (b.1, b.2)) {
            best = Some((*unique_id, score, name.len()));
        }
    }
    best.map(|b| (b.0, b.1))
}

/// Best affix for a tooltip line. Restricted to what can roll on `item_type`
/// first; if nothing matches (the roll tables have gaps) any gear affix is
/// accepted at a stricter score. "+1 to Smite" is expanded to "level of smite".
pub fn match_affix(line: &str, item_type: Option<u32>, data: &GameData) -> Option<(u32, f64)> {
    let expanded = skill_level_query(line);
    let text = expanded.as_deref().unwrap_or(line);
    match_affix_inner(text, item_type, data, MIN_AFFIX_SCORE)
        .or_else(|| match_affix_inner(text, None, data, MIN_AFFIX_SCORE + 13.0))
}

fn match_affix_inner(line: &str, item_type: Option<u32>, data: &GameData, min_score: f64) -> Option<(u32, f64)> {
    let norm = normalize(line);
    let query = strip_stop_words(&norm);
    let query_squashed = query.replace(' ', "");
    if query_squashed.len() < 3 {
        return None;
    }
    let wants_increased = line.contains('%') || norm.replace(' ', "").contains("creased");
    let mut best: Option<(u32, f64)> = None;
    for affix in data.affixes() {
        let Some((spaced, squashed)) = data.affix_match_text(affix.id) else { continue };
        if let Some(t) = item_type {
            if !affix.can_roll_on_type(t) {
                continue;
            }
        }
        let mut score = ratio(&query_squashed, squashed).max(token_sort_ratio(&query, spaced));
        if score < min_score {
            continue;
        }
        // "+18% Increased X" vs "+18 X": steer between percent and flat variants
        match affix.modifier_type {
            Some(1) => score += if wants_increased { 4.0 } else { -6.0 },
            Some(0) => score += if wants_increased { -6.0 } else { 4.0 },
            _ => {}
        }
        if best.map_or(true, |b| score > b.1) {
            best = Some((affix.id, score));
        }
    }
    best.map(|(id, s)| (id, s.min(100.0)))
}

/// Parse tooltip lines (top to bottom) into an item. Never panics on garbage.
/// Parse a tooltip. When the lines are a whole screen region (a tooltip drawn
/// over the inventory grid gets merged with the grid's labels), the item is
/// located by its type line ("AMULET", "BOOTS", ...) and parsed from there.
pub fn parse_tooltip(lines: &[String], data: &GameData) -> ParsedItem {
    let first = parse_once(lines, data);
    if first.recognised() {
        return first;
    }
    let cleaned: Vec<String> = lines.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if cleaned.len() <= 8 {
        return first;
    }
    for (index, line) in cleaned.iter().enumerate().skip(1) {
        if match_type_line(line, data).is_none() {
            continue;
        }
        // the title: up to two lines above the type line without digits
        let mut start = index;
        while start > 0 && index - start < 2 {
            let above = &cleaned[start - 1];
            if above.chars().any(|c| c.is_ascii_digit()) || is_noise(above) {
                break;
            }
            start -= 1;
        }
        if start == index {
            continue; // a bare type word with no title above it (grid label)
        }
        let candidate = parse_once(&cleaned[start..], data);
        if candidate.recognised() {
            return candidate;
        }
    }
    first
}

fn parse_once(lines: &[String], data: &GameData) -> ParsedItem {
    let mut item = ParsedItem::default();
    let cleaned: Vec<String> = lines.iter().map(|l| l.trim().replace('@', "O")).filter(|l| !l.is_empty()).collect();
    if cleaned.is_empty() {
        item.warnings.push("no text".into());
        return item;
    }

    // 1. type line -> item type; title = lines above it
    let mut type_index: Option<usize> = None;
    for (index, line) in cleaned.iter().take(8).enumerate() {
        if let Some((type_id, _)) = match_type_line(line, data) {
            item.item_type = Some(type_id);
            item.type_line = line.clone();
            type_index = Some(index);
            break;
        }
    }
    let body: Vec<String> = match type_index {
        Some(i) => {
            item.title = cleaned[..i].join(" ");
            cleaned[i + 1..].to_vec()
        }
        None => {
            // no type line: the title is the leading lines without digits (at most 3)
            let title_len = cleaned.iter().take(3).take_while(|l| !l.chars().any(|c| c.is_ascii_digit())).count().max(1);
            item.title = cleaned[..title_len].join(" ");
            cleaned[title_len..].to_vec()
        }
    };

    // 2. identity: unique first, then base inside the title
    let unique = match_unique(&item.title, item.item_type, data);
    let base = if type_index.is_some() {
        match_base(&item.title, item.item_type, data).or_else(|| {
            // "UNIQUE SPIDERSILK SASH" / "LEGENDARY SPLIT GREATSWORD" under the type line
            // names the base even when the title itself was misread
            body.first().and_then(|l| {
                let upper = l.trim().to_uppercase();
                ["UNIQUE ", "LEGENDARY ", "SET "].iter().find_map(|p| upper.strip_prefix(p).map(str::to_string))
            }).and_then(|rest| match_base(&rest, item.item_type, data))
        })
    } else {
        // No type line (OCR dropped it): the title is the lines before the first
        // numeric line. Accept a whole-line base match, or a long base name found
        // inside the title (short names inside affix lines are never accepted).
        let title_lines: Vec<&String> = cleaned.iter().take(3).take_while(|l| !l.chars().any(|c| c.is_ascii_digit())).collect();
        let exact = title_lines.iter().filter_map(|l| match_base_exact(l, data))
            .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
        exact.or_else(|| {
            let title = title_lines.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(" ");
            match_base_long(&title, data)
        })
    };
    match (unique, base) {
        (Some((unique_id, _)), base) => {
            let info = data.unique(unique_id).expect("unique id from index");
            item.unique_id = Some(unique_id);
            item.unique_name = Some(info.name.clone());
            match base {
                Some((t, s, _)) if t == info.base_type => {
                    item.item_type = Some(t);
                    item.sub_type = Some(s);
                }
                _ => {
                    item.item_type = Some(info.base_type);
                    item.sub_type = Some(info.sub_types.first().copied().unwrap_or(0));
                }
            }
        }
        (None, Some((t, s, _))) => {
            item.item_type = Some(t);
            item.sub_type = Some(s);
        }
        (None, None) => {
            item.warnings.push("item base not recognised".into());
            return item;
        }
    }
    let (type_id, sub_type) = (item.item_type.unwrap(), item.sub_type.unwrap());
    item.base_name = data.base_name(type_id, sub_type);
    item.type_name = data.type_name(type_id);

    // 3. body: implicits, affixes, until the requirement line
    let mut content: Vec<String> = Vec::new();
    for line in &body {
        if is_requirement(line) {
            if let Some(level) = requirement_level(line) {
                item.level_requirement = level;
            }
            break;
        }
        if is_footer(line) {
            break;
        }
        if is_noise(line) {
            continue;
        }
        content.push(line.clone());
    }
    // The base fixes the implicit count even when the type line was misread.
    let implicit_count = data.sub_type(type_id, sub_type).map(|s| s.implicits.len()).unwrap_or(0);
    if type_index.is_none() {
        item.warnings.push("item type line not read; base taken from the title".into());
    }
    let split = implicit_count.min(content.len());
    item.implicit_lines = content[..split].to_vec();
    if item.is_unique() {
        return item; // unique mods are not affixes
    }

    let implicit_keys: Vec<String> = item.implicit_lines.iter().map(|l| strip_stop_words(&normalize(l))).collect();
    let affix_lines: Vec<&String> = content[split..].iter().collect();
    let mut skip_next = false;
    for (index, line) in affix_lines.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let (label_tier, text) = extract_tier(line);
        let value = extract_value(&text);
        if let Some((v, _)) = value {
            if v < 0.0 {
                if item.affixes.is_empty() {
                    continue; // a negative line before any affix is an implicit ("-3 Spell Mana Cost")
                }
                break; // affixes never show negative values: compare section reached
            }
        }
        let key = strip_stop_words(&normalize(&text));
        // Hybrid affixes print one stat per line: the second line belongs to the
        // affix matched from the first ("+21 Ward per Second" / "+124 Ward Decay Threshold").
        if let Some(last) = item.affixes.last() {
            let last_norm = normalize(&last.name);
            if last_norm.contains(" and ") && key.len() >= 4 && strip_stop_words(&last_norm).contains(&key) {
                continue;
            }
        }
        let Some((affix_id, score)) = match_affix(&text, Some(type_id), data) else {
            // An unmatched line that repeats an implicit ("+1 Potion Slots") is the
            // compare block listing differences; a real affix repeating an implicit
            // stat (Health implicit + Health affix) still matches and is kept.
            if key.len() >= 4 && implicit_keys.iter().any(|k| k.len() >= 4 && token_sort_ratio(k, &key) >= 85.0) {
                break;
            }
            // A long affix name wrapped over two lines ("12% increased Damage" /
            // "while Channelling"): the joined text matches as one affix.
            if let Some(next) = affix_lines.get(index + 1) {
                let (_, next_text) = extract_tier(next);
                if !is_requirement(next) && !is_footer(next) && extract_value(&next_text).is_none() {
                    let joined = format!("{text} {next_text}");
                    if let Some((joined_id, joined_score)) = match_affix(&joined, Some(type_id), data) {
                        if joined_score >= 78.0 && !item.affixes.iter().any(|a| a.affix_id == joined_id) {
                            let affix = data.affix(joined_id).expect("affix id from index");
                            let (tier, source) = match label_tier {
                                Some(t) => (t, TierSource::Label),
                                None => match value.and_then(|(v, pct)| affix.tier_for_value(v, pct)) {
                                    Some(t) => (t, TierSource::Value),
                                    None => (1, TierSource::Unknown),
                                },
                            };
                            item.affixes.push(ParsedAffix {
                                affix_id: joined_id, name: affix.display_name.clone(), kind: affix.kind, tier,
                                tier_source: source, value: value.map(|(v, _)| v),
                                is_percent: value.map(|(_, p)| p).unwrap_or(false), line: joined, match_score: joined_score,
                            });
                            skip_next = true;
                            continue;
                        }
                    }
                }
                let next_unmatched = match_affix(&next_text, Some(type_id), data).is_none()
                    && !is_requirement(next) && !is_footer(next)
                    && extract_value(&next_text).map_or(true, |(v, _)| v >= 0.0);
                if next_unmatched {
                    item.unmatched.push(format!("{} / {}", line, next));
                    skip_next = true;
                    continue;
                }
            }
            item.unmatched.push(line.to_string());
            continue;
        };
        // A hybrid affix prints one stat per line and its first line alone often
        // matches a plain single-stat affix ("+21 Ward per Second"). If the text of
        // this line joined with the next matches a different affix whose name also
        // covers the next line, that hybrid is the real affix.
        let mut affix_id = affix_id;
        let mut score = score;
        if let Some(next) = affix_lines.get(index + 1) {
            let (_, next_text) = extract_tier(next);
            let next_key = strip_stop_words(&normalize(&next_text));
            if next_key.len() >= 4 && !is_requirement(next) && !is_footer(next) {
                let joined = format!("{text} {next_text}");
                if let Some((hybrid_id, hybrid_score)) = match_affix(&joined, Some(type_id), data) {
                    if hybrid_id != affix_id && hybrid_score >= 80.0 {
                        let hybrid_name = strip_stop_words(&normalize(&data.affix(hybrid_id).map(|a| a.display_name.clone()).unwrap_or_default()));
                        if hybrid_name.contains(&next_key) && hybrid_name.contains(&key) {
                            affix_id = hybrid_id;
                            score = hybrid_score;
                            skip_next = true;
                        }
                    }
                }
            }
        }
        if item.affixes.iter().any(|a| a.affix_id == affix_id) {
            break; // the compare-with-equipped block repeats the item's affixes
        }
        let affix = data.affix(affix_id).expect("affix id from index");
        let lower = affix.display_name.to_lowercase();
        if item.affixes.iter().any(|a| a.affix_id != affix_id && a.name.to_lowercase().contains(&lower)) {
            continue; // second line of a hybrid affix already counted ("... and Added Dodge Rating")
        }
        let (tier, source) = match label_tier {
            Some(t) => (t, TierSource::Label),
            None => match value.and_then(|(v, pct)| affix.tier_for_value(v, pct)) {
                Some(t) => (t, TierSource::Value),
                None => (1, TierSource::Unknown),
            },
        };
        item.affixes.push(ParsedAffix {
            affix_id,
            name: affix.display_name.clone(),
            kind: affix.kind,
            tier,
            tier_source: source,
            value: value.map(|(v, _)| v),
            is_percent: value.map(|(_, p)| p).unwrap_or(false),
            line: line.to_string(),
            match_score: score,
        });
    }
    if item.affixes.iter().any(|a| a.tier_source == TierSource::Unknown) {
        item.warnings.push("some tiers could not be read (value missing); assumed T1".into());
    }
    if item.affixes.is_empty() {
        item.warnings.push("no affixes recognised".into());
    }
    item
}

/// Convenience for pasted text: one tooltip line per text line.
pub fn parse_tooltip_text(text: &str, data: &GameData) -> ParsedItem {
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    parse_tooltip(&lines, data)
}
