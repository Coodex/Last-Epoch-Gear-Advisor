//! `le-advisor`: validate parsing and scoring on real tooltips before any overlay work.
//!
//!   le-advisor parse    <tooltip.txt|image.png>          parse and print an item
//!   le-advisor equip    <tooltip.txt|image.png> [--slot ring2]   store as equipped
//!   le-advisor equipped                                  list the equipped profile
//!   le-advisor unequip  <slot>
//!   le-advisor compare  <tooltip.txt|image.png>          verdict vs equipped
//!   le-advisor weak-slots                                equipped slots, weakest first
//!   le-advisor character                                 state + which guide rules are active
//!   le-advisor ocr      <image.png> [--engine windows|tesseract]
//!   le-advisor guide                                     the guide's gear plan
//!   le-advisor update-guide [url-or-planner-id]          refresh the planner JSON
//!   le-advisor builds                                    saved builds (profile/builds), active one marked
//!   le-advisor use-build <id>                            activate a saved build
//!   le-advisor analyze-guide <guide.txt> --character X --build Y   AI: guide -> stat profile
//!   le-advisor ai <image.png>                            AI verdict for the tooltip in a screenshot

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use le_core::ai::{self, ModelConfig};
use le_core::build_library::{Build, BuildLibrary};
use le_core::character_state::{CharacterState, EquippedProfile};
use le_core::compare::{compare, weak_slots, VerdictLabel};
use le_core::game_data::{GameData, Slot};
use le_core::guide_profile::GuideProfile;
use le_core::item_parser::{parse_tooltip, ParsedItem};
use le_core::ocr::{self, OcrEngineKind};
use le_core::planner::GearPlan;
use le_core::scorer::{score_item, ScoreContext};

