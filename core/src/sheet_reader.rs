//! Read the character sheet (the "C" screen) from OCR boxes: level and the
//! seven resistances. The sheet shows each resistance as `75%` with the
//! uncapped total underneath in parentheses, `(89%)`, when it exceeds the
//! cap; the uncapped number is what the scorer needs.
//!
//! Layout: a row of element names, a row of values, an optional row of
//! parenthesised totals. Values are matched to names by horizontal overlap,
//! so the OCR line grouping does not matter.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::character_state::{CharacterState, ELEMENTS};
use crate::ocr::{self, OcrBox, OcrEngineKind};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SheetReading {
    pub level: Option<u32>,
    /// element -> uncapped total in percent
    pub resistances: HashMap<String, f64>,
    pub endurance: Option<f64>,
    /// the sheet's HEALTH / MANA totals
    pub health: Option<f64>,
    pub mana: Option<f64>,
    /// element name boxes found on the sheet (for the crop fallback)
    #[serde(skip)]
    pub name_boxes: Vec<(String, OcrBox)>,
}

impl SheetReading {
    /// A partial row is still useful: a tooltip often covers the right-hand
    /// elements, and only the values that were read are applied.
    pub fn is_useful(&self) -> bool {
        self.resistances.len() >= 2
    }

    /// Apply to a character state (only the fields that were read).
    pub fn apply(&self, state: &mut CharacterState) -> Vec<String> {
        let mut changes = Vec::new();
        if let Some(level) = self.level {
            if level != state.level {
                changes.push(format!("level {} -> {}", state.level, level));
                state.level = level;
            }
        }
        for (element, value) in &self.resistances {
            let old = state.resistance(element);
            if (old - value).abs() >= 0.5 {
                changes.push(format!("{element} {old:.0} -> {value:.0}"));
            }
            state.resistances.insert(element.clone(), *value);
        }
        if let Some(endurance) = self.endurance {
            if (state.endurance - endurance).abs() >= 0.5 {
                changes.push(format!("endurance {:.0} -> {endurance:.0}", state.endurance));
            }
            state.endurance = endurance;
        }
        if let Some(health) = self.health {
            if (state.health - health).abs() >= 0.5 {
                changes.push(format!("health {:.0} -> {health:.0}", state.health));
            }
            state.health = health;
        }
        if let Some(mana) = self.mana {
            if (state.mana - mana).abs() >= 0.5 {
                changes.push(format!("mana {:.0} -> {mana:.0}", state.mana));
            }
            state.mana = mana;
        }
        changes
    }
}

fn is_percent_text(text: &str) -> bool {
    crate::text::fix_ocr_percent(text).contains('%')
}

fn percent_value(text: &str) -> Option<f64> {
    let fixed = crate::text::fix_ocr_percent(text);
    if !fixed.contains('%') {
        return None;
    }
    let digits: String = fixed.chars().take_while(|c| *c != '%').filter(|c| c.is_ascii_digit() || *c == '.').collect();
    digits.parse().ok()
}

/// The sheet's big "HEALTH 1734" / "MANA 126" figures: an upper-case label
/// with the number in the same box or in the next box on the same row.
/// Lower-case "Health"/"Mana" rows of the stat tables never match.
fn labelled_number(boxes: &[OcrBox], label: &str) -> Option<f64> {
    for b in boxes {
        let text = b.text.trim();
        let Some(rest) = text.strip_prefix(label) else { continue };
        let rest = rest.trim();
        if rest.is_empty() {
            // number in the next box on the same row, close to the label
            let mut row: Vec<&OcrBox> = boxes
                .iter()
                .filter(|n| (n.y - b.y).abs() < b.h.max(10.0) && n.x > b.x + b.w * 0.8 && n.x - (b.x + b.w) < b.w * 1.5)
                .collect();
            row.sort_by(|p, q| p.x.partial_cmp(&q.x).unwrap());
            if let Some(v) = row.iter().filter_map(|n| n.text.trim().replace(',', "").parse::<f64>().ok()).next() {
                return Some(v);
            }
        } else if let Ok(v) = rest.replace(',', "").parse::<f64>() {
            return Some(v);
        }
    }
    None
}

fn overlaps(a: &OcrBox, b: &OcrBox) -> bool {
    let overlap = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    overlap > 0.3 * a.w.min(b.w).max(1.0)
}

