//! Text normalisation and fuzzy matching for noisy OCR output.
//!
//! Real Last Epoch tooltips come back from OCR in UPPER CASE, frequently with
//! spaces dropped ("CREASEDHEALTHREGENERATION") and with `0` read as `O`
//! ("+3O%"). Everything here is deterministic and dependency-light.

use once_cell::sync::Lazy;
use regex::Regex;

static NUMBER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[-+]?\d[\d,.]*\s*%?").unwrap());
static NON_ALPHA_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z ]").unwrap());
static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static OCR_ZERO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:(?P<pre>[\d+\-])[Oo])|(?:[Oo](?P<post>[\d%]))").unwrap());
static VALUE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([-+]?)\s*(\d+(?:[.,]\d+)?)\s*(%?)").unwrap());
static TIER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[^A-Za-z])T(?:ier)?\s*([1-8])(?:[^0-9]|$)").unwrap());

/// Words that carry no information once numbers are removed.
const STOP_WORDS: &[&str] = &[
    "added", "increased", "to", "of", "the", "and", "with", "per", "on", "in", "a", "more", "less",
];

/// Lower-case, drop numbers/percentages/punctuation, collapse whitespace.
pub fn normalize(text: &str) -> String {
    let lowered = text.to_lowercase();
    let no_numbers = NUMBER_RE.replace_all(&lowered, " ");
    let alpha = NON_ALPHA_RE.replace_all(&no_numbers, " ");
    WS_RE.replace_all(&alpha, " ").trim().to_string()
}

/// [`normalize`] without spaces: what OCR usually produces.
pub fn squash(text: &str) -> String {
    normalize(text).replace(' ', "")
}

pub fn strip_stop_words(norm: &str) -> String {
    norm.split(' ')
        .filter(|w| !w.is_empty() && !STOP_WORDS.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

static PERCENT_VARIANT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(\d)\s*(?:0/0|o/o|°/o|º/o|%o|0/o|o/0)").unwrap());

/// OCR breaks the percent sign into "0/0" or "o/o" ("230/0" = "23%").
pub fn fix_ocr_percent(line: &str) -> String {
    PERCENT_VARIANT_RE.replace_all(line, "$1%").to_string()
}

/// OCR reads 0 as O next to digits: "+3O%" -> "+30%", "+1OO" -> "+100".
pub fn fix_ocr_digits(line: &str) -> String {
    let mut current = fix_ocr_percent(line);
    for _ in 0..6 {
        let next = OCR_ZERO_RE
            .replace_all(&current, |caps: &regex::Captures| {
                if let Some(pre) = caps.name("pre") {
                    format!("{}0", pre.as_str())
                } else {
                    format!("0{}", &caps["post"])
                }
            })
            .to_string();
        if next == current {
            break;
        }
        current = next;
    }
    current
}

/// First number on the line as (value, is_percent).
pub fn extract_value(line: &str) -> Option<(f64, bool)> {
    let fixed = fix_ocr_digits(line);
    let caps = VALUE_RE.captures(&fixed)?;
    let mut value: f64 = caps[2].replace(',', ".").parse().ok()?;
    if &caps[1] == "-" {
        value = -value;
    }
    Some((value, &caps[3] == "%"))
}

/// Tier label such as "T3" or "Tier 3"; returns (tier, line without the label).
pub fn extract_tier(line: &str) -> (Option<u8>, String) {
    if let Some(caps) = TIER_RE.captures(line) {
        let tier: u8 = caps[1].parse().unwrap_or(1);
        let whole = caps.get(0).unwrap();
        // keep the non-letter characters around the label
        let mut rest = String::new();
        rest.push_str(&line[..whole.start()]);
        rest.push(' ');
        rest.push_str(&line[whole.end()..]);
        let cleaned = WS_RE.replace_all(rest.trim(), " ").to_string();
        return (Some(tier), cleaned);
    }
    (None, line.to_string())
}

/// Similarity 0..100 based on normalised Levenshtein distance.
pub fn ratio(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 100.0;
    }
    strsim::normalized_levenshtein(a, b) * 100.0
}

/// Best [`ratio`] of `needle` against every window of `haystack` of the
/// needle's length (rapidfuzz's partial_ratio, simplified).
pub fn partial_ratio(needle: &str, haystack: &str) -> f64 {
    let n: Vec<char> = needle.chars().collect();
    let h: Vec<char> = haystack.chars().collect();
    if n.is_empty() || h.is_empty() {
        return 0.0;
    }
    if n.len() >= h.len() {
        return ratio(needle, haystack);
    }
    let mut best = 0.0f64;
    for start in 0..=(h.len() - n.len()) {
        let window: String = h[start..start + n.len()].iter().collect();
        let score = ratio(needle, &window);
        if score > best {
            best = score;
            if best >= 100.0 {
                break;
            }
        }
    }
    best
}

/// Token-order-insensitive similarity: sort words on both sides first.
pub fn token_sort_ratio(a: &str, b: &str) -> f64 {
    let mut wa: Vec<&str> = a.split_whitespace().collect();
    let mut wb: Vec<&str> = b.split_whitespace().collect();
    wa.sort_unstable();
    wb.sort_unstable();
    ratio(&wa.join(" "), &wb.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_and_squashes() {
        assert_eq!(normalize("+18% Increased Movement Speed"), "increased movement speed");
        assert_eq!(squash("CREASED CRITICALSTRIKE CHANCE"), "creasedcriticalstrikechance");
        assert_eq!(normalize("Requires: Level 39 · Sentinel"), "requires level sentinel");
    }

    #[test]
    fn fixes_ocr_percent_variants() {
        assert_eq!(fix_ocr_percent("230/0"), "23%");
        assert_eq!(fix_ocr_percent("+13 o/o Chance"), "+13% Chance");
        assert_eq!(extract_value("230/0"), Some((23.0, true)));
    }

    #[test]
    fn fixes_ocr_zeroes() {
        assert_eq!(fix_ocr_digits("+3O%FIRE RESISTANCE"), "+30%FIRE RESISTANCE");
        assert_eq!(fix_ocr_digits("+1OO BLOCK"), "+100 BLOCK");
        assert_eq!(fix_ocr_digits("OF ENDURANCE"), "OF ENDURANCE");
    }

    #[test]
    fn extracts_values_and_tiers() {
        assert_eq!(extract_value("+18% Increased Movement Speed"), Some((18.0, true)));
        assert_eq!(extract_value("-3 Melee Attack Mana Cost"), Some((-3.0, false)));
        assert_eq!(extract_value("Increased Poison Damage"), None);
        assert_eq!(extract_tier("+12 Health T1"), (Some(1), "+12 Health".to_string()));
        assert_eq!(extract_tier("Tower Shield").0, None);
        assert_eq!(extract_tier("T5 +45% Increased Damage").0, Some(5));
    }

    #[test]
    fn fuzzy_helpers() {
        assert!(partial_ratio("inscribedtablet", "assassinsinscribedtabletofregrowth") > 95.0);
        assert!(ratio("brigandlneboots", "brigandineboots") > 90.0);
        assert!(token_sort_ratio("speed movement", "movement speed") > 99.0);
    }
}