#[derive(Parser)]
#[command(name = "le-advisor", version, about = "Last Epoch gear advisor: judges bag items against your build guide's stat priorities")]
struct Cli {
    /// Directory holding character.json and equipped.json
    #[arg(long, default_value = "profile", global = true)]
    profile_dir: PathBuf,
    /// Guide profile JSON (weights + conditions); default: the active build in profile/builds
    /// (the built-in example profile, Maxroll Paladin leveling, when none is active)
    #[arg(long, global = true)]
    guide_profile: Option<PathBuf>,
    /// Print JSON instead of text
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum EngineArg {
    Auto,
    Windows,
    Tesseract,
}

impl From<EngineArg> for OcrEngineKind {
    fn from(value: EngineArg) -> Self {
        match value {
            EngineArg::Auto => OcrEngineKind::Auto,
            EngineArg::Windows => OcrEngineKind::Windows,
            EngineArg::Tesseract => OcrEngineKind::Tesseract,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Parse a tooltip (text file, "-" for stdin, or a screenshot) and print the item
    Parse {
        input: String,
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
    },
    /// Store a tooltip as the equipped item of its slot
    Equip {
        input: String,
        /// Override the slot (needed for rings: ring1 / ring2)
        #[arg(long)]
        slot: Option<String>,
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
    },
    /// List equipped items
    Equipped,
    /// Remove an equipped item
    Unequip { slot: String },
    /// Compare a tooltip against the equipped item of its slot
    Compare {
        input: String,
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
    },
    /// Equipped slots scored against the guide, weakest first
    WeakSlots,
    /// Show character.json and which guide rules are active
    Character,
    /// OCR an image and print the panels found
    Ocr {
        image: PathBuf,
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
    },
    /// Read level and resistances from a character-sheet screenshot
    Sheet {
        image: PathBuf,
        /// Write the values into character.json
        #[arg(long)]
        apply: bool,
    },
    /// Print the guide's gear plan per level bracket
    Guide,
    /// Refresh the planner JSON from Maxroll (guide URL or planner id)
    UpdateGuide {
        #[arg(default_value = "https://maxroll.gg/last-epoch/build-guides/paladin-leveling-guide")]
        source: String,
    },
    /// List saved builds (profile/builds); the active one is marked
    Builds,
    /// Activate a saved build (its profile drives compare/weak-slots/character)
    UseBuild { id: String },
    /// AI: turn a pasted guide (text file or "-" for stdin) into a stat profile and save it as a build
    AnalyzeGuide {
        input: String,
        #[arg(long)]
        character: String,
        #[arg(long)]
        build: String,
        /// URL the guide came from (stored with the build)
        #[arg(long, default_value = "")]
        source: String,
        /// Override the guide model as provider/model-id (default: settings.json `ai.guide`)
        #[arg(long)]
        model: Option<String>,
        /// Re-analyse even when the text is unchanged
        #[arg(long)]
        force: bool,
    },
    /// AI: list the models a provider offers for its key (settings.json `ai.providers`), with prices
    Models {
        /// Provider id: anthropic | openai | kimi | deepseek | openai-compatible
        #[arg(long, default_value = "anthropic")]
        provider: String,
    },
    /// AI: judge the tooltip in a screenshot with the item model (settings.json `ai.item`)
    Ai {
        image: PathBuf,
        /// Override the item model as provider/model-id (default: settings.json `ai.item`)
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_enum, default_value = "auto")]
        engine: EngineArg,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data = GameData::embedded();
    let library = BuildLibrary::new(&cli.profile_dir);
    let profile = match &cli.guide_profile {
        Some(path) => GuideProfile::from_file(path).with_context(|| format!("loading {}", path.display()))?,
        None => {
            let (_, profile, problem) = library.active_profile();
            if let Some(problem) = problem {
                eprintln!("warning: {problem}");
            }
            profile
        }
    };
    let character_path = cli.profile_dir.join("character.json");
    let equipped_path = cli.profile_dir.join("equipped.json");
    let mut state = if character_path.exists() {
        CharacterState::load(&character_path).with_context(|| format!("loading {}", character_path.display()))?
    } else {
        CharacterState::default()
    };
    profile.apply_to_state(&mut state);

    match cli.command {
        Command::Parse { input, engine } => {
            let (item, _) = load_item(&input, engine.into(), data)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&item)?);
            } else {
                print_item(&item, data);
            }
        }
        Command::Equip { input, slot, engine } => {
            let (item, text) = load_item(&input, engine.into(), data)?;
            if !item.recognised() {
                anyhow::bail!("item not recognised: {}", item.warnings.join("; "));
            }
            let slot = match slot {
                Some(s) => Slot::parse(&s).ok_or_else(|| anyhow::anyhow!("unknown slot {s:?}"))?,
                None => {
                    let slots = item.slots();
                    if slots.len() != 1 {
                        anyhow::bail!("ambiguous slot for a {}; pass --slot ring1 or --slot ring2", item.type_name);
                    }
                    slots[0]
                }
            };
            let mut equipped = EquippedProfile::load(&equipped_path)?;
            equipped.set(slot, item.clone(), text);
            equipped.save(&equipped_path)?;
            println!("equipped {}: {} ({}, {} affixes)", slot.label(), item.display_name(), item.rarity(), item.affixes.len());
        }
        Command::Equipped => {
            let equipped = EquippedProfile::load(&equipped_path)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&equipped)?);
            } else {
                for slot in Slot::ALL {
                    match equipped.get(slot) {
                        Some(item) => {
                            let ctx = ScoreContext { data, profile: &profile, state: &state, replaced: Some(item) };
                            let score = score_item(item, &ctx);
                            println!("{:<10} {:<32} score {:.2}  [{}]", slot.label(), item.display_name(), score.total,
                                     item.affixes.iter().map(|a| format!("{} T{}", a.name, a.tier)).collect::<Vec<_>>().join(", "));
                        }
                        None => println!("{:<10} (empty)", slot.label()),
                    }
                }
            }
        }
        Command::Unequip { slot } => {
            let slot = Slot::parse(&slot).ok_or_else(|| anyhow::anyhow!("unknown slot {slot:?}"))?;
            let mut equipped = EquippedProfile::load(&equipped_path)?;
            if equipped.remove(slot) {
                equipped.save(&equipped_path)?;
                println!("removed {}", slot.label());
            } else {
                println!("{} was empty", slot.label());
            }
        }
        Command::Compare { input, engine } => {
            let (item, _) = load_item(&input, engine.into(), data)?;
            let equipped = EquippedProfile::load(&equipped_path)?;
            let verdict = compare(&item, &equipped, data, &profile, &state);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&verdict)?);
            } else {
                print_verdict(&verdict, &item);
            }
        }
        Command::WeakSlots => {
            let equipped = EquippedProfile::load(&equipped_path)?;
            let plan = GearPlan::embedded();
            let standings = weak_slots(&equipped, data, &profile, &state, Some(&plan));
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&standings)?);
            } else {
                println!("{:<10} {:>6}  {:<30} {:<24} priority stats", "Slot", "Score", "Equipped", "Guide base");
                for s in standings {
                    println!("{:<10} {:>6.2}  {:<30} {:<24} {}", s.slot.label(), s.score,
                             s.item_name.clone().unwrap_or_else(|| "(empty)".into()),
                             s.guide_base.clone().unwrap_or_default(), s.top_affixes.join(", "));
                }
            }
        }
        Command::Character => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&state)?);
            } else {
                println!("character.json: {}", character_path.display());
                println!("level {}  phase {}  endurance {}%", state.level, state.phase(), state.endurance);
                println!("resistances: {}", state.resistance_list().iter().map(|(e, v)| format!("{e} {v:.0}%")).collect::<Vec<_>>().join(", "));
                println!("Heaven's Bulwark {} pts, Healing Hands specced {}, Solarum Plate {}, Nagasa Scymitar {}",
                         state.heavens_bulwark_points, state.healing_hands_specced, state.solarum_plate_equipped, state.nagasa_scymitar_equipped);
                println!("\nguide profile: {}", profile.name);
                for rule in &profile.stats {
                    let status = if rule.is_active(&state) { format!("active  w={:.2}", rule.weight) } else {
                        format!("OFF     w={:.2} ({})", rule.inactive_weight, rule.why_inactive(&state).unwrap_or_default())
                    };
                    println!("  {:<8} #{:<2} {:<50} {}", format!("{:?}", rule.group).to_lowercase(), rule.rank, rule.name, status);
                }
            }
        }
        Command::Ocr { image, engine } => {
            let boxes = ocr::recognize_file(&image, engine.into())?;
            let panels = ocr::panels(&boxes, None);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "boxes": boxes, "panels": panels }))?);
            } else {
                for (i, panel) in panels.iter().enumerate() {
                    println!("## panel {i} rect={:?}{}", panel.rect, if panel.is_equipped_compare() { " EQUIPPED-COMPARE" } else { "" });
                    for (i, line) in panel.lines.iter().enumerate() {
                        let tag = match panel.tints.get(i) { Some(ocr::Tint::Green) => "[green] ", Some(ocr::Tint::Red) => "[red] ", _ => "" };
                        println!("   {tag}{line}");
                    }
                }
            }
        }
        Command::Sheet { image, apply } => {
            let boxes = ocr::recognize_file(&image, OcrEngineKind::Auto)?;
            let rgba = image::open(&image)?.to_rgba8();
            match le_core::sheet_reader::read_sheet_from_image(&rgba, &boxes, OcrEngineKind::Auto) {
                Some(reading) if reading.is_useful() => {
                    println!("level: {:?}", reading.level);
                    let mut elements: Vec<_> = reading.resistances.iter().collect();
                    elements.sort_by(|a, b| a.0.cmp(b.0));
                    for (e, v) in elements {
                        println!("{e:<10} {v:.0}%");
                    }
                    if apply {
                        let mut new_state = state.clone();
                        let changes = reading.apply(&mut new_state);
                        new_state.save(&character_path)?;
                        println!("saved {} ({})", character_path.display(), if changes.is_empty() { "no changes".into() } else { changes.join(", ") });
                    }
                }
                _ => println!("no RESISTANCES block found in {}", image.display()),
            }
        }
        Command::Guide => {
            let plan = GearPlan::embedded();
            println!("{} [{}] by {}", plan.name, plan.planner_id, plan.author);
            for bracket in &plan.brackets {
                println!("\n== {} (levels {}-{}, phase {})", bracket.name, bracket.level_min, bracket.level_max, bracket.phase);
                for slot in Slot::ALL {
                    let Some(item) = bracket.items.get(&slot) else { continue };
                    let name = match item.unique_id {
                        Some(u) => data.unique(u).map(|u| u.name.clone()).unwrap_or_default(),
                        None => data.base_name(item.item_type, item.sub_type),
                    };
                    let affixes = item.affixes.iter().map(|a| format!("{} T{}", data.affix_name(a.id), a.tier)).collect::<Vec<_>>().join(", ");
                    println!("  {:<10} {} ({}): {}", slot.label(), name, data.type_name(item.item_type), affixes);
                }
            }
        }
        Command::UpdateGuide { source } => {
            let planner_id = if source.starts_with("http") { GearPlan::fetch_planner_id(&source)? } else { source };
            let (raw, plan) = GearPlan::fetch(&planner_id)?;
            let out = Path::new("core/data").join(format!("planner_{planner_id}.json"));
            std::fs::create_dir_all("core/data")?;
            std::fs::write(&out, raw)?;
            println!("saved {} ({} brackets) to {}; rebuild to embed it", plan.name, plan.brackets.len(), out.display());
        }
        Command::Builds => {
            let builds = library.list();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&builds)?);
            } else {
                for b in builds {
                    println!("{} {:<40} {:<18} {:<28} {} stats  {}", if b.active { "*" } else { " " }, b.id, b.character_name, b.build_name, b.stat_count, b.model);
                }
                println!("\nbuilds live in {}", library.dir().display());
            }
        }
        Command::UseBuild { id } => {
            library.set_active(&id)?;
            let (_, profile, problem) = library.active_profile();
            if let Some(problem) = problem {
                anyhow::bail!(problem);
            }
            println!("active build: {} ({})", id, profile.name);
        }
        Command::AnalyzeGuide { input, character, build, source, model, force } => {
            let text = if input == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                std::fs::read_to_string(&input).with_context(|| format!("reading {input}"))?
            };
            let id = BuildLibrary::make_id(&character, &build);
            let hash = BuildLibrary::hash(&text);
            if let Some(existing) = library.get(&id).filter(|b| b.guide_hash == hash && !force) {
                library.set_active(&id)?;
                println!("guide unchanged: reused the stored profile of {} ({}); now active", existing.id, existing.model);
                return Ok(());
            }
            let model = model_from_settings(&cli.profile_dir, "guide", model.as_deref())?;
            let client = ai::Client::new(model.clone())?;
            let mut names: Vec<String> = data.affixes().map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).collect();
            names.sort();
            names.dedup();
            eprintln!("analysing {} characters with {} …", text.len(), model.label());
            let analysis = ai::analyze_guide(&client, ai::GuideRequest {
                guide_text: &text,
                character_name: &character,
                build_name: &build,
                template_json: le_core::guide_profile::embedded_json(),
                affix_names: &names,
            })?;
            let now = BuildLibrary::now();
            let created = library.get(&id).map(|b| b.created).unwrap_or(now);
            let saved = Build {
                id: id.clone(), character_name: character, build_name: build,
                source: if source.is_empty() { "pasted guide".into() } else { source },
                guide_hash: hash, guide_text: text, created, updated: now, model: model.label(),
                profile_json: analysis.profile_json, notes: String::new(),
            };
            library.save(&saved)?;
            library.set_active(&id)?;
            println!("saved and activated build {id}: {} ({} stats, {} facts) [{}]", analysis.profile.name, analysis.profile.stats.len(), analysis.profile.facts.len(), analysis.usage);
            if cli.json {
                println!("{}", saved.profile_json);
            }
        }
        Command::Models { provider } => {
            let config = model_from_settings(&cli.profile_dir, "item", Some(&format!("{provider}/-")))?;
            let prices = ai::PriceTable::load(Some(&cli.profile_dir));
            let models = ai::list_models(&config, &prices)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&models)?);
            } else {
                println!("{:<40} {:>9} {:>9} {:>10} {:>10}", "model", "in/1M", "out/1M", "per item", "per guide");
                for m in models {
                    match m.input_price {
                        Some(i) => println!("{:<40} {:>9.2} {:>9.2} {:>10.4} {:>10.3}", m.id, i, m.output_price.unwrap_or(0.0), m.item_cost.unwrap_or(0.0), m.guide_cost.unwrap_or(0.0)),
                        None => println!("{:<40} {:>9} {:>9}", m.id, "?", "?"),
                    }
                }
            }
        }
        Command::Ai { image, model, engine } => {
            let model = model_from_settings(&cli.profile_dir, "item", model.as_deref())?;
            let client = ai::Client::new(model.clone())?;
            let boxes = ocr::recognize_file(&image, engine.into())?;
            let rgba = image::open(&image)?.to_rgba8();
            let panels = ocr::panels(&boxes, None);
            let (candidate, candidate_panel) = panels
                .iter()
                .filter(|p| !p.is_equipped_compare() && !p.is_clipped() && p.lines.len() >= 3)
                .map(|p| (parse_tooltip(&p.item_lines(), data), p))
                .find(|(item, _)| item.recognised())
                .ok_or_else(|| anyhow::anyhow!("no item tooltip found in {}", image.display()))?;
            let equipped_panel = panels.iter().find(|p| p.is_equipped_compare() && p.lines.len() >= 3 && !p.is_clipped());
            let equipped = EquippedProfile::load(&equipped_path)?;
            let verdict = compare(&candidate, &equipped, data, &profile, &state);
            let crop = |rect: (i32, i32, i32, i32)| -> Result<Vec<u8>> {
                let (iw, ih) = rgba.dimensions();
                let x0 = (rect.0 - 12).max(0) as u32;
                let y0 = (rect.1 - 12).max(0) as u32;
                let x1 = ((rect.0 + rect.2 + 12) as u32).min(iw);
                let y1 = ((rect.1 + rect.3 + 12) as u32).min(ih);
                let img = image::imageops::crop_imm(&rgba, x0, y0, x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1)).to_image();
                let mut bytes = std::io::Cursor::new(Vec::new());
                img.write_to(&mut bytes, image::ImageFormat::Png)?;
                Ok(bytes.into_inner())
            };
            eprintln!("asking {} …", model.label());
            let answer = ai::analyze_item(&client, ai::ItemRequest {
                candidate_png: crop(candidate_panel.rect)?,
                equipped_png: equipped_panel.map(|p| crop(p.rect)).transpose()?,
                candidate_text: candidate_panel.item_lines().join("\n"),
                equipped_text: equipped_panel.map(|p| p.item_lines().join("\n")),
                game_diff: candidate_panel.compare_lines(),
                deterministic: &verdict,
                profile: &profile,
                state: &state,
            })?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "deterministic": verdict, "ai": answer }))?);
            } else {
                println!("deterministic: {} ({:+.2})", verdict.label.as_str(), verdict.delta);
                println!("AI [{} · {}]: {}", model.label(), answer.confidence, answer.verdict);
                println!("  {}", answer.summary);
                for r in &answer.reasons {
                    println!("  {r}");
                }
                for w in &answer.warnings {
                    println!("  warning: {w}");
                }
            }
        }
    }
    Ok(())
}