/// Returns a reading when the boxes contain the sheet's RESISTANCES block.
pub fn read_sheet(boxes: &[OcrBox]) -> Option<SheetReading> {
    // the header is sometimes clipped by an overlapping tooltip ("RESIS'")
    let header = boxes.iter().find(|b| {
        let t = b.text.trim().to_lowercase();
        t == "resistances" || (t.len() >= 5 && "resistances".starts_with(t.trim_end_matches(|c: char| !c.is_ascii_alphabetic())) && t.starts_with("resis"))
    })?;
    let mut reading = SheetReading::default();

    // element name boxes below the header (within ~6 text heights)
    for element in ELEMENTS {
        let name = boxes.iter().find(|b| {
            b.text.trim().eq_ignore_ascii_case(element) && b.y > header.y && b.y < header.y + 6.0 * header.h.max(10.0)
        });
        let Some(name) = name else { continue };
        // value boxes under the name: plain "75%" first, then "(89%)" if present
        let mut candidates: Vec<&OcrBox> = boxes
            .iter()
            .filter(|b| b.y > name.y && b.y < name.y + 5.0 * name.h.max(10.0) && overlaps(name, b) && is_percent_text(&b.text))
            .collect();
        candidates.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
        let plain = candidates.iter().find(|b| !b.text.contains('(')).and_then(|b| percent_value(&b.text));
        let uncapped = candidates.iter().find(|b| b.text.contains('(')).and_then(|b| percent_value(&b.text));
        if let Some(value) = uncapped.or(plain) {
            reading.resistances.insert(element.to_string(), value);
        }
    }

    // element boxes kept for the crop fallback
    let name_boxes: Vec<(&str, &OcrBox)> = ELEMENTS.iter().filter_map(|element| {
        boxes.iter().find(|b| b.text.trim().eq_ignore_ascii_case(element) && b.y > header.y && b.y < header.y + 6.0 * header.h.max(10.0)).map(|b| (*element, b))
    }).collect();
    reading.name_boxes = name_boxes.iter().map(|(e, b)| (e.to_string(), (*b).clone())).collect();

    // level: a "LEVEL" box with a number box right under it
    if let Some(label) = boxes.iter().find(|b| b.text.trim().eq_ignore_ascii_case("level")) {
        let number = boxes
            .iter()
            .filter(|b| b.y > label.y && b.y < label.y + 4.0 * label.h.max(10.0) && overlaps(label, b))
            .filter_map(|b| b.text.trim().parse::<u32>().ok())
            .next();
        reading.level = number;
    }
    // endurance (defense tab): "Endurance" label with a percentage to the right on the same row
    // (nearest percentage to the right; a tooltip further along the row must not win)
    if let Some(label) = boxes.iter().find(|b| b.text.trim().eq_ignore_ascii_case("endurance")) {
        let mut row: Vec<&OcrBox> = boxes
            .iter()
            .filter(|b| (b.y - label.y).abs() < label.h.max(10.0) && b.x > label.x + label.w && b.x - (label.x + label.w) < 6.0 * label.w.max(40.0))
            .collect();
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        reading.endurance = row.iter().filter_map(|b| percent_value(&b.text)).next();
    }
    reading.health = labelled_number(boxes, "HEALTH").filter(|v| (1.0..100000.0).contains(v));
    reading.mana = labelled_number(boxes, "MANA").filter(|v| (1.0..100000.0).contains(v));
    Some(reading)
}

