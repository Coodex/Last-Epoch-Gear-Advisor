//! OCR of tooltip screenshots and grouping of the result into panels.
//!
//! Engine: the Windows built-in OCR (`Windows.Media.Ocr`) through the
//! pure-Rust `windows` crate, so no native build tools are needed. If a
//! `tesseract.exe` is installed it can be used instead (`OcrEngineKind::Tesseract`).
//!
//! Grouping (validated on real captures): word boxes are merged into lines
//! when they share a baseline and sit close together; lines are merged into
//! panels when their x-ranges overlap. Hovering a bag item shows the item's
//! tooltip AND the equipped item's tooltip headed "EQUIPPED" side by side,
//! so the caller skips panels whose first lines say EQUIPPED and judges the
//! panel nearest the cursor.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrEngineKind {
    Auto,
    Windows,
    Tesseract,
}

/// Text colour class. The compare-with-equipped block under a tooltip is the
/// only green/red text in it, so colour is the reliable way to cut it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tint {
    #[default]
    Neutral,
    Green,
    Red,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrBox {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub tint: Tint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    pub lines: Vec<String>,
    /// colour class per line (same length as `lines`)
    #[serde(default)]
    pub tints: Vec<Tint>,
    /// x, y, w, h in image pixels
    pub rect: (i32, i32, i32, i32),
    /// distance from the cursor to the panel's nearest edge (0 when no cursor given)
    pub distance: f32,
}

impl Panel {
    /// Touches the capture's left edge: its lines are cut and cannot be trusted.
    pub fn is_clipped(&self) -> bool {
        self.rect.0 <= 2
    }

    pub fn is_equipped_compare(&self) -> bool {
        self.lines.iter().take(3).any(|l| l.trim().to_lowercase().replace(' ', "") == "equipped")
    }

    /// Lines up to (not including) the first green/red line after the title
    /// block: the item itself, without the compare-with-equipped differences.
    /// The first three lines (title, type line, unique base) are never cut:
    /// unique names are orange and class-restricted type lines are red.
    pub fn item_lines(&self) -> Vec<String> {
        self.lines[..self.compare_cut()].to_vec()
    }

    fn compare_cut(&self) -> usize {
        self.tints.iter().enumerate().skip(3).find(|(_, t)| **t != Tint::Neutral).map(|(i, _)| i).unwrap_or(self.lines.len())
    }

    /// The game's own "compare with equipped" block under the item: each line
    /// tagged by its colour (green = better than equipped, red = worse).
    /// Empty when Auto Compare is off or the tooltip had no coloured lines.
    pub fn compare_lines(&self) -> Vec<String> {
        let cut = self.compare_cut();
        self.lines[cut..]
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let tag = match self.tints.get(cut + i) {
                    Some(Tint::Green) => "[better] ",
                    Some(Tint::Red) => "[worse] ",
                    _ => "",
                };
                format!("{tag}{line}")
            })
            .collect()
    }
}

/// Classify the text colour inside a box: mean colour of the bright (text)
/// pixels. Green/red text is clearly saturated; item text is white/gold/grey.
pub fn tint_of(image: &image::RgbaImage, b: &OcrBox) -> Tint {
    let (iw, ih) = image.dimensions();
    let x0 = b.x.max(0.0) as u32;
    let y0 = b.y.max(0.0) as u32;
    let x1 = ((b.x + b.w).max(0.0) as u32).min(iw);
    let y1 = ((b.y + b.h).max(0.0) as u32).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return Tint::Neutral;
    }
    let (mut r, mut g, mut bl, mut n) = (0u64, 0u64, 0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let p = image.get_pixel(x, y);
            let max = p[0].max(p[1]).max(p[2]);
            if max < 110 {
                continue; // background
            }
            r += p[0] as u64;
            g += p[1] as u64;
            bl += p[2] as u64;
            n += 1;
        }
    }
    if n < 8 {
        return Tint::Neutral;
    }
    let (r, g, bl) = (r as f64 / n as f64, g as f64 / n as f64, bl as f64 / n as f64);
    // compare green ~ (110,210,100); compare red ~ (210,70,60); unique orange
    // ~ (225,150,60) and gold ~ (212,170,80) keep far more green than red text
    if g > 110.0 && g > r * 1.25 && g > bl * 1.25 {
        Tint::Green
    } else if r > 120.0 && g < r * 0.5 && bl < r * 0.6 {
        Tint::Red
    } else {
        Tint::Neutral
    }
}

