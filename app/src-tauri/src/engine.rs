//! One hotkey press: capture -> OCR -> parse hovered + "EQUIPPED" tooltips ->
//! compare -> card for the overlay. The AI hotkey runs the same pipeline and
//! then sends the tooltip crops plus the deterministic verdict to the chosen
//! model. Guides pasted in the Builds window become guide profiles here too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use le_core::ai::{self, Client, ModelConfig};
use le_core::build_library::{Build, BuildLibrary, BuildSummary, BUILTIN_ID};
use le_core::character_state::{CharacterState, EquippedProfile};
use le_core::compare::{compare, Verdict, VerdictLabel};
use le_core::game_data::GameData;
use le_core::guide_profile::{self, GuideProfile};
use le_core::item_parser::{parse_tooltip, ParsedItem};
use le_core::ocr::{self, OcrEngineKind, Panel};

use crate::capture::{self, Capture};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderSettings {
    /// Empty: the provider's environment variable is used.
    pub api_key: String,
    /// Empty: the provider's default endpoint (required for "openai-compatible").
    pub base_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: &str, model: &str) -> ModelRef {
        ModelRef { provider: provider.into(), model: model.into() }
    }

    pub fn is_set(&self) -> bool {
        !self.provider.trim().is_empty() && !self.model.trim().is_empty()
    }
}

/// `settings.json` → `ai`. Keys live per provider; models are picked from the
/// provider's catalogue (fetched once and cached here) for two jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Hotkey for the AI-powered analysis
    pub hotkey_ai: String,
    /// Hotkey that cycles the item-analysis model through `cycle`
    pub hotkey_cycle: String,
    /// provider id -> key / endpoint
    pub providers: std::collections::BTreeMap<String, ProviderSettings>,
    /// model used by the AI hotkey (fast and cheap works well)
    pub item: ModelRef,
    /// model used for guide analysis (the most capable one)
    pub guide: ModelRef,
    /// models the cycle hotkey rotates the item model through
    pub cycle: Vec<ModelRef>,
    /// reasoning effort where supported: "low" | "medium" | "high"
    pub effort: String,
    /// cached model lists per provider (from "Fetch models"), with prices
    pub catalog: std::collections::BTreeMap<String, Vec<le_core::ai::ModelInfo>>,
}

impl Default for AiSettings {
    fn default() -> Self {
        let providers = ["anthropic", "openai", "kimi", "deepseek", "openai-compatible"]
            .iter()
            .map(|p| (p.to_string(), ProviderSettings::default()))
            .collect();
        let item = ModelRef::new("anthropic", "claude-sonnet-5");
        let guide = ModelRef::new("anthropic", "claude-opus-5");
        AiSettings {
            hotkey_ai: "Numpad1".into(),
            hotkey_cycle: "Numpad3".into(),
            providers,
            cycle: vec![item.clone(), guide.clone()],
            item,
            guide,
            effort: "medium".into(),
            catalog: Default::default(),
        }
    }
}

impl AiSettings {
    pub fn provider(&self, id: &str) -> ProviderSettings {
        self.providers.get(id).cloned().unwrap_or_default()
    }

    /// Runtime config for a model reference (key from the provider entry or the environment).
    pub fn config(&self, r: &ModelRef) -> ModelConfig {
        let p = self.provider(&r.provider);
        ModelConfig {
            provider: r.provider.clone(),
            model: r.model.clone(),
            api_key: p.api_key,
            base_url: p.base_url,
            vision: le_core::ai::supports_vision(&r.provider, &r.model),
            effort: self.effort.clone(),
        }
    }

    pub fn usable(&self, r: &ModelRef) -> bool {
        r.is_set() && self.config(r).has_key()
    }

    pub fn item_model(&self) -> Option<ModelConfig> {
        self.item.is_set().then(|| self.config(&self.item))
    }

    pub fn guide_model(&self) -> Option<ModelConfig> {
        self.guide.is_set().then(|| self.config(&self.guide))
    }