/// Full-image OCR misses small value cells now and then. For every element
/// without a value, OCR an upscaled crop of the cell under its name.
pub fn read_sheet_from_image(image: &image::RgbaImage, boxes: &[OcrBox], engine: OcrEngineKind) -> Option<SheetReading> {
    let mut reading = read_sheet(boxes)?;
    let (iw, ih) = image.dimensions();
    for (element, name) in reading.name_boxes.clone() {
        if reading.resistances.contains_key(&element) {
            continue;
        }
        let pad = name.w * 0.6;
        let x0 = (name.x - pad).max(0.0) as u32;
        let y0 = (name.y + name.h * 0.9).max(0.0) as u32;
        let x1 = ((name.x + name.w + pad) as u32).min(iw);
        let y1 = ((name.y + name.h * 5.0) as u32).min(ih);
        if x1 <= x0 + 4 || y1 <= y0 + 4 {
            continue;
        }
        let crop = image::imageops::crop_imm(image, x0, y0, x1 - x0, y1 - y0).to_image();
        let big = image::imageops::resize(&crop, (x1 - x0) * 3, (y1 - y0) * 3, image::imageops::FilterType::CatmullRom);
        let Ok(found) = ocr::recognize_rgba(&big, engine, None) else { continue };
        let mut cells: Vec<&OcrBox> = found.iter().filter(|b| is_percent_text(&b.text)).collect();
        cells.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
        let plain = cells.iter().find(|b| !b.text.contains('(')).and_then(|b| percent_value(&b.text));
        let uncapped = cells.iter().find(|b| b.text.contains('(')).and_then(|b| percent_value(&b.text));
        if let Some(value) = uncapped.or(plain) {
            reading.resistances.insert(element, value);
        }
    }
    Some(reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::Tint;

    fn b(text: &str, x: f32, y: f32, w: f32) -> OcrBox {
        OcrBox { text: text.into(), x, y, w, h: 16.0, tint: Tint::Neutral }
    }

    #[test]
    fn reads_resistances_with_uncapped_totals_and_level() {
        let mut boxes = vec![b("RESISTANCES", 400.0, 400.0, 150.0), b("LEVEL", 110.0, 90.0, 50.0), b("54", 120.0, 110.0, 30.0)];
        let names = ["FIRE", "LIGHTNING", "COLD", "PHYSICAL", "POISON", "NECROTIC", "VOID"];
        let plain = ["75%", "73%", "66%", "75%", "23%", "74%", "75%"];
        let extra = [Some("(89%)"), None, None, Some("(77%)"), None, None, Some("(103%)")];
        for (i, name) in names.iter().enumerate() {
            let x = 100.0 + 110.0 * i as f32;
            boxes.push(b(name, x, 480.0, 70.0));
            boxes.push(b(plain[i], x + 10.0, 505.0, 40.0));
            if let Some(e) = extra[i] {
                boxes.push(b(e, x + 5.0, 528.0, 50.0));
            }
        }
        let reading = read_sheet(&boxes).unwrap();
        assert!(reading.is_useful());
        assert_eq!(reading.level, Some(54));
        assert_eq!(reading.resistances["fire"], 89.0);
        assert_eq!(reading.resistances["lightning"], 73.0);
        assert_eq!(reading.resistances["physical"], 77.0);
        assert_eq!(reading.resistances["void"], 103.0);
        let mut state = CharacterState::default();
        state.resistances.insert("fire".into(), 62.0);
        let changes = reading.apply(&mut state);
        assert!(changes.iter().any(|c| c.starts_with("fire 62 -> 89")));
        assert_eq!(state.level, 54);
    }

    #[test]
    fn partial_row_behind_a_tooltip_still_reads_the_visible_elements() {
        let mut boxes = vec![b("RESIS'", 400.0, 400.0, 60.0)];
        for (i, (name, plain, extra)) in [("FIRE", "75%", "(136%)"), ("LIGHTNING", "75%", "(125%)"), ("COLD", "75%", "(121%)")].iter().enumerate() {
            let x = 100.0 + 110.0 * i as f32;
            boxes.push(b(name, x, 480.0, 70.0));
            boxes.push(b(plain, x + 10.0, 505.0, 40.0));
            boxes.push(b(extra, x + 5.0, 528.0, 50.0));
        }
        let reading = read_sheet(&boxes).unwrap();
        assert!(reading.is_useful());
        assert_eq!(reading.resistances.len(), 3);
        assert_eq!(reading.resistances["cold"], 121.0);
        let mut state = CharacterState::default();
        state.resistances.insert("void".into(), 53.0);
        reading.apply(&mut state);
        assert_eq!(state.resistance("void"), 53.0, "hidden elements keep their old value");
    }

    #[test]
    fn endurance_takes_the_nearest_percentage_on_its_row() {
        let mut boxes = vec![b("RESISTANCES", 400.0, 400.0, 150.0), b("FIRE", 100.0, 480.0, 70.0), b("75%", 110.0, 505.0, 40.0), b("COLD", 210.0, 480.0, 70.0), b("66%", 220.0, 505.0, 40.0)];
        boxes.push(b("Endurance", 150.0, 962.0, 88.0));
        boxes.push(b("+28%", 1182.0, 962.0, 40.0)); // a tooltip line on the same row, listed first
        boxes.push(b("57%", 510.0, 962.0, 40.0));
        let reading = read_sheet(&boxes).unwrap();
        assert_eq!(reading.endurance, Some(57.0));
    }

    #[test]
    fn health_and_mana_come_from_the_upper_case_headers_only() {
        let mut boxes = vec![b("RESISTANCES", 400.0, 400.0, 150.0), b("FIRE", 100.0, 480.0, 70.0), b("75%", 110.0, 505.0, 40.0), b("COLD", 210.0, 480.0, 70.0), b("66%", 220.0, 505.0, 40.0)];
        boxes.push(b("HEALTH", 265.0, 328.0, 88.0));
        boxes.push(b("1734", 363.0, 328.0, 47.0));
        boxes.push(b("MANA 330", 600.0, 328.0, 90.0));
        boxes.push(b("Health", 559.0, 924.0, 55.0)); // stat table row, ignored
        boxes.push(b("0", 700.0, 924.0, 10.0));
        boxes.push(b("MANA", 639.0, 716.0, 53.0)); // "+21 MANA" from a tooltip: no number to its right
        let reading = read_sheet(&boxes).unwrap();
        assert_eq!(reading.health, Some(1734.0));
        assert_eq!(reading.mana, Some(330.0));
        let mut state = CharacterState::default();
        let changes = reading.apply(&mut state);
        assert!(changes.iter().any(|c| c.starts_with("mana 0 -> 330")));
    }

    #[test]
    fn no_sheet_means_none() {
        assert!(read_sheet(&[b("NOMAD BELT", 0.0, 0.0, 50.0)]).is_none());
    }
}