/// Fill in `tint` for every box from the capture it came from.
pub fn tint_boxes(image: &image::RgbaImage, boxes: &mut [OcrBox]) {
    for b in boxes.iter_mut() {
        b.tint = tint_of(image, b);
    }
}

/// Majority tint of a line's words (ties resolve to Neutral).
pub fn line_tint(line: &[OcrBox]) -> Tint {
    let green = line.iter().filter(|b| b.tint == Tint::Green).count();
    let red = line.iter().filter(|b| b.tint == Tint::Red).count();
    let neutral = line.len() - green - red;
    if green > neutral && green >= red {
        Tint::Green
    } else if red > neutral && red > green {
        Tint::Red
    } else {
        Tint::Neutral
    }
}

/// Run OCR on an image file (png/jpeg/bmp).
pub fn recognize_file(path: &Path, engine: OcrEngineKind) -> anyhow::Result<Vec<OcrBox>> {
    let image = image::open(path)?.to_rgba8();
    recognize_rgba(&image, engine, Some(path))
}

/// Run OCR on RGBA pixels. `source` is only used for the tesseract fallback
/// (it reads files) and may be omitted.
pub fn recognize_rgba(image: &image::RgbaImage, engine: OcrEngineKind, source: Option<&Path>) -> anyhow::Result<Vec<OcrBox>> {
    let mut boxes = match engine {
        OcrEngineKind::Tesseract => tesseract::recognize(image, source)?,
        OcrEngineKind::Windows => windows_ocr::recognize(image)?,
        OcrEngineKind::Auto => match windows_ocr::recognize(image) {
            Ok(boxes) => boxes,
            Err(err) => tesseract::recognize(image, source).map_err(|e| anyhow::anyhow!("windows OCR failed ({err}); tesseract failed ({e})"))?,
        },
    };
    tint_boxes(image, &mut boxes);
    Ok(boxes)
}