/// A model from `profile/settings.json`: `ai.<which>` (item | guide), or an
/// explicit "provider/model-id"; the key and endpoint come from `ai.providers`.
fn model_from_settings(profile_dir: &Path, which: &str, override_ref: Option<&str>) -> Result<ModelConfig> {
    let settings: serde_json::Value = std::fs::read_to_string(profile_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null);
    let ai_settings = &settings["ai"];
    let (provider, model) = match override_ref {
        Some(r) => {
            let (p, m) = r.split_once('/').ok_or_else(|| anyhow::anyhow!("--model expects provider/model-id, e.g. openai/gpt-5-mini"))?;
            (p.to_string(), m.to_string())
        }
        None => (
            ai_settings[which]["provider"].as_str().unwrap_or("anthropic").to_string(),
            ai_settings[which]["model"].as_str().unwrap_or(if which == "guide" { "claude-opus-5" } else { "claude-sonnet-5" }).to_string(),
        ),
    };
    let entry = &ai_settings["providers"][&provider];
    Ok(ModelConfig {
        vision: ai::supports_vision(&provider, &model),
        effort: ai_settings["effort"].as_str().unwrap_or("medium").to_string(),
        api_key: entry["api_key"].as_str().unwrap_or("").to_string(),
        base_url: entry["base_url"].as_str().unwrap_or("").to_string(),
        provider,
        model,
    })
}

