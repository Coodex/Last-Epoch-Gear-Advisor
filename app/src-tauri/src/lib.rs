//! LE Gear Advisor overlay: a transparent, always-on-top, click-through window
//! that stays empty until a hotkey is pressed over a bag item in Last Epoch.
//! A second, normal window ("builds") holds the Builds & AI settings.

mod capture;
mod engine;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use engine::{AiSettings, Engine, Outcome, Settings, Status};
#[allow(unused_imports)]
use engine::ModelRef;
use le_core::build_library::BuildSummary;
use le_core::character_state::CharacterState;

struct AppState {
    engine: Arc<Engine>,
    scanning: Arc<Mutex<bool>>,
    /// bumped per card so an older mouse watcher stops when a new card appears
    card_generation: Arc<std::sync::atomic::AtomicU64>,
    /// hotkey scans enabled (tray "Enabled")
    enabled: Arc<AtomicBool>,
    /// tray label showing the item-analysis model
    model_label: Mutex<Option<MenuItem<tauri::Wry>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanMode {
    Normal,
    Ai,
}

fn status(app: &AppHandle, text: impl Into<String>, kind: &'static str) {
    let text = text.into();
    log(app, &format!("status[{kind}]: {text}"));
    let _ = app.emit("status", Status { text, kind });
}

/// Append a line to profile/overlay.log (diagnostics without a console).
fn log(app: &AppHandle, line: &str) {
    use std::io::Write;
    let dir = app.state::<AppState>().engine.profile_dir.clone();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("overlay.log")) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{now} {line}");
    }
}

/// Logical size of the overlay window (the card lives at its top-left).
/// AI cards carry a summary box and more reasons, so they get a taller window.
const WINDOW_W: f64 = 460.0;
const WINDOW_H: f64 = 340.0;
const WINDOW_H_AI: f64 = 560.0;

/// Put the small overlay window next to a physical screen point, kept inside
/// the monitor. No scale conversion: window positions are physical pixels too.
fn place_window(app: &AppHandle, point: (i32, i32), monitor: &capture::MonitorInfo) {
    place_window_sized(app, point, monitor, WINDOW_H);
}

fn place_window_sized(app: &AppHandle, point: (i32, i32), monitor: &capture::MonitorInfo, height: f64) {
    if let Some(window) = app.get_webview_window("main") {
        let scale = window.scale_factor().unwrap_or(1.0).max(0.5);
        let w = (WINDOW_W * scale) as i32;
        let h = (height * scale) as i32;
        let max_x = monitor.x + monitor.width as i32 - w;
        let max_y = monitor.y + monitor.height as i32 - h;
        let x = (point.0 + 28).min(max_x).max(monitor.x);
        let y = (point.1 - 24).min(max_y).max(monitor.y);
        let _ = window.set_size(tauri::LogicalSize::new(WINDOW_W, height));
        let _ = window.set_position(PhysicalPosition::new(x, y));
        let _ = window.set_ignore_cursor_events(true);
        let _ = window.set_always_on_top(true);
        if !window.is_visible().unwrap_or(false) {
            let _ = window.show();
        }
    }
}

/// Park the window in the monitor's bottom-right corner (for status toasts).
fn park_window(app: &AppHandle, monitor: &capture::MonitorInfo) {
    let corner = (monitor.x + monitor.width as i32, monitor.y + monitor.height as i32);
    place_window(app, (corner.0 - 28, corner.1 + 24), monitor);
}