/// Merge boxes on one baseline that are horizontally close into lines (so a
/// "T3" label joins its affix but text from another panel does not).
/// Baselines are formed first (by vertical centre), then each baseline is
/// split where the horizontal gap exceeds three text heights.
pub fn group_lines(boxes: &[OcrBox]) -> Vec<Vec<OcrBox>> {
    let mut ordered: Vec<&OcrBox> = boxes.iter().collect();
    ordered.sort_by(|a, b| (a.y + a.h / 2.0).partial_cmp(&(b.y + b.h / 2.0)).unwrap());
    // 1. baselines
    let mut baselines: Vec<Vec<&OcrBox>> = Vec::new();
    for b in ordered {
        let centre = b.y + b.h / 2.0;
        let mut placed = false;
        for line in baselines.iter_mut() {
            let ref_c = line.iter().map(|l| l.y + l.h / 2.0).sum::<f32>() / line.len() as f32;
            let ref_h = line.iter().map(|l| l.h).fold(0.0, f32::max);
            if (centre - ref_c).abs() <= 0.6 * ref_h.max(b.h) {
                line.push(b);
                placed = true;
                break;
            }
        }
        if !placed {
            baselines.push(vec![b]);
        }
    }
    // 2. split each baseline at large horizontal gaps
    let mut lines: Vec<Vec<OcrBox>> = Vec::new();
    for mut line in baselines {
        line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let mut current: Vec<OcrBox> = Vec::new();
        for b in line {
            if let Some(last) = current.last() {
                let gap = b.x - (last.x + last.w);
                let h = current.iter().map(|l| l.h).fold(0.0, f32::max).max(b.h);
                if gap > 3.0 * h {
                    lines.push(std::mem::take(&mut current));
                }
            }
            current.push(b.clone());
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines.sort_by(|a, b| {
        let ay = a.iter().map(|l| l.y).fold(f32::MAX, f32::min);
        let by = b.iter().map(|l| l.y).fold(f32::MAX, f32::min);
        (ay, a[0].x).partial_cmp(&(by, b[0].x)).unwrap()
    });
    lines
}

pub fn line_text(line: &[OcrBox]) -> String {
    line.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join(" ")
}

fn bounds(group: &[OcrBox]) -> (f32, f32, f32, f32) {
    let x0 = group.iter().map(|b| b.x).fold(f32::MAX, f32::min);
    let y0 = group.iter().map(|b| b.y).fold(f32::MAX, f32::min);
    let x1 = group.iter().map(|b| b.x + b.w).fold(f32::MIN, f32::max);
    let y1 = group.iter().map(|b| b.y + b.h).fold(f32::MIN, f32::max);
    (x0, y0, x1, y1)
}

/// Group LINE boxes into panels.
///
/// A tooltip is a column of left-aligned lines; its header block (title, type
/// line, implicits) is indented beside the item icon and sits directly above
/// the affix column. Adjacent tooltips have different left edges, so
/// clustering by left edge separates them even when their line widths
/// overlap. Right-aligned fragments ("Requires: Level 45", the "EQUIPPED"
/// label) attach to the panel whose vertical span contains them.
pub fn split_panels(lines: &[OcrBox]) -> Vec<Vec<OcrBox>> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut heights: Vec<f32> = lines.iter().map(|b| b.h).collect();
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let h = heights[heights.len() / 2].max(8.0);
    let tol = 1.6 * h;

    // 1. columns by left edge, split at large vertical gaps
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|a, b| (lines[*a].x, lines[*a].y).partial_cmp(&(lines[*b].x, lines[*b].y)).unwrap());
    let mut columns: Vec<Vec<usize>> = Vec::new();
    for i in order {
        match columns.iter_mut().find(|c| (lines[c[0]].x - lines[i].x).abs() <= tol) {
            Some(col) => col.push(i),
            None => columns.push(vec![i]),
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for mut col in columns {
        col.sort_by(|a, b| lines[*a].y.partial_cmp(&lines[*b].y).unwrap());
        let mut current: Vec<usize> = Vec::new();
        for i in col {
            if let Some(&last) = current.last() {
                let gap = lines[i].y - (lines[last].y + lines[last].h);
                if gap > 9.0 * h {
                    groups.push(std::mem::take(&mut current));
                }
            }
            current.push(i);
        }
        if !current.is_empty() {
            groups.push(current);
        }
    }

    let bounds_of = |g: &[usize]| -> (f32, f32, f32, f32) {
        let x0 = g.iter().map(|&i| lines[i].x).fold(f32::MAX, f32::min);
        let y0 = g.iter().map(|&i| lines[i].y).fold(f32::MAX, f32::min);
        let x1 = g.iter().map(|&i| lines[i].x + lines[i].w).fold(f32::MIN, f32::max);
        let y1 = g.iter().map(|&i| lines[i].y + lines[i].h).fold(f32::MIN, f32::max);
        (x0, y0, x1, y1)
    };

    // 2. merge a header block (indented, above) with the affix column below it
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..groups.len() {
            for j in 0..groups.len() {
                if i == j || groups[i].len() < 2 {
                    continue; // the header block has several lines; the affix column may have one
                }
                let (ax0, _ay0, ax1, ay1) = bounds_of(&groups[i]); // upper (header)
                let (bx0, by0, bx1, _by1) = bounds_of(&groups[j]); // lower (affixes)
                let gh = groups[i].iter().chain(groups[j].iter()).map(|&k| lines[k].h).fold(0.0, f32::max).max(h);
                let stacked = by0 >= ay1 - gh && by0 - ay1 <= 8.0 * gh;
                let overlap = ax1.min(bx1) - ax0.max(bx0);
                let indented_over = ax0 >= bx0 - tol && ax0 <= bx1 && overlap > 0.0;
                if stacked && indented_over {
                    let lower = groups.remove(j);
                    let upper_index = if j < i { i - 1 } else { i };
                    groups[upper_index].extend(lower);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }

    // 3. attach single-line fragments to the panel they lie in
    let mut panels: Vec<Vec<usize>> = groups.iter().filter(|g| g.len() >= 2).cloned().collect();
    let fragments: Vec<usize> = groups.iter().filter(|g| g.len() < 2).flatten().copied().collect();
    let mut leftovers: Vec<Vec<usize>> = Vec::new();
    for f in fragments {
        let (fx0, fy0, fx1, fy1) = (lines[f].x, lines[f].y, lines[f].x + lines[f].w, lines[f].y + lines[f].h);
        let mut best: Option<(usize, f32)> = None;
        for (k, panel) in panels.iter().enumerate() {
            let (x0, y0, x1, y1) = bounds_of(panel);
            let width = (x1 - x0).max(1.0);
            let vertically_in = fy1 >= y0 - 3.0 * h && fy0 <= y1 + 2.0 * h;
            let horizontally_in = fx0 >= x0 - tol && fx0 <= x1 + 0.6 * width && fx1 <= x1 + 1.0 * width;
            if vertically_in && horizontally_in && best.map_or(true, |(_, bx)| x0 > bx) {
                best = Some((k, x0));
            }
        }
        match best {
            Some((k, _)) => panels[k].push(f),
            None => leftovers.push(vec![f]),
        }
    }
    panels.extend(leftovers);
    panels
        .into_iter()
        .map(|g| {
            let mut boxes: Vec<OcrBox> = g.into_iter().map(|i| lines[i].clone()).collect();
            boxes.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
            boxes
        })
        .collect()
}

/// Panels with their text lines, nearest to the cursor first (tiny fragments last).
/// Words are merged into lines first, then whole lines are grouped into
/// panels, so the first word of a line can never end up in another column.
pub fn panels(boxes: &[OcrBox], cursor: Option<(f32, f32)>) -> Vec<Panel> {
    let line_boxes: Vec<OcrBox> = group_lines(boxes)
        .iter()
        .map(|line| {
            let (x0, y0, x1, y1) = bounds(line);
            OcrBox { text: line_text(line), x: x0, y: y0, w: x1 - x0, h: y1 - y0, tint: line_tint(line) }
        })
        .collect();
    let mut out: Vec<Panel> = split_panels(&line_boxes)
        .into_iter()
        .map(|group| {
            let (x0, y0, x1, y1) = bounds(&group);
            let distance = cursor
                .map(|(cx, cy)| {
                    let dx = (x0 - cx).max(cx - x1).max(0.0);
                    let dy = (y0 - cy).max(cy - y1).max(0.0);
                    (dx * dx + dy * dy).sqrt()
                })
                .unwrap_or(0.0);
            let lines = group_lines(&group);
            Panel {
                tints: lines.iter().map(|l| line_tint(l)).collect(),
                lines: lines.iter().map(|l| line_text(l)).collect(),
                rect: (x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32),
                distance,
            }
        })
        .collect();
    let labels: Vec<OcrBox> = line_boxes.iter().filter(|b| is_equipped_word(&b.text)).cloned().collect();
    attach_equipped_labels(&mut out, &labels);
    out.sort_by(|a, b| (a.lines.len() < 3, a.distance).partial_cmp(&(b.lines.len() < 3, b.distance)).unwrap());
    out
}

fn is_equipped_word(text: &str) -> bool {
    text.trim().to_lowercase().replace(' ', "") == "equipped"
}

/// The game draws "EQUIPPED" as a small label with a gap above the compare
/// tooltip, so the label lands in its own panel or in a neighbouring one.
/// Using the label's pixel position, move it to the front of the tooltip
/// directly below it so that panel is recognised as the equipped item.
fn attach_equipped_labels(panels: &mut Vec<Panel>, labels: &[OcrBox]) {
    for label in labels {
        let (lx, ly, lw, lh) = (label.x as i32, label.y as i32, label.w as i32, label.h as i32);
        let label_bottom = ly + lh;
        // the tooltip body: at least 3 lines, horizontally overlapping, starting just below the label
        let mut best: Option<(usize, i32)> = None;
        for (i, p) in panels.iter().enumerate() {
            if p.lines.len() < 3 {
                continue;
            }
            let (px, py, pw, _) = p.rect;
            let overlaps = lx < px + pw && px < lx + lw;
            let gap = py - label_bottom;
            if overlaps && (-lh..=160).contains(&gap) && best.map_or(true, |(_, g)| gap < g) {
                best = Some((i, gap));
            }
        }
        let Some((target, _)) = best else { continue };
        if panels[target].is_equipped_compare() {
            continue;
        }
        // take the label line out of whichever panel holds it (by position)
        let cx = lx + lw / 2;
        let cy = ly + lh / 2;
        let mut emptied: Option<usize> = None;
        for (i, p) in panels.iter_mut().enumerate() {
            let (px, py, pw, ph) = p.rect;
            if i == target || cx < px || cx > px + pw || cy < py || cy > py + ph {
                continue;
            }
            if let Some(pos) = p.lines.iter().position(|l| is_equipped_word(l)) {
                p.lines.remove(pos);
                if pos < p.tints.len() {
                    p.tints.remove(pos);
                }
                if p.lines.is_empty() {
                    emptied = Some(i);
                }
                break;
            }
        }
        let t = &mut panels[target];
        t.lines.insert(0, label.text.trim().to_string());
        t.tints.insert(0, Tint::Neutral);
        let (px, py, pw, ph) = t.rect;
        let x0 = px.min(lx);
        let y0 = py.min(ly);
        let x1 = (px + pw).max(lx + lw);
        let y1 = (py + ph).max(ly + lh);
        t.rect = (x0, y0, x1 - x0, y1 - y0);
        if let Some(i) = emptied {
            panels.remove(i);
        }
    }
}

/// The panel to judge: nearest non-"EQUIPPED" panel with enough lines.
pub fn pick_item_panel(panels: &[Panel]) -> Option<&Panel> {
    panels.iter().find(|p| !p.is_equipped_compare() && p.lines.len() >= 3)
}

#[cfg(windows)]
mod windows_ocr {
    use super::{OcrBox, Tint};
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;

    pub fn recognize(image: &image::RgbaImage) -> anyhow::Result<Vec<OcrBox>> {
        let max_dim = OcrEngine::MaxImageDimension().unwrap_or(2600);
        let (w, h) = image.dimensions();
        let scale = (max_dim as f32 / w.max(h) as f32).min(1.0);
        let working = if scale < 1.0 {
            image::imageops::resize(image, (w as f32 * scale) as u32, (h as f32 * scale) as u32, image::imageops::FilterType::Triangle)
        } else {
            image.clone()
        };
        let (w, h) = working.dimensions();
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for px in working.pixels() {
            bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        let buffer = CryptographicBuffer::CreateFromByteArray(&bgra)?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w as i32, h as i32, BitmapAlphaMode::Premultiplied)?;
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
        let result = engine.RecognizeAsync(&bitmap)?.get()?;
        let mut boxes = Vec::new();
        let lines = result.Lines()?;
        for i in 0..lines.Size()? {
            let line = lines.GetAt(i)?;
            let words = line.Words()?;
            for j in 0..words.Size()? {
                let word = words.GetAt(j)?;
                let rect = word.BoundingRect()?;
                let text = word.Text()?.to_string();
                if text.trim().is_empty() {
                    continue;
                }
                boxes.push(OcrBox {
                    text,
                    x: rect.X / scale,
                    y: rect.Y / scale,
                    w: rect.Width / scale,
                    h: rect.Height / scale,
                    tint: Tint::Neutral,
                });
            }
        }
        Ok(boxes)
    }
}

#[cfg(not(windows))]
mod windows_ocr {
    use super::{OcrBox, Tint};
    pub fn recognize(_image: &image::RgbaImage) -> anyhow::Result<Vec<OcrBox>> {
        anyhow::bail!("Windows OCR is only available on Windows")
    }
}

mod tesseract {
    use super::{OcrBox, Tint};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub fn find_exe() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe"),
        ];
        candidates.into_iter().find(|p| p.exists()).or_else(|| {
            std::env::var_os("PATH").and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|dir| dir.join("tesseract.exe"))
                    .find(|p| p.exists())
            })
        })
    }

    pub fn recognize(image: &image::RgbaImage, source: Option<&Path>) -> anyhow::Result<Vec<OcrBox>> {
        let exe = find_exe().ok_or_else(|| anyhow::anyhow!("tesseract.exe not found"))?;
        let temp;
        let path: &Path = match source {
            Some(p) => p,
            None => {
                temp = std::env::temp_dir().join(format!("le-advisor-ocr-{}.png", std::process::id()));
                image.save(&temp)?;
                &temp
            }
        };
        let output = Command::new(exe).arg(path).arg("stdout").arg("--psm").arg("11").arg("tsv").output()?;
        if !output.status.success() {
            anyhow::bail!("tesseract failed: {}", String::from_utf8_lossy(&output.stderr));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut boxes = Vec::new();
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 12 || cols[0] != "5" {
                continue;
            }
            let conf: f32 = cols[10].parse().unwrap_or(0.0);
            let word = cols[11].trim();
            if conf < 40.0 || word.is_empty() {
                continue;
            }
            boxes.push(OcrBox {
                text: word.to_string(),
                x: cols[6].parse().unwrap_or(0.0),
                y: cols[7].parse().unwrap_or(0.0),
                w: cols[8].parse().unwrap_or(0.0),
                h: cols[9].parse().unwrap_or(0.0),
                tint: Tint::Neutral,
            });
        }
        Ok(boxes)
    }
}