    /// Next usable entry of `cycle` after the current item model (wraps).
    /// None when fewer than two are usable.
    pub fn next_item_model(&self) -> Option<ModelRef> {
        let usable: Vec<&ModelRef> = self.cycle.iter().filter(|r| self.usable(r)).collect();
        if usable.len() < 2 {
            return None;
        }
        let pos = usable.iter().position(|r| **r == self.item).map(|p| (p + 1) % usable.len()).unwrap_or(0);
        Some(usable[pos].clone())
    }

    pub fn usable_cycle_len(&self) -> usize {
        self.cycle.iter().filter(|r| self.usable(r)).count()
    }

    /// Keep the item model in the cycle list and drop empty entries.
    pub fn normalise(&mut self) {
        self.cycle.retain(|r| r.is_set());
        self.cycle.dedup();
        if self.item.is_set() && !self.cycle.contains(&self.item) {
            self.cycle.insert(0, self.item.clone());
        }
        if self.effort.trim().is_empty() {
            self.effort = "medium".into();
        }
        for p in ["anthropic", "openai", "kimi", "deepseek", "openai-compatible"] {
            self.providers.entry(p.to_string()).or_default();
        }
    }
}

/// Settings written by the previous layout (`ai.models` rows with per-row keys)
/// are converted in place so nothing the user typed is lost.
fn migrate_ai(value: &mut serde_json::Value) {
    let Some(ai) = value.get_mut("ai").filter(|a| a.is_object()) else { return };
    if ai.get("providers").is_some() || ai.get("models").is_none() {
        return;
    }
    let models: Vec<ModelConfig> = serde_json::from_value(ai["models"].clone()).unwrap_or_default();
    let mut providers = serde_json::Map::new();
    for m in &models {
        let entry = providers.entry(m.provider.clone()).or_insert_with(|| serde_json::json!({ "api_key": "", "base_url": "" }));
        if !m.api_key.trim().is_empty() {
            entry["api_key"] = serde_json::json!(m.api_key);
        }
        if !m.base_url.trim().is_empty() {
            entry["base_url"] = serde_json::json!(m.base_url);
        }
    }
    let index = |key: &str| ai[key].as_u64().unwrap_or(0) as usize;
    let reference = |i: usize| models.get(i).map(|m| serde_json::json!({ "provider": m.provider, "model": m.model }));
    let mut migrated = serde_json::json!({
        "providers": providers,
        "cycle": models.iter().map(|m| serde_json::json!({ "provider": m.provider, "model": m.model })).collect::<Vec<_>>(),
        "effort": models.first().map(|m| m.effort.clone()).unwrap_or_else(|| "medium".into()),
    });
    if let Some(r) = reference(index("item_model")) {
        migrated["item"] = r;
    }
    if let Some(r) = reference(index("guide_model")) {
        migrated["guide"] = r;
    }
    for key in ["hotkey_ai", "hotkey_cycle"] {
        if let Some(v) = ai[key].as_str() {
            migrated[key] = serde_json::json!(v);
        }
    }
    *ai = migrated;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Global hotkey (Tauri shortcut syntax): "F8", "Numpad0", "Ctrl+Shift+Space"
    pub hotkey: String,
    /// Scan even when Last Epoch is not the foreground window (testing only)
    pub any_window: bool,
    /// Seconds the verdict card stays up; 0 = until the mouse moves (default)
    pub card_seconds: u64,
    /// Save each capture and its OCR lines under profile/debug/
    pub debug_captures: bool,
    /// "auto" | "windows" | "tesseract"
    pub ocr_engine: String,
    pub ai: AiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { hotkey: "F8".into(), any_window: false, card_seconds: 0, debug_captures: false, ocr_engine: "auto".into(), ai: AiSettings::default() }
    }
}