fn run_scan(app: AppHandle, mode: ScanMode) {
    let (engine, scanning, enabled) = {
        let state = app.state::<AppState>();
        (state.engine.clone(), state.scanning.clone(), state.enabled.load(Ordering::Relaxed))
    };
    if !enabled {
        status(&app, "LE Gear Advisor is paused (tray menu)", "info");
        return;
    }
    {
        let mut busy = scanning.lock().unwrap();
        if *busy {
            return;
        }
        *busy = true;
    }
    std::thread::spawn(move || {
        let started = Instant::now();
        log(&app, &format!("scan requested ({})", if mode == ScanMode::Ai { "AI" } else { "normal" }));
        match engine.scan(mode == ScanMode::Ai) {
            Ok(Outcome::Sheet { summary }) => {
                log(&app, &summary);
                if let Ok((x, y)) = capture::cursor_pos() {
                    if let Ok((_, info)) = capture::monitor_at(x, y) {
                        place_window(&app, (x, y), &info);
                    }
                }
                let _ = app.emit("verdict", &engine::Card {
                    verdict: le_core::compare::Verdict {
                        label: le_core::compare::VerdictLabel::Review, slot: None, candidate_name: "Character sheet".into(),
                        candidate_score: 0.0, equipped_name: None, equipped_score: 0.0, delta: 0.0,
                        reasons: vec![summary], warnings: vec![], candidate: Default::default(), equipped: None,
                    },
                    x: 0.0, y: 0.0, cursor: (0, 0), seconds: match engine.settings().card_seconds { 0 => 0, s => s + 2 },
                    subtitle: "resistances and level refreshed from the sheet".into(), equipped_from_tooltip: false, ai: None,
                });
                watch_mouse_for_dismiss(app.clone(), match engine.settings().card_seconds { 0 => 0, s => s + 2 });
            }
            Ok(Outcome::Card(scan)) => {
                let mut card = scan.card;
                log(&app, &format!("scan ok: {} {} cursor {:?} ({} ms)", card.verdict.label.as_str(), card.verdict.candidate_name, card.cursor, started.elapsed().as_millis()));
                place_window(&app, card.cursor, &scan.monitor);
                let _ = app.emit("verdict", &card);
                match (mode, scan.ai_inputs) {
                    (ScanMode::Ai, Some(inputs)) => {
                        let settings = engine.settings();
                        match settings.ai.item_model() {
                            None => status(&app, "no AI model configured (tray > Builds & AI)", "error"),
                            Some(model) => {
                                if !engine.ai_cache.lock().unwrap().entries.contains_key(&format!("{} || {}", inputs.cache_key, model.label())) {
                                    status(&app, format!("asking {}…", model.label()), "info");
                                }
                                let ai_started = Instant::now();
                                match engine.ai_judge(&mut card, inputs, &model) {
                                    Ok(cached) => {
                                        log(&app, &format!("AI {}: {} ({} ms{})", model.label(), card.verdict.label.as_str(), ai_started.elapsed().as_millis(), if cached { ", cached" } else { "" }));
                                        place_window_sized(&app, card.cursor, &scan.monitor, WINDOW_H_AI);
                                        let _ = app.emit("verdict", &card);
                                        if cached {
                                            status(&app, "same items as before: cached AI verdict, no tokens spent", "info");
                                        } else {
                                            status(&app, format!("AI answered in {:.1} s", ai_started.elapsed().as_secs_f64()), "info");
                                        }
                                    }
                                    Err(err) => {
                                        log(&app, &format!("AI error: {err:#}"));
                                        status(&app, format!("AI failed: {err}"), "error");
                                    }
                                }
                            }
                        }
                    }
                    _ => status(&app, format!("scan {} ms", started.elapsed().as_millis()), "info"),
                }
                watch_mouse_for_dismiss(app.clone(), card.seconds);
            }
            Err(err) => {
                status(&app, err.to_string(), "error");
            }
        }
        *scanning.lock().unwrap() = false;
    });
}

/// Dismiss the card when the mouse moves away. With `seconds` = 0 the card
/// has no timer and stays until the mouse moves (30 min safety cap).
fn watch_mouse_for_dismiss(app: AppHandle, seconds: u64) {
    let generation = app.state::<AppState>().card_generation.clone();
    let mine = generation.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        let Ok(origin) = capture::cursor_pos() else { return };
        let deadline = Instant::now() + Duration::from_secs(if seconds == 0 { 1800 } else { seconds });
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(60));
            if generation.load(Ordering::Relaxed) != mine {
                return; // a newer card has its own watcher
            }
            if let Ok((x, y)) = capture::cursor_pos() {
                let moved = ((x - origin.0).pow(2) + (y - origin.1).pow(2)) as f64;
                if moved.sqrt() > 40.0 {
                    let _ = app.emit("dismiss", ());
                    return;
                }
            }
        }
    });
}