#[cfg(test)]
mod compare_lines_tests {
    use super::*;

    fn panel(lines: &[&str], rect: (i32, i32, i32, i32)) -> Panel {
        Panel { lines: lines.iter().map(|s| s.to_string()).collect(), tints: vec![Tint::Neutral; lines.len()], rect, distance: 0.0 }
    }

    fn label(x: f32, y: f32) -> OcrBox {
        OcrBox { text: "EQUIPPED".into(), x, y, w: 122.0, h: 18.0, tint: Tint::Neutral }
    }

    #[test]
    fn lone_equipped_label_joins_the_tooltip_below() {
        let mut panels = vec![
            panel(&["EQUIPPED"], (792, 185, 122, 18)),
            panel(&["SHIMMERING SILVER", "AMULET OF FRAILTY", "AMULET", "+21 MANA"], (614, 262, 481, 534)),
            panel(&["OTHER", "TOOLTIP", "FAR AWAY"], (1400, 300, 300, 300)),
        ];
        attach_equipped_labels(&mut panels, &[label(792.0, 185.0)]);
        assert_eq!(panels.len(), 2);
        assert!(panels[0].is_equipped_compare());
        assert_eq!(panels[0].lines[1], "SHIMMERING SILVER");
        assert_eq!(panels[0].rect.1, 185);
        assert!(!panels[1].is_equipped_compare());
        // a label with nothing under it stays alone
        let mut lonely = vec![panel(&["EQUIPPED"], (10, 10, 100, 18)), panel(&["A", "B", "C"], (10, 900, 100, 100))];
        attach_equipped_labels(&mut lonely, &[label(10.0, 10.0)]);
        assert_eq!(lonely.len(), 2);
    }