impl Settings {
    pub fn load(profile_dir: &Path) -> Settings {
        let path = profile_dir.join("settings.json");
        let mut settings: Settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|mut v| {
                migrate_ai(&mut v);
                serde_json::from_value(v).ok()
            })
            .unwrap_or_default();
        settings.ai.normalise();
        settings
    }

    pub fn save(&self, profile_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(profile_dir)?;
        std::fs::write(profile_dir.join("settings.json"), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn engine(&self) -> OcrEngineKind {
        match self.ocr_engine.as_str() {
            "windows" => OcrEngineKind::Windows,
            "tesseract" => OcrEngineKind::Tesseract,
            _ => OcrEngineKind::Auto,
        }
    }
}

/// The AI's answer, shown on the card next to the deterministic scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiInfo {
    pub model: String,
    pub summary: String,
    pub confidence: String,
    /// true when the answer came from the cache (same items, model, build and character)
    #[serde(default)]
    pub cached: bool,
}

/// One stored AI answer (`profile/ai_cache.json`), so the same hovered/equipped
/// pair never costs tokens twice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedVerdict {
    pub label: String,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub ai: AiInfo,
    /// unix seconds
    pub time: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AiCache {
    #[serde(default)]
    pub entries: HashMap<String, CachedVerdict>,
}

impl AiCache {
    const MAX_ENTRIES: usize = 400;

    pub fn load(path: &Path) -> AiCache {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_string(self)?)?;
        Ok(())
    }

    pub fn insert(&mut self, key: String, verdict: CachedVerdict) {
        self.entries.insert(key, verdict);
        if self.entries.len() > Self::MAX_ENTRIES {
            // drop the oldest quarter
            let mut times: Vec<u64> = self.entries.values().map(|v| v.time).collect();
            times.sort_unstable();
            let cutoff = times[times.len() / 4];
            self.entries.retain(|_, v| v.time > cutoff);
        }
    }
}

/// Stable identity of a parsed item: what it is and what it rolls, not the
/// OCR text (which varies slightly between captures).
fn item_identity(item: &ParsedItem) -> String {
    let mut parts: Vec<String> = vec![
        format!("u{:?}", item.unique_id),
        format!("t{:?}/{:?}", item.item_type, item.sub_type),
        format!("lvl{}", item.level_requirement),
    ];
    // implicits: keep the numbers only (e.g. "+280 Armor" -> "280")
    for line in &item.implicit_lines {
        let digits: String = line.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
        parts.push(format!("i{digits}"));
    }
    let mut affixes: Vec<String> = item
        .affixes
        .iter()
        .map(|a| format!("a{}:{}:{}", a.affix_id, a.tier, a.value.map(|v| (v * 10.0).round() as i64).unwrap_or(i64::MIN)))
        .collect();
    affixes.sort();
    parts.extend(affixes);
    parts.join("|")
}

/// Everything about the character the AI was told, so a changed sheet or fact
/// invalidates the cached answer.
fn state_identity(state: &CharacterState) -> String {
    let mut res: Vec<String> = state.resistances.iter().map(|(k, v)| format!("{k}={}", v.round())).collect();
    res.sort();
    let mut flags: Vec<String> = state.flags.iter().map(|(k, v)| format!("{k}={v}")).collect();
    flags.sort();
    let mut counters: Vec<String> = state.counters.iter().map(|(k, v)| format!("{k}={v}")).collect();
    counters.sort();
    format!(
        "L{} {} e{} h{} m{} [{}] [{}] [{}]",
        state.level, state.phase(), state.endurance.round(), state.health.round(), state.mana.round(), res.join(","), flags.join(","), counters.join(",")
    )
}

/// What the overlay renders.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub verdict: Verdict,
    /// logical pixels relative to the overlay window (filled in by lib.rs)
    pub x: f64,
    pub y: f64,
    /// absolute physical cursor position at capture time
    pub cursor: (i32, i32),
    pub seconds: u64,
    pub subtitle: String,
    pub equipped_from_tooltip: bool,
    pub ai: Option<AiInfo>,
    /// model being asked right now (the card shows a spinner until `ai` arrives)
    #[serde(default)]
    pub ai_pending: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub text: String,
    pub kind: &'static str,
}