/// Numpad3: next item-analysis model that has a key.
fn cycle_model(app: &AppHandle) {
    let engine = app.state::<AppState>().engine.clone();
    let settings = engine.settings();
    match settings.ai.next_item_model() {
        None => {
            let usable = settings.ai.usable_cycle_len();
            status(app, if usable == 0 { "no usable AI model: add a provider key in Builds & AI".to_string() } else { format!("only one model in the cycle list has a key: {}", settings.ai.item_model().map(|m| m.label()).unwrap_or_default()) }, "info");
        }
        Some(next) => {
            let updated = engine.update_settings(|s| s.ai.item = next.clone());
            match updated {
                Ok(s) => {
                    let label = s.ai.item_model().map(|m| m.label()).unwrap_or_default();
                    let usable: Vec<_> = s.ai.cycle.iter().filter(|r| s.ai.usable(r)).collect();
                    let pos = usable.iter().position(|r| **r == next).map(|p| p + 1).unwrap_or(0);
                    status(app, format!("AI item model: {label} ({pos}/{})", usable.len()), "info");
                    refresh_model_label(app);
                    let _ = app.emit("settings-changed", &s);
                }
                Err(err) => status(app, format!("could not save settings: {err}"), "error"),
            }
        }
    }
}

fn refresh_model_label(app: &AppHandle) {
    let state = app.state::<AppState>();
    let label = state.engine.settings().ai.item_model().map(|m| m.label()).unwrap_or_else(|| "none".into());
    let guard = state.model_label.lock().unwrap();
    if let Some(item) = guard.as_ref() {
        let _ = item.set_text(format!("AI item model: {label}"));
    }
    drop(guard);
}

fn open_builds(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("builds") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = app.emit_to("builds", "builds-opened", ());
    }
}

// ---------------------------------------------------------------------------
// commands (Builds & AI window)
// ---------------------------------------------------------------------------

#[tauri::command]
fn scan_now(app: AppHandle) {
    run_scan(app, ScanMode::Normal);
}

#[tauri::command]
fn settings(app: AppHandle) -> Settings {
    app.state::<AppState>().engine.settings()
}

#[tauri::command]
fn save_ai_settings(app: AppHandle, ai: AiSettings) -> Result<Settings, String> {
    let engine = app.state::<AppState>().engine.clone();
    let mut ai = ai;
    ai.normalise();
    let updated = engine.update_settings(|s| s.ai = ai).map_err(|e| e.to_string())?;
    refresh_model_label(&app);
    Ok(updated)
}

#[tauri::command]
fn list_builds(app: AppHandle) -> Vec<BuildSummary> {
    app.state::<AppState>().engine.library.list()
}

#[tauri::command]
fn activate_build(app: AppHandle, id: String) -> Result<Vec<BuildSummary>, String> {
    let engine = app.state::<AppState>().engine.clone();
    engine.library.set_active(&id).map_err(|e| e.to_string())?;
    if let Some(err) = engine.reload_guide() {
        return Err(err);
    }
    // seed the build's facts in character.json so the window can show them
    let path = engine.character_path();
    let mut character = CharacterState::load(&path).unwrap_or_default();
    engine.active_profile().apply_to_state(&mut character);
    let _ = character.save(&path);
    status(&app, format!("active build: {}", engine.active_profile().name), "info");
    Ok(engine.library.list())
}

#[tauri::command]
fn delete_build(app: AppHandle, id: String) -> Result<Vec<BuildSummary>, String> {
    let engine = app.state::<AppState>().engine.clone();
    engine.library.delete(&id).map_err(|e| e.to_string())?;
    engine.reload_guide();
    Ok(engine.library.list())
}

#[tauri::command]
fn get_build_profile(app: AppHandle, id: String) -> Result<String, String> {
    app.state::<AppState>().engine.library.get(&id).map(|b| b.profile_json).ok_or_else(|| format!("no build {id}"))
}

#[tauri::command]
async fn analyze_guide(
    app: AppHandle,
    character_name: String,
    build_name: String,
    guide_text: String,
    source: String,
    force: bool,
) -> Result<engine::GuideOutcome, String> {
    let engine = app.state::<AppState>().engine.clone();
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        engine.analyze_guide(&character_name, &build_name, &guide_text, &source, force)
    })
    .await
    .map_err(|e| e.to_string())?;
    match result {
        Ok(outcome) => {
            log(&handle, &format!("guide analysed: {} ({}; {})", outcome.build.build_name, if outcome.reused { "reused" } else { "new" }, outcome.usage));
            status(&handle, format!("active build: {}", outcome.build.profile_name), "info");
            Ok(outcome)
        }
        Err(err) => {
            log(&handle, &format!("guide analysis failed: {err:#}"));
            Err(format!("{err:#}"))
        }
    }
}