/// Tooltip from a text file, stdin ("-") or an image (OCR, nearest non-EQUIPPED panel).
fn load_item(input: &str, engine: OcrEngineKind, data: &GameData) -> Result<(ParsedItem, String)> {
    let path = Path::new(input);
    let is_image = path.extension().map_or(false, |e| matches!(e.to_string_lossy().to_lowercase().as_str(), "png" | "jpg" | "jpeg" | "bmp"));
    let lines: Vec<String> = if is_image {
        let boxes = ocr::recognize_file(path, engine)?;
        let panels = ocr::panels(&boxes, None);
        panels
            .iter()
            .filter(|p| !p.is_equipped_compare() && !p.is_clipped() && p.lines.len() >= 3)
            .map(|p| p.item_lines())
            .find(|lines| parse_tooltip(lines, data).recognised())
            .or_else(|| ocr::pick_item_panel(&panels).map(|p| p.item_lines()))
            .unwrap_or_default()
    } else if input == "-" {
        std::io::read_to_string(std::io::stdin())?.lines().map(String::from).collect()
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?.lines().map(String::from).collect()
    };
    let item = parse_tooltip(&lines, data);
    Ok((item, lines.join("\n")))
}

fn print_item(item: &ParsedItem, data: &GameData) {
    if !item.recognised() {
        println!("not recognised: {}", item.warnings.join("; "));
        println!("title: {:?}", item.title);
        return;
    }
    println!("{} ({} {}, {}){}", item.display_name(), item.rarity(), item.type_name, item.base_name,
             if item.level_requirement > 0 { format!(", requires level {}", item.level_requirement) } else { String::new() });
    if !item.implicit_lines.is_empty() {
        println!("implicits: {}", item.implicit_lines.join(" | "));
    }
    for a in &item.affixes {
        let value = a.value.map(|v| format!("{}{}", v, if a.is_percent { "%" } else { "" })).unwrap_or_else(|| "?".into());
        let range = data.affix(a.affix_id).and_then(|x| x.tier_range(a.tier)).map(|(lo, hi)| format!("{lo:.0}-{hi:.0}")).unwrap_or_default();
        println!("  {:<6} T{} ({:?}, {}) {:<40} value {}  range {}", format!("{:?}", a.kind).to_lowercase(), a.tier,
                 a.tier_source, a.match_score.round(), a.name, value, range);
    }
    for u in &item.unmatched {
        println!("  unrecognised: {u}");
    }
    for w in &item.warnings {
        println!("  ! {w}");
    }
}

fn print_verdict(verdict: &le_core::compare::Verdict, item: &ParsedItem) {
    println!("{}  {}  {:.2} vs {:.2}  (delta {:+.2})", verdict.label.as_str(), verdict.candidate_name,
             verdict.candidate_score, verdict.equipped_score, verdict.delta);
    if let Some(slot) = verdict.slot {
        println!("slot: {}  equipped: {}", slot.label(), verdict.equipped_name.clone().unwrap_or_else(|| "(empty)".into()));
    }
    println!("reasons:");
    for r in &verdict.reasons {
        println!("  {r}");
    }
    if verdict.label != VerdictLabel::Unknown {
        println!("candidate affixes:");
        for a in &verdict.candidate.affixes {
            println!("  {:+.2}  {:<40} T{}  {}", a.contribution, a.name, a.tier, a.note);
        }
        if let Some(eq) = &verdict.equipped {
            println!("equipped affixes:");
            for a in &eq.affixes {
                println!("  {:+.2}  {:<40} T{}  {}", a.contribution, a.name, a.tier, a.note);
            }
        }
    }
    if !verdict.warnings.is_empty() {
        println!("warnings:");
        for w in &verdict.warnings {
            println!("  ! {w}");
        }
    }
    let _ = item;
}