/// `profile/` next to the exe, in an ancestor (dev: target/debug -> repo root),
/// or in the current directory. Created in the user's config dir otherwise.
pub fn find_profile_dir() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("profile"));
        candidates.push(cwd.join("..").join("..").join("profile"));
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..5 {
            if let Some(d) = &dir {
                candidates.push(d.join("profile"));
                dir = d.parent().map(Path::to_path_buf);
            }
        }
    }
    for c in &candidates {
        if c.join("character.json").exists() {
            return c.clone();
        }
    }
    let fallback = std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("LE Gear Advisor").join("profile"))
        .unwrap_or_else(|| PathBuf::from("profile"));
    let _ = std::fs::create_dir_all(&fallback);
    if !fallback.join("character.json").exists() {
        let _ = CharacterState::default().save(&fallback.join("character.json"));
    }
    fallback
}

pub struct Engine {
    pub profile_dir: PathBuf,
    pub settings: RwLock<Settings>,
    pub ai_cache: Mutex<AiCache>,
    pub data: &'static GameData,
    /// active build id + its profile
    guide: RwLock<(String, GuideProfile)>,
    pub library: BuildLibrary,
    /// runtime toggle (tray menu); seeded from settings.debug_captures
    pub debug_captures: std::sync::atomic::AtomicBool,
}

/// Everything the AI needs from a scan, kept only when the AI hotkey ran it.
pub struct AiInputs {
    /// items + build + character; the model is appended when judging
    pub cache_key: String,
    /// the game's green/red compare block under the hovered item
    pub game_diff: Vec<String>,
    pub candidate_png: Vec<u8>,
    pub equipped_png: Option<Vec<u8>>,
    pub candidate_text: String,
    pub equipped_text: Option<String>,
    pub state: CharacterState,
}

pub struct Scan {
    pub card: Card,
    pub monitor: capture::MonitorInfo,
    pub ai_inputs: Option<AiInputs>,
}

/// Outcome of a hotkey press: a verdict card, or the character sheet was read.
pub enum Outcome {
    Card(Scan),
    Sheet { summary: String },
}

/// Result of pasting a guide in the Builds window.
#[derive(Debug, Clone, Serialize)]
pub struct GuideOutcome {
    pub build: BuildSummary,
    /// true when the same guide text was analysed before and the stored profile was reused
    pub reused: bool,
    pub usage: String,
}