/// The models a provider lists for the key in `config` (unsaved edits allowed), with prices.
#[tauri::command]
async fn list_models(app: AppHandle, config: le_core::ai::ModelConfig) -> Result<Vec<le_core::ai::ModelInfo>, String> {
    let engine = app.state::<AppState>().engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let prices = le_core::ai::PriceTable::load(Some(&engine.profile_dir));
        let models = le_core::ai::list_models(&config, &prices).map_err(|e| format!("{e:#}"))?;
        let provider = config.provider.clone();
        let cached = models.clone();
        let _ = engine.update_settings(|s| {
            s.ai.catalog.insert(provider, cached);
        });
        Ok(models)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Which providers have a key in the environment (the window shows "env key").
#[tauri::command]
fn env_keys() -> std::collections::BTreeMap<String, bool> {
    ["anthropic", "openai", "kimi", "deepseek", "openai-compatible"]
        .iter()
        .map(|p| {
            let cfg = le_core::ai::ModelConfig { provider: p.to_string(), ..Default::default() };
            (p.to_string(), std::env::var(cfg.env_var()).map(|v| !v.trim().is_empty()).unwrap_or(false))
        })
        .collect()
}

/// A one-line round trip to check a model's key and endpoint.
#[tauri::command]
async fn test_model(config: le_core::ai::ModelConfig) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let model = config;
        let client = le_core::ai::Client::new(model.clone()).map_err(|e| e.to_string())?;
        let started = Instant::now();
        let turn = le_core::ai::Turn { role: "user", parts: vec![le_core::ai::Part::Text("Reply with the single word OK.".into())] };
        let answer = client.complete("You are a connectivity test.", &[turn], 64, None).map_err(|e| format!("{e:#}"))?;
        Ok(format!("{} replied \"{}\" in {:.1} s ({})", model.label(), answer.text.trim().chars().take(40).collect::<String>(), started.elapsed().as_secs_f64(), answer.usage))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_character(app: AppHandle) -> CharacterState {
    let engine = app.state::<AppState>().engine.clone();
    let mut character = CharacterState::load(&engine.character_path()).unwrap_or_default();
    engine.active_profile().apply_to_state(&mut character);
    character
}

#[tauri::command]
fn save_character(app: AppHandle, character: CharacterState) -> Result<CharacterState, String> {
    let engine = app.state::<AppState>().engine.clone();
    character.save(&engine.character_path()).map_err(|e| e.to_string())?;
    Ok(character)
}

#[tauri::command]
fn active_profile_name(app: AppHandle) -> String {
    app.state::<AppState>().engine.active_profile().name
}

#[tauri::command]
fn profile_dir(app: AppHandle) -> String {
    app.state::<AppState>().engine.profile_dir.display().to_string()
}

// ---------------------------------------------------------------------------

fn build_tray(app: &AppHandle, engine: &Engine, enabled: Arc<AtomicBool>) -> tauri::Result<()> {
    let settings = engine.settings();
    let scan = MenuItem::with_id(app, "scan", "Scan now", true, None::<&str>)?;
    let hotkey_label = MenuItem::with_id(app, "hotkey", format!("Hotkeys: {} scan · {} AI · {} cycle model", settings.hotkey, settings.ai.hotkey_ai, settings.ai.hotkey_cycle), false, None::<&str>)?;
    let model_label = MenuItem::with_id(app, "model", format!("AI item model: {}", settings.ai.item_model().map(|m| m.label()).unwrap_or_else(|| "none".into())), false, None::<&str>)?;
    let builds = MenuItem::with_id(app, "builds", "Builds & AI…", true, None::<&str>)?;
    let enabled_item = CheckMenuItem::with_id(app, "enabled", "Enabled", true, true, None::<&str>)?;
    let debug_item = CheckMenuItem::with_id(app, "debug", "Save debug captures", true,
                                            engine.debug_captures.load(Ordering::Relaxed), None::<&str>)?;
    let clear_cache = MenuItem::with_id(app, "clear_cache", "Clear AI verdict cache", true, None::<&str>)?;
    let profile = MenuItem::with_id(app, "profile", "Open profile folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[
        &scan, &builds, &PredefinedMenuItem::separator(app)?,
        &hotkey_label, &model_label, &PredefinedMenuItem::separator(app)?,
        &enabled_item, &debug_item, &PredefinedMenuItem::separator(app)?,
        &clear_cache, &profile, &quit,
    ])?;
    *app.state::<AppState>().model_label.lock().unwrap() = Some(model_label);
    let enabled_handle = enabled_item.clone();
    let debug_handle = debug_item.clone();
    let profile_dir = engine.profile_dir.clone();
    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("LE Gear Advisor")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "scan" => run_scan(app.clone(), ScanMode::Normal),
            "builds" => open_builds(app),
            "enabled" => {
                let on = enabled_handle.is_checked().unwrap_or(true);
                enabled.store(on, Ordering::Relaxed);
                status(app, if on { "LE Gear Advisor enabled" } else { "LE Gear Advisor paused" }, "info");
            }
            "debug" => {
                let on = debug_handle.is_checked().unwrap_or(false);
                app.state::<AppState>().engine.debug_captures.store(on, Ordering::Relaxed);
                status(app, if on { "debug captures on (profile/debug)" } else { "debug captures off" }, "info");
            }
            "clear_cache" => {
                let n = app.state::<AppState>().engine.clear_ai_cache();
                status(app, format!("AI verdict cache cleared ({n} entries)"), "info");
            }
            "profile" => {
                let _ = std::process::Command::new("explorer").arg(&profile_dir).spawn();
            }
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn register_hotkey(app: &AppHandle, hotkey: &str, what: &'static str, mode: Option<ScanMode>) -> Result<(), String> {
    let hotkey = hotkey.trim().to_string();
    if hotkey.is_empty() {
        return Ok(());
    }
    let result = app.global_shortcut().on_shortcut(hotkey.as_str(), move |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            match mode {
                Some(mode) => run_scan(app.clone(), mode),
                None => cycle_model(app),
            }
        }
    });
    result.map_err(|err| format!("hotkey {hotkey} ({what}) could not be registered: {err}"))
}