    #[test]
    fn equipped_label_inside_a_neighbouring_panel_still_joins_the_tooltip() {
        // the label merged into a wide panel to the left (sheet header etc.), body below it
        let mut panels = vec![
            panel(&["LEVEL 60", "PALADIN", "EQUIPPED", "VITALITY 10", "STRENGTH 17"], (96, 73, 1072, 808)),
            panel(&["SHIMMERING SILVER", "AMULET OF FRAILTY", "AMULET", "+21 MANA"], (612, 262, 480, 534)),
        ];
        attach_equipped_labels(&mut panels, &[label(792.0, 185.0)]);
        assert_eq!(panels.len(), 2);
        assert!(!panels[0].lines.iter().any(|l| l == "EQUIPPED"));
        assert!(panels[1].is_equipped_compare());
        assert_eq!(panels[1].lines[0], "EQUIPPED");
    }

    #[test]
    fn compare_block_is_tagged_by_colour() {
        let panel = Panel {
            lines: ["Shrine Boots", "BOOTS", "+8 Armor", "+35 Health", "+35 Health", "-6 Vitality", "-11% Movement Speed"].iter().map(|s| s.to_string()).collect(),
            tints: vec![Tint::Neutral, Tint::Neutral, Tint::Neutral, Tint::Neutral, Tint::Green, Tint::Red, Tint::Red],
            rect: (10, 10, 100, 100),
            distance: 0.0,
        };
        assert_eq!(panel.item_lines().len(), 4);
        assert_eq!(panel.compare_lines(), vec!["[better] +35 Health", "[worse] -6 Vitality", "[worse] -11% Movement Speed"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(text: &str, x: f32, y: f32, w: f32) -> OcrBox {
        OcrBox { text: text.into(), x, y, w, h: 20.0, tint: Tint::Neutral }
    }

    #[test]
    fn adjacent_tooltips_with_overlapping_widths_stay_separate() {
        let mut boxes = Vec::new();
        for (i, t) in ["MANAFUSED NOBLE PLATE", "OF THE OX", "SENTINEL BODY ARMOR", "+240 ARMOR", "+29% LIGHTNING RESISTANCE", "19 FORGING POTENTIAL"].iter().enumerate() {
            boxes.push(b(t, 650.0, 100.0 + 26.0 * i as f32, 300.0));
        }
        for (i, t) in ["+9 MANA", "10% INCREASED HEALTH", "15% INCREASED ARMOR"].iter().enumerate() {
            boxes.push(b(t, 520.0, 300.0 + 30.0 * i as f32, 260.0));
        }
        boxes.push(b("Requires: Level 42 Sentinel", 800.0, 410.0, 200.0));
        boxes.push(b("EQUIPPED", 230.0, 40.0, 100.0));
        for (i, t) in ["VITAL WORN PLATE OF THE", "OX", "SENTINEL BODY ARMOR", "+75 ARMOR", "25 FORGING POTENTIAL"].iter().enumerate() {
            boxes.push(b(t, 190.0, 100.0 + 26.0 * i as f32, 320.0));
        }
        for (i, t) in ["+4 VITALITY", "13% INCREASED HEALTH"].iter().enumerate() {
            boxes.push(b(t, 60.0, 300.0 + 30.0 * i as f32, 240.0));
        }
        boxes.push(b("Requires: Level 26 Sentinel", 330.0, 410.0, 200.0));
        let panels = panels(&boxes, Some((760.0, 300.0)));
        let hovered = pick_item_panel(&panels).unwrap();
        assert_eq!(hovered.lines[0], "MANAFUSED NOBLE PLATE", "{:?}", panels.iter().map(|p| p.lines.clone()).collect::<Vec<_>>());
        assert!(hovered.lines.contains(&"+9 MANA".to_string()));
        assert!(hovered.lines.contains(&"Requires: Level 42 Sentinel".to_string()));
        assert!(!hovered.lines.iter().any(|l| l.contains("VITALITY")));
        let equipped = panels.iter().find(|p| p.is_equipped_compare()).unwrap();
        assert!(equipped.lines.contains(&"13% INCREASED HEALTH".to_string()));
    }

    #[test]
    fn tint_classification_and_item_lines() {
        let mut img = image::RgbaImage::from_pixel(120, 60, image::Rgba([20, 18, 15, 255]));
        for x in 5..55 { for y in 5..15 { img.put_pixel(x, y, image::Rgba([230, 225, 210, 255])); } }   // white text
        for x in 5..55 { for y in 25..35 { img.put_pixel(x, y, image::Rgba([90, 210, 100, 255])); } }   // green text
        for x in 5..55 { for y in 45..55 { img.put_pixel(x, y, image::Rgba([220, 80, 70, 255])); } }    // red text
        let mut boxes = vec![b("+39% Physical Resistance", 5.0, 5.0, 50.0), b("+1 Potion Slots", 5.0, 25.0, 50.0), b("19% reduced Fire", 5.0, 45.0, 50.0)];
        for bx in boxes.iter_mut() { bx.h = 10.0; }
        tint_boxes(&img, &mut boxes);
        assert_eq!(boxes.iter().map(|b| b.tint).collect::<Vec<_>>(), vec![Tint::Neutral, Tint::Green, Tint::Red]);
        // orange unique titles and gold text are not "red"
        let orange = OcrBox { text: "KNOWLEDGE OF AN ERASED MAGE".into(), x: 5.0, y: 5.0, w: 50.0, h: 10.0, tint: Tint::Neutral };
        let mut img2 = image::RgbaImage::from_pixel(120, 20, image::Rgba([20, 18, 15, 255]));
        for x in 5..55 { for y in 5..15 { img2.put_pixel(x, y, image::Rgba([225, 150, 60, 255])); } }
        assert_eq!(tint_of(&img2, &orange), Tint::Neutral);
        // a red type line inside the title block never cuts; a red line later does
        let panel = Panel {
            lines: vec!["KNOWLEDGE OF AN ERASED MAGE".into(), "MAGE RELIC".into(), "UNIQUE WARDING SCROLL".into(), "+40 Mana".into(), "+1 to Mage Skills".into(), "9% reduced Armor".into()],
            tints: vec![Tint::Neutral, Tint::Red, Tint::Neutral, Tint::Neutral, Tint::Neutral, Tint::Red],
            rect: (0, 0, 1, 1), distance: 0.0,
        };
        assert_eq!(panel.item_lines().len(), 5);
        // with a title block in front, the cut lands on the first coloured line after it
        let mut full = vec![b("NOMAD BELT OF", 5.0, -70.0, 50.0), b("DEFLECTION", 5.0, -50.0, 50.0), b("BELT", 5.0, -30.0, 30.0)];
        full.extend(boxes);
        let panels = panels(&full, None);
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].item_lines(), vec!["NOMAD BELT OF", "DEFLECTION", "BELT", "+39% Physical Resistance"]);
    }

    #[test]
    fn tier_label_joins_affix_but_far_text_does_not() {
        let boxes = vec![
            b("+18% Increased Movement Speed", 100.0, 100.0, 300.0),
            b("T3", 420.0, 101.0, 30.0),
            b("ITEMS RESOURCES", 1200.0, 102.0, 200.0),
            b("+12 Health", 100.0, 130.0, 120.0),
        ];
        let lines: Vec<String> = group_lines(&boxes).iter().map(|l| line_text(l)).collect();
        assert_eq!(lines, vec!["+18% Increased Movement Speed T3", "ITEMS RESOURCES", "+12 Health"]);
    }

    #[test]
    fn side_by_side_tooltips_become_separate_panels() {
        let mut boxes = Vec::new();
        for (i, t) in ["PROTECTIVE TOWER", "SHIELD", "+28% BLOCK CHANCE", "+14% POISON RESISTANCE"].iter().enumerate() {
            boxes.push(b(t, 500.0, 100.0 + 30.0 * i as f32, 260.0));
        }
        boxes.push(b("Requires: Level 30", 700.0, 240.0, 150.0)); // right-aligned, past the widest line
        for (i, t) in ["EQUIPPED", "GUARDIAN'S ROOT SHIELD", "SHIELD", "+21% BLOCK CHANCE"].iter().enumerate() {
            boxes.push(b(t, 60.0, 100.0 + 30.0 * i as f32, 260.0));
        }
        boxes.push(b("87 FPS", 900.0, 10.0, 60.0));
        let panels = panels(&boxes, Some((760.0, 150.0)));
        assert_eq!(panels.len(), 3);
        let item = pick_item_panel(&panels).unwrap();
        assert_eq!(item.lines[0], "PROTECTIVE TOWER");
        assert_eq!(item.lines.last().unwrap(), "Requires: Level 30");
        assert!(panels.iter().any(|p| p.is_equipped_compare()));
    }
}