impl Engine {
    pub fn new() -> Engine {
        let profile_dir = find_profile_dir();
        let settings = Settings::load(&profile_dir);
        // persist a migrated layout right away so the CLI reads the same shape
        let on_disk = std::fs::read_to_string(profile_dir.join("settings.json")).unwrap_or_default();
        if !on_disk.contains("\"providers\"") {
            let _ = settings.save(&profile_dir);
        }
        let debug = std::sync::atomic::AtomicBool::new(settings.debug_captures);
        let library = BuildLibrary::new(&profile_dir);
        let (id, profile, _) = library.active_profile();
        let ai_cache = Mutex::new(AiCache::load(&profile_dir.join("ai_cache.json")));
        Engine {
            ai_cache,
            profile_dir,
            settings: RwLock::new(settings),
            data: GameData::embedded(),
            guide: RwLock::new((id, profile)),
            library,
            debug_captures: debug,
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut guard = self.settings.write().unwrap();
        f(&mut guard);
        guard.save(&self.profile_dir)?;
        Ok(guard.clone())
    }

    pub fn active_build_id(&self) -> String {
        self.guide.read().unwrap().0.clone()
    }

    fn ai_cache_path(&self) -> PathBuf {
        self.profile_dir.join("ai_cache.json")
    }

    pub fn clear_ai_cache(&self) -> usize {
        let mut cache = self.ai_cache.lock().unwrap();
        let n = cache.entries.len();
        cache.entries.clear();
        let _ = cache.save(&self.ai_cache_path());
        n
    }

    pub fn active_profile(&self) -> GuideProfile {
        self.guide.read().unwrap().1.clone()
    }

    /// Re-read the active build from disk (after activate/delete/analyse).
    pub fn reload_guide(&self) -> Option<String> {
        let (id, profile, err) = self.library.active_profile();
        *self.guide.write().unwrap() = (id, profile);
        err
    }

    /// Character state with the active profile's phase brackets and fact defaults applied.
    fn state(&self) -> CharacterState {
        let mut state = CharacterState::load(&self.profile_dir.join("character.json")).unwrap_or_default();
        self.active_profile().apply_to_state(&mut state);
        state
    }

    pub fn character_path(&self) -> PathBuf {
        self.profile_dir.join("character.json")
    }

    fn equipped_path(&self) -> PathBuf {
        self.profile_dir.join("equipped.json")
    }

    /// Full pipeline for one hotkey press. `for_ai` keeps the tooltip crops.
    pub fn scan(&self, for_ai: bool) -> Result<Outcome> {
        let settings = self.settings();
        if !settings.any_window && !capture::game_is_foreground() {
            return Err(anyhow!("Last Epoch is not the active window"));
        }
        let shot = capture::capture_around_cursor()?;
        let boxes = ocr::recognize_rgba(&shot.image, settings.engine(), None)?;
        let panels = ocr::panels(&boxes, Some(shot.cursor_in_crop));
        if self.debug_captures.load(std::sync::atomic::Ordering::Relaxed) {
            self.dump_debug(&shot, &panels);
        }
        // nearest panel (cursor first) that parses as an item; grid labels near the
        // cursor ("TRANSFER", "SORT") are skipped that way
        let mut candidate: Option<(ParsedItem, &Panel)> = None;
        for panel in panels.iter().filter(|p| !p.is_equipped_compare() && !p.is_clipped() && p.lines.len() >= 3) {
            let parsed = parse_tooltip(&panel.item_lines(), self.data);
            if parsed.recognised() {
                candidate = Some((parsed, panel));
                break;
            }
        }
        // the character sheet in the same capture (open next to the inventory)
        // refreshes character.json before anything is compared
        let mut sheet_changes: Option<Vec<String>> = None;
        if let Some(reading) = le_core::sheet_reader::read_sheet_from_image(&shot.image, &boxes, settings.engine()).filter(|r| r.is_useful()) {
            let path = self.character_path();
            let mut state = CharacterState::load(&path).unwrap_or_default();
            let changes = reading.apply(&mut state);
            state.save(&path)?;
            sheet_changes = Some(changes);
        }
        // no item tooltip: the sheet read is the whole result
        if candidate.is_none() {
            if let Some(changes) = sheet_changes {
                let summary = if changes.is_empty() {
                    "character sheet read: no changes".to_string()
                } else {
                    format!("character.json updated: {}", changes.join(", "))
                };
                return Ok(Outcome::Sheet { summary });
            }
        }
        // nothing recognisable near the cursor: say so instead of judging screen noise
        let (candidate, candidate_panel) = candidate.ok_or_else(|| anyhow!("no item tooltip under the cursor"))?;
        let equipped_panel: Option<&Panel> = panels.iter().find(|p| p.is_equipped_compare() && p.lines.len() >= 3);
        // a compare tooltip touching the capture's left edge is cut off
        let equipped_clipped = equipped_panel.map_or(false, |p| p.rect.0 <= 2);

        let state = self.state();
        // Only what is on screen counts: with Auto Compare the game shows the
        // worn item next to the hovered one, so a remembered item is never
        // used as a stand-in. equipped.json is still refreshed for the CLI.
        let mut on_screen = EquippedProfile::default();
        let mut from_tooltip = false;
        let mut equipped_text: Option<String> = None;
        let mut equipped_problem: Option<&str> = None;
        match equipped_panel {
            None => equipped_problem = Some("no EQUIPPED tooltip in the capture: nothing to compare against (Auto Compare shows the worn item next to the hovered one)"),
            Some(_) if equipped_clipped => equipped_problem = Some("the EQUIPPED tooltip was cut off at the capture edge; move the item tooltip further right and scan again"),
            Some(panel) => {
                let lines: Vec<String> = panel.item_lines().into_iter().skip_while(|l| l.trim().to_lowercase().replace(' ', "") == "equipped").collect();
                let parsed = parse_tooltip(&lines, self.data);
                equipped_text = Some(lines.join("
"));
                if parsed.recognised() {
                    let mut stored = EquippedProfile::load(&self.equipped_path()).unwrap_or_default();
                    if let Some(slot) = pick_equipped_slot(&parsed, &candidate, &stored) {
                        on_screen.set(slot, parsed.clone(), lines.join("
"));
                        stored.set(slot, parsed, lines.join("
"));
                        let _ = stored.save(&self.equipped_path());
                        from_tooltip = true;
                    } else {
                        equipped_problem = Some("the EQUIPPED tooltip's item has no slot");
                    }
                } else {
                    equipped_problem = Some("the EQUIPPED tooltip could not be read (OCR); scan again");
                }
            }
        }
        let profile = self.active_profile();
        let mut verdict = compare(&candidate, &on_screen, self.data, &profile, &state);
        if let Some(changes) = sheet_changes.filter(|c| !c.is_empty()) {
            verdict.warnings.push(format!("character sheet refreshed: {}", changes.join(", ")));
        }
        if let Some(problem) = equipped_problem {
            // a verdict against nothing is not a verdict: show the item's own score for review
            verdict.label = VerdictLabel::Review;
            verdict.equipped_name = None;
            verdict.equipped_score = 0.0;
            verdict.delta = 0.0;
            verdict.reasons.retain(|r| !r.to_lowercase().contains("empty"));
            verdict.warnings.retain(|w| !w.to_lowercase().contains("slot is empty"));
            verdict.warnings.insert(0, problem.to_string());
        }

        let ai_inputs = if for_ai {
            let equipped_item = verdict.slot.and_then(|s| on_screen.get(s));
            let cache_key = format!(
                "p2 || {} || {} || {} || {}",
                item_identity(&candidate),
                equipped_item.map(item_identity).unwrap_or_else(|| "empty".into()),
                self.active_build_id(),
                state_identity(&state)
            );
            Some(AiInputs {
                cache_key,
                game_diff: candidate_panel.compare_lines(),
                candidate_png: crop_png(&shot, candidate_panel.rect)?,
                equipped_png: equipped_panel.filter(|_| !equipped_clipped).map(|p| crop_png(&shot, p.rect)).transpose()?,
                candidate_text: candidate_panel.item_lines().join("\n"),
                equipped_text,
                state: state.clone(),
            })
        } else {
            None
        };

        // card position: next to the cursor, in PHYSICAL pixels; lib.rs places the window
        let card = Card {
            x: 0.0,
            y: 0.0,
            cursor: shot.cursor,
            seconds: settings.card_seconds,
            subtitle: subtitle(&candidate),
            equipped_from_tooltip: from_tooltip,
            verdict,
            ai: None,
            ai_pending: None,
        };
        Ok(Outcome::Card(Scan { card, monitor: shot.monitor, ai_inputs }))
    }

    /// Ask the item model about a scanned item; the card's label, reasons and
    /// warnings become the AI's, the deterministic scores stay for reference.
    /// Returns true when the answer came from the cache.
    pub fn ai_judge(&self, card: &mut Card, inputs: AiInputs, model: &ModelConfig) -> Result<bool> {
        let key = format!("{} || {}", inputs.cache_key, model.label());
        if let Some(hit) = self.ai_cache.lock().unwrap().entries.get(&key).cloned() {
            card.verdict.label = label_from(&hit.label);
            card.verdict.reasons = hit.reasons;
            card.verdict.warnings = hit.warnings;
            if card.seconds > 0 {
                card.seconds = card.seconds.max(6) + 2;
            }
            card.ai = Some(AiInfo { cached: true, ..hit.ai });
            return Ok(true);
        }
        let mut client = Client::new(model.clone())?;
        // an item verdict must not hang the overlay: cap the round trip
        client.timeout = std::time::Duration::from_secs(90);
        let profile = self.active_profile();
        let answer = ai::analyze_item(&client, ai::ItemRequest {
            candidate_png: inputs.candidate_png,
            equipped_png: inputs.equipped_png,
            candidate_text: inputs.candidate_text,
            equipped_text: inputs.equipped_text,
            game_diff: inputs.game_diff,
            deterministic: &card.verdict,
            profile: &profile,
            state: &inputs.state,
        })?;
        card.verdict.label = label_from(&answer.verdict);
        card.verdict.reasons = answer.reasons.clone();
        card.verdict.warnings = answer.warnings.clone();
        if card.seconds > 0 {
            card.seconds = card.seconds.max(6) + 2;
        }
        let info = AiInfo { model: model.model.clone(), summary: answer.summary, confidence: answer.confidence, cached: false };
        card.ai = Some(info.clone());
        let mut cache = self.ai_cache.lock().unwrap();
        cache.insert(key, CachedVerdict { label: answer.verdict, reasons: answer.reasons, warnings: answer.warnings, ai: info, time: BuildLibrary::now() });
        let _ = cache.save(&self.ai_cache_path());
        Ok(false)
    }

    /// Paste a guide: reuse the stored profile when the text is unchanged,
    /// otherwise analyse it with the guide model and store + activate the build.
    pub fn analyze_guide(&self, character_name: &str, build_name: &str, guide_text: &str, source: &str, force: bool) -> Result<GuideOutcome> {
        let character_name = character_name.trim();
        let build_name = build_name.trim();
        if character_name.is_empty() || build_name.is_empty() {
            return Err(anyhow!("character name and build name are required"));
        }
        let hash = BuildLibrary::hash(guide_text);
        let id = BuildLibrary::make_id(character_name, build_name);
        if id == BUILTIN_ID {
            return Err(anyhow!("that name is reserved"));
        }
        let existing = self.library.get(&id);
        if let Some(build) = existing.as_ref().filter(|b| b.guide_hash == hash && !force) {
            if build.profile().is_ok() {
                self.library.set_active(&id)?;
                self.reload_guide();
                let summary = self.summary_of(&id)?;
                return Ok(GuideOutcome { build: summary, reused: true, usage: "stored profile reused (guide unchanged)".into() });
            }
        }
        let settings = self.settings();
        let model = settings.ai.guide_model().ok_or_else(|| anyhow!("no guide model chosen (Builds & AI window > AI models)"))?;
        let client = Client::new(model.clone())?;
        let mut names: Vec<String> = self.data.affixes().map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).collect();
        names.sort();
        names.dedup();
        let analysis = ai::analyze_guide(&client, ai::GuideRequest {
            guide_text,
            character_name,
            build_name,
            template_json: guide_profile::embedded_json(),
            affix_names: &names,
        })?;
        let now = BuildLibrary::now();
        let build = Build {
            id: id.clone(),
            character_name: character_name.to_string(),
            build_name: build_name.to_string(),
            source: if source.trim().is_empty() { "pasted guide".into() } else { source.trim().to_string() },
            guide_hash: hash,
            guide_text: guide_text.to_string(),
            created: existing.as_ref().map(|b| b.created).unwrap_or(now),
            updated: now,
            model: model.label(),
            profile_json: analysis.profile_json,
            notes: format!("{} stats, {} facts", analysis.profile.stats.len(), analysis.profile.facts.len()),
        };
        self.library.save(&build)?;
        self.library.set_active(&id)?;
        self.reload_guide();
        // seed the new build's facts with their defaults so the user sees them
        let path = self.character_path();
        let mut state = CharacterState::load(&path).unwrap_or_default();
        analysis.profile.apply_to_state(&mut state);
        let _ = state.save(&path);
        let summary = self.summary_of(&id)?;
        Ok(GuideOutcome { build: summary, reused: false, usage: analysis.usage })
    }

    fn summary_of(&self, id: &str) -> Result<BuildSummary> {
        self.library.list().into_iter().find(|b| b.id == id).ok_or_else(|| anyhow!("build {id} vanished"))
    }

    fn dump_debug(&self, shot: &Capture, panels: &[Panel]) {
        let dir = self.profile_dir.join("debug");
        let _ = std::fs::create_dir_all(&dir);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = shot.image.save(dir.join(format!("{stamp}_capture.png")));
        let mut text = String::new();
        for (i, p) in panels.iter().enumerate() {
            text.push_str(&format!("## panel {i} rect={:?} distance={:.0}{}\n", p.rect, p.distance, if p.is_equipped_compare() { " EQUIPPED" } else { "" }));
            for (i, l) in p.lines.iter().enumerate() {
                match p.tints.get(i) {
                    Some(ocr::Tint::Green) => text.push_str("[green] "),
                    Some(ocr::Tint::Red) => text.push_str("[red] "),
                    _ => {}
                }
                text.push_str(l);
                text.push('\n');
            }
            text.push('\n');
        }
        let _ = std::fs::write(dir.join(format!("{stamp}_panels.txt")), text);
    }
}

fn label_from(verdict: &str) -> VerdictLabel {
    match verdict {
        "UPGRADE" => VerdictLabel::Upgrade,
        "SIDEGRADE" => VerdictLabel::Sidegrade,
        "WORSE" => VerdictLabel::Worse,
        _ => VerdictLabel::Review,
    }
}

/// PNG of one panel's area (with a margin) for the AI.
fn crop_png(shot: &Capture, rect: (i32, i32, i32, i32)) -> Result<Vec<u8>> {
    let (iw, ih) = shot.image.dimensions();
    let margin = 12;
    let x0 = (rect.0 - margin).max(0) as u32;
    let y0 = (rect.1 - margin).max(0) as u32;
    let x1 = ((rect.0 + rect.2 + margin) as u32).min(iw);
    let y1 = ((rect.1 + rect.3 + margin) as u32).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return Err(anyhow!("empty tooltip crop"));
    }
    let crop = image::imageops::crop_imm(&shot.image, x0, y0, x1 - x0, y1 - y0).to_image();
    let mut bytes = std::io::Cursor::new(Vec::new());
    crop.write_to(&mut bytes, image::ImageFormat::Png).context("encoding tooltip crop")?;
    Ok(bytes.into_inner())
}

/// Which slot the "EQUIPPED" tooltip describes. Rings: the stored ring that
/// matches the compare tooltip's item keeps its slot; otherwise ring1.
fn pick_equipped_slot(parsed: &ParsedItem, candidate: &ParsedItem, equipped: &EquippedProfile) -> Option<le_core::game_data::Slot> {
    let slots = parsed.slots();
    if slots.is_empty() {
        return None;
    }
    if slots.len() == 1 {
        return Some(slots[0]);
    }
    let _ = candidate;
    for slot in slots {
        if let Some(existing) = equipped.get(*slot) {
            if existing.display_name() == parsed.display_name()
                && existing.affixes.iter().map(|a| a.affix_id).collect::<Vec<_>>() == parsed.affixes.iter().map(|a| a.affix_id).collect::<Vec<_>>()
            {
                return Some(*slot);
            }
        }
    }
    slots.iter().copied().find(|s| equipped.get(*s).is_none()).or(Some(slots[0]))
}

fn subtitle(item: &ParsedItem) -> String {
    if !item.recognised() {
        return "not recognised".into();
    }
    let mut s = format!("{} {}", item.rarity(), item.type_name);
    if item.level_requirement > 0 {
        s.push_str(&format!(" · level {}", item.level_requirement));
    }
    s
}