pub fn run() {
    let engine = Arc::new(Engine::new());
    let scanning = Arc::new(Mutex::new(false));
    let enabled = Arc::new(AtomicBool::new(true));

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState { engine: engine.clone(), scanning, enabled: enabled.clone(), model_label: Mutex::new(None), card_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)) })
        .invoke_handler(tauri::generate_handler![
            scan_now, settings, save_ai_settings, list_builds, activate_build, delete_build, get_build_profile,
            analyze_guide, test_model, list_models, env_keys, get_character, save_character, active_profile_name, profile_dir
        ])
        .on_window_event(|window, event| {
            // the Builds window hides instead of closing so it can be reopened from the tray
            if window.label() == "builds" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            build_tray(&handle, &engine, enabled.clone())?;
            // park in the corner of the monitor under the cursor; click-through from the start
            if let Ok((x, y)) = capture::cursor_pos() {
                if let Ok((_, info)) = capture::monitor_at(x, y) {
                    park_window(&handle, &info);
                }
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_ignore_cursor_events(true);
            }
            let settings = engine.settings();
            log(&handle, &format!("startup: profile {} build {} hotkeys {} / {} / {}", engine.profile_dir.display(), engine.active_build_id(), settings.hotkey, settings.ai.hotkey_ai, settings.ai.hotkey_cycle));
            if let Some(err) = engine.reload_guide() {
                log(&handle, &err);
            }
            let mut problems: Vec<String> = Vec::new();
            for (key, what, mode) in [
                (settings.hotkey.as_str(), "scan", Some(ScanMode::Normal)),
                (settings.ai.hotkey_ai.as_str(), "AI", Some(ScanMode::Ai)),
                (settings.ai.hotkey_cycle.as_str(), "cycle model", None),
            ] {
                if let Err(err) = register_hotkey(&handle, key, what, mode) {
                    problems.push(err);
                }
            }
            let text = if problems.is_empty() {
                format!("LE Gear Advisor ready · {} scan · {} AI · {} cycle model", settings.hotkey, settings.ai.hotkey_ai, settings.ai.hotkey_cycle)
            } else {
                problems.join("; ")
            };
            let kind = if problems.is_empty() { "info" } else { "error" };
            // `le-gear-advisor --builds` opens the Builds & AI window right away
            if std::env::args().any(|a| a == "--builds") {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(600));
                    open_builds(&h);
                });
            }
            let h = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(800));
                status(&h, text, kind);
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running LE Gear Advisor");
}
