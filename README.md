# LE Gear Advisor

[![Download installer](https://img.shields.io/github/v/release/Coodex/Last-Epoch-Gear-Advisor?label=installer&logo=windows&color=d4a94e)](https://github.com/Coodex/Last-Epoch-Gear-Advisor/releases/latest/download/LE.Gear.Advisor_0.1.0_x64-setup.exe)
[![Release](https://img.shields.io/github/release-date/Coodex/Last-Epoch-Gear-Advisor?label=released)](https://github.com/Coodex/Last-Epoch-Gear-Advisor/releases/latest)
![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust)
![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8D8?logo=tauri&logoColor=white)
![Svelte 5](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white)
![Windows 11](https://img.shields.io/badge/Windows-11-0078D4?logo=windows11&logoColor=white)
![License: MIT](https://img.shields.io/badge/license-MIT-green)

**[Download the Windows installer (v0.1.0)](https://github.com/Coodex/Last-Epoch-Gear-Advisor/releases/latest)** · per-user NSIS setup, no admin rights needed. All releases: [releases page](https://github.com/Coodex/Last-Epoch-Gear-Advisor/releases).

Tells you whether an item in your **inventory** is an upgrade over what you
have equipped, for the Maxroll Paladin leveling guide
(<https://maxroll.gg/last-epoch/build-guides/paladin-leveling-guide>), and why.

No online character data is used. The character API only exposes equipped
gear; what needs comparing is the bag. Items come from tooltip screenshots
(OCR) or pasted tooltip text, equipped gear is stored per slot in a local
profile, and the few facts the guide's conditions depend on live in
`profile/character.json`.

Phase 1 (this state of the repo) is the Rust core plus a CLI so scoring can
be validated on real tooltips. Phase 2 is the Tauri 2 overlay (`app/`).

## Layout

```
core/            Rust library `le_core`
  src/text.rs            normalisation, OCR digit fixes, fuzzy matching
  src/game_data.rs       affixes / bases / uniques (embedded from core/data)
  src/item_parser.rs     tooltip lines -> ParsedItem
  src/guide_profile.rs   Stats Priorities as data: weights, conditions, saturation
  src/character_state.rs character.json + equipped profile
  src/scorer.rs          item score with saturation and conditions
  src/compare.rs         verdict vs equipped item, warnings, weak-slot list
  src/planner.rs         the guide's Maxroll planner (gear per level bracket)
  src/ocr.rs             Windows OCR (or tesseract.exe), line + panel grouping
  data/                  affixes.json, bases.json, uniques.json, planner_*.json,
                         guide_profile.json
  tests/tooltips.rs      sample tooltips for every slot type
cli/             `le-advisor` binary
tools/build_data.py      rebuilds core/data from Maxroll's game data + planner
profile/                 character.json (yours), equipped.json (created by `equip`)
```

## Setup

Rust stable (1.97 tested) is all the core needs. Windows OCR is built into
Windows 10/11 and reached through the pure-Rust `windows` crate, so there are
no native build tools or extra installs. If `tesseract.exe` is installed it
can be selected with `--engine tesseract`.

```
cargo build --release
target\release\le-advisor.exe --help
```

Regenerating the game data (needs Python 3.12, only when the game or the
guide changes):

```
py -3.12 tools/build_data.py            # data + planner for the Paladin guide
py -3.12 tools/build_data.py <guide-url-or-planner-id>
cargo build --release                   # the JSON is embedded at compile time
```

## Workflow

1. Fill in `profile/character.json` (schema below).
2. Store your equipped gear once, one tooltip per slot:
   ```
   le-advisor equip boots.txt                 # slot inferred from the item type
   le-advisor equip ring.png --slot ring2     # rings need the slot
   le-advisor equipped                        # review
   ```
   Tooltip text files hold one tooltip line per row, exactly as the game
   shows them (title, type line, implicits, affixes, "Requires: Level N").
   Screenshots are OCR'd; the tooltip nearest the cursor is used and the
   "EQUIPPED" compare tooltip is skipped.
3. Judge bag items:
   ```
   le-advisor compare item.txt
   le-advisor compare screenshot.png
   le-advisor weak-slots                      # which equipped slot to replace first
   le-advisor character                       # state + which guide rules are active
   ```
   `--json` on any command prints machine-readable output (the overlay uses the same structures).

Example verdict:

```
UPGRADE  Outcast Gloves  1.85 vs 0.62  (delta +1.23)
slot: Gloves  equipped: Hide Gloves
reasons:
  +0.80 Elemental DoT / DoT / Fire damage T5 (T5 of target T5)
  +0.72 Health T4 (T4 of target T5)
  -0.48 losing Increased Melee Attack Speed T4
warnings:
  ! swapping drops Fire Res below cap by 12%
```

## Guide profiles, facts and builds

`core/data/guide_profile.json` is the built-in profile. Every profile (built-in
or AI-generated) may declare `facts` and `phase_levels`:

```json
"phase_levels": { "intermediate_from": 27, "final_from": 38 },
"facts": [
  { "key": "judgement_points", "label": "Points in Judgement", "kind": "counter", "default_count": 0 },
  { "key": "uses_shield", "label": "Shield equipped", "kind": "flag", "default_on": true }
]
```

and a stat's `conditions` can reference them next to `phase` / `level`:
`{ "counters": { "judgement_points": { "min": 5 } }, "flags": { "uses_shield": true } }`.
Their values live in `character.json` under `flags` / `counters` (seeded with
the defaults when a build is activated) and are edited in the Builds window.
Saved builds are plain JSON files in `profile/builds/`; `profile/builds/active.txt`
names the active one.

## character.json

```json
{
  "level": 54,
  "phase": null,
  "resistances": { "fire": 89, "cold": 84, "lightning": 93, "physical": 142,
                   "necrotic": 58, "void": 114, "poison": 23 },
  "endurance": 46,
  "heavens_bulwark_points": 0,
  "healing_hands_specced": true,
  "solarum_plate_equipped": false,
  "nagasa_scymitar_equipped": false
}
```

* `level` picks the setup phase (1-26 early, 27-37 intermediate, 38+ final,
  matching the guide's brackets); `phase` overrides it (`"early"`,
  `"intermediate"`, `"final"`).
* `resistances` are the character sheet values in percent. When the sheet
  shows `75% (89%)`, enter the number in parentheses: that is the total before
  the cap, and the scorer needs it to know how much headroom a swap really has.
* The four flags feed the guide's conditions. Every field has a default, so a
  partial file works.

## Scoring

Everything is data in `core/data/guide_profile.json` (`--guide-profile FILE`
swaps it). Each entry is a stat group: regexes over affix names, a weight, a
group (offense/defense), optional conditions, an optional `inactive_weight`
("near zero" cases), an optional `tier_target`, an optional `only_item_types`
list and an optional saturation rule.

* An ordinary affix scores `weight * min(tier / tier_target, 1.2)`.
* Resistances score `weight * min(value, gap) / reference_roll`, where
  `gap = cap - (current - contribution of the item being replaced)`; capped
  elements are worth nothing and the note says so.
* Conditions: `phase`, `level`, `heavens_bulwark_points` (min/max),
  `healing_hands_specced`, `solarum_plate_equipped`, `nagasa_scymitar_equipped`.
  When they fail the rule uses `inactive_weight`.
* Affixes not on the list score zero and are listed; unreadable lines are
  reported as `unrecognised affix line`. Nothing about an item aborts parsing.
* Verdict: candidate score minus the equipped item's score in that slot (rings:
  the weaker ring). `>= +0.1` UPGRADE, `<= -0.1` WORSE, otherwise SIDEGRADE.
  Uniques are REVIEW (their mods are not scored). Warnings cover resistance
  caps ("swapping drops Fire Res below cap by 12%"), level requirements and
  parser doubts.

The encoded priorities, from the guide's Stats Priorities section:

| # | Offense | condition |
| --- | --- | --- |
| 1 | Ignite / Poison / Bleed chance | zero once Heaven's Bulwark has 5 points |
| 2 | Level of Javelin, Rive | early and intermediate setups |
| 2 | Level of Multistrike, Healing Hands, Holy Aura, Symbols of Hope | final setup |
| 2 | Level of Judgement (weight 1.4, target T5) | final setup |
| 3 | Elemental DoT, DoT, Fire damage | |
| 4 | Healing Effectiveness (weighted above its position) | |
| 5 | Increased Melee Attack Speed | |
| 6 | Spell damage | near zero unless Nagasa Scymitar is equipped |

| # | Defense | condition |
| --- | --- | --- |
| 1 | Health Regen | near zero once Healing Hands is specialised |
| 2 | Health | |
| 3 | Resistances, physical weighted higher | gap to the 75% cap, per element |
| 4 | Ward per Second, Ward Decay Threshold | |
| 5 | Endurance | zero once Solarum Plate is equipped |
| 6 | Vitality | |
| 7 | Frailty / Slow / Chill chance | |
| 8-9 | Block Chance, Block Effectiveness | only after 5 points in Heaven's Bulwark |
| 10 | Armour | |
| 11 | Dodge | |

## Tooltip parsing

Real tooltip layout (verified on captures): title (affix titles plus the base
name, may wrap), a "[CLASS] TYPE" line, one line per base implicit, affixes,
then "Requires: Level N"; a compare-with-equipped section follows and is
ignored (parsing stops at the requirement line or at the first negative
value). The type line fixes the slot, the longest base name inside the title
fixes the base, the base's implicit count separates implicits from affixes.
Affix lines are fuzzy-matched against what can roll on that item type, with
OCR quirks handled (upper case, dropped spaces, `0` read as `O`). The tier is
read from a `T<n>` label when present and otherwise inferred from the value
using the affix's per-tier roll ranges.

## Updating the guide profile

Edit `core/data/guide_profile.json` (or a copy passed with
`--guide-profile`). Patterns are regexes over the affix name and display
name; use `le-advisor parse` on a tooltip to see the exact affix names the
data uses. `le-advisor character` lists every rule with its current
active/inactive status and the condition it failed. `le-advisor update-guide`
refreshes the planner JSON when Maxroll changes the gear plan; rebuild to embed it.

## Tests

```
cargo test
```

`core/tests/tooltips.rs` covers a tooltip for every slot type (helmet, body,
belt, boots, gloves, one-handed and two-handed weapons, shield, amulet, ring,
relic, unique), OCR quirks, unrecognised affixes, garbage input, verdict
labels, the resistance-cap warning, ring selection and condition changes.

## The overlay (`app/`)

Tauri 2 app: Rust backend (global hotkey, screen capture around the cursor,
Windows OCR and scoring through `le_core`) and a Svelte 5 + Tailwind
frontend. The window is transparent, always on top and click-through, and
shows nothing until the hotkey is pressed.

### Using it

1. Start the overlay (from a built installer, or `npm run tauri dev` in `app/`).
   A small "ready" toast appears bottom-right for a moment.
2. In Last Epoch (windowed or borderless), open your inventory, hover a bag
   item and press **Numpad0** (the deterministic scorer) or **Numpad1** (AI).
3. A verdict card fades in next to the tooltip: UPGRADE / SIDEGRADE / WORSE,
   the score delta, the top three reasons and any warnings. It stays until
   the mouse moves away (set `card_seconds` to add a timer). With Numpad1 the
   deterministic card shows first and is replaced by the AI's answer a few
   seconds later (purple "AI · model" badge, one-line summary, its reasons).
4. **Numpad3** cycles the item-analysis model among the configured models
   that have an API key; a toast names the new one.

There is nothing to set up per slot: with the game's **Auto Compare Items**
enabled, hovering a bag item shows the equipped item's tooltip next to it
(headed "EQUIPPED"), and the overlay reads both from the same capture. The
green/red difference block the game draws under the hovered item is cut off
by text colour, so it never counts as affixes. The equipped item is also written to
`profile/equipped.json` so `le-advisor weak-slots` works afterwards, but the
overlay never compares against that memory: only the EQUIPPED tooltip in the
same capture counts. If it is missing, cut off at the capture edge or
unreadable, the card shows REVIEW with the item's own score and says why,
instead of a verdict against a stale item. If the character sheet is open
in the same capture (its RESISTANCES block is visible), level and
resistances are refreshed from it before the comparison, and the card notes
what changed; a hotkey press over the sheet with nothing hovered does the
same on its own.

Capture only happens while a window titled "Last Epoch" is in the
foreground; browser tooltips never trigger it.

### Tray menu

The overlay lives in the system tray (gold shield icon). Left- or right-click it for:

- **Scan now**: same as the hotkey.
- **Builds & AI…**: opens the Builds window (below).
- **Hotkeys / AI item model**: read-only labels showing the current bindings and model.
- **Enabled**: uncheck to pause the hotkey without quitting.
- **Save debug captures**: toggles writing each capture and its OCR text to `profile/debug/`.
- **Open profile folder**: opens the folder holding `character.json`, `settings.json`, `equipped.json`.
- **Quit**.

### AI analysis and pasted guides

Two AI features, both optional and off until a model has an API key:

- **Numpad1 · AI item verdict.** The same capture/OCR/scoring pipeline runs,
  then the hovered tooltip crop, the EQUIPPED tooltip crop, the OCR text,
  the build's priorities (with which rules are active and why), the
  character state and the deterministic verdict go to the *item model*. The
  model answers with a strict JSON verdict (UPGRADE / SIDEGRADE / WORSE /
  REVIEW, a one-line summary, reasons, warnings, confidence) and the card
  shows it. The deterministic scores stay on the card for reference. Models
  configured without vision get the OCR text only. The prompt carries a
  short mechanics sheet (Vitality = 6 flat Health, attribute effects, caps,
  how increased/flat/more stack) and the game's own green/red compare block
  under the hovered item, tagged [better]/[worse], which the model is told to
  trust for net changes. Answers are cached in
  `profile/ai_cache.json` keyed by the hovered item, the equipped item, the
  model, the active build and the character facts: pressing the hotkey again
  on the same combination shows the stored verdict (badge "cached") without
  a request. Tray > *Clear AI verdict cache* empties it.
- **Builds window · guide → priorities.** Tray > *Builds & AI…* opens a normal
  window. Paste any Last Epoch guide (Ctrl+A / Ctrl+C on the page), give the
  character and build a name, pick the *guide model* and press *Analyse with
  AI*. The model converts the guide into the same stat-priority JSON format
  as `core/data/guide_profile.json` (weights, regex patterns over the real
  affix names, phase/level conditions, resistance saturation) plus
  build-specific *facts* (yes/no flags and counters such as "skill X has 5
  points") that its conditions can reference. The result is validated
  (regexes compile, every fact declared, weights sane; one automatic retry
  with the validation error) and stored under `profile/builds/<character>--<build>.json`
  together with the guide text and its hash, then activated. Pasting the same
  guide again reuses the stored profile without any AI call; a changed text
  re-analyses it. The built-in Maxroll Paladin profile is always listed and
  can be re-activated at any time. The window also shows the active build's
  facts (and, for the built-in profile, the four Paladin facts) so you can
  keep them current, and the level/resistances read from the sheet.

Providers: Anthropic (Messages API), OpenAI, Kimi (Moonshot) and DeepSeek
(OpenAI-compatible chat completions), or any other OpenAI-compatible server
via a base URL. The AI section of the Builds window has three parts:

1. **Providers**: one API-key field per provider (stored in `settings.json`,
   or read from `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `MOONSHOT_API_KEY`,
   `DEEPSEEK_API_KEY`). Entering a key connects: the provider's model list is
   fetched, which proves the key works, and cached for the pickers.
2. **Model per job**: an *Item verdicts* dropdown (the hotkey model; fast and
   cheap does well) and a *Guide analysis* dropdown (the most capable; a guide
   is analysed once). Options are grouped by provider and show list prices
   per 1M tokens plus a rough cost per item verdict / per guide, from the
   built-in table `core/data/model_prices.json` (providers do not expose
   prices; copy the file to `profile/model_prices.json` to add or correct
   entries). "Other model id…" accepts any id. A chip list defines what
   Numpad3 rotates the item model through.
3. **Advanced**: reasoning effort.

Everything autosaves. Text-only models (DeepSeek) get the OCR text instead
of screenshots; a model that rejects images is retried without them. Only
tooltip crops, OCR text and the pasted guide leave the machine, and only to
the provider you chose.

The CLI has the same features without the overlay: `le-advisor builds`,
`le-advisor use-build <id>`, `le-advisor analyze-guide guide.txt --character X --build Y`,
`le-advisor ai screenshot.png [--model openai/gpt-5-mini]`, `le-advisor models --provider openai`. Every CLI command scores with the active
build's profile unless `--guide-profile` points elsewhere.

### Settings (`profile/settings.json`)

```json
{
  "hotkey": "Numpad0", "any_window": false, "card_seconds": 0, "debug_captures": false, "ocr_engine": "auto",
  "ai": {
    "hotkey_ai": "Numpad1", "hotkey_cycle": "Numpad3",
    "providers": {
      "anthropic": { "api_key": "", "base_url": "" },
      "openai":    { "api_key": "", "base_url": "" },
      "kimi":      { "api_key": "", "base_url": "" },
      "deepseek":  { "api_key": "", "base_url": "" },
      "openai-compatible": { "api_key": "", "base_url": "" }
    },
    "item":  { "provider": "anthropic", "model": "claude-sonnet-5" },
    "guide": { "provider": "anthropic", "model": "claude-opus-5" },
    "cycle": [ { "provider": "anthropic", "model": "claude-sonnet-5" }, { "provider": "anthropic", "model": "claude-opus-5" } ],
    "effort": "medium",
    "catalog": {}
  }
}
```

`catalog` caches the fetched model lists. Settings files from the earlier
per-model layout (`ai.models`) are migrated on load.

`card_seconds` is 0 by default: a verdict card stays until the mouse moves
about 40 px; any positive value adds a timer on top (AI cards get a few extra
seconds). `hotkey`, `ai.hotkey_ai` and `ai.hotkey_cycle` use Tauri shortcut syntax
(`"F8"`, `"Numpad0"`, `"Ctrl+Shift+Space"`). Hotkeys are registered
system-wide with `RegisterHotKey`, so the game does not receive them; the
overlay must be restarted after changing them. Avoid Alt or Ctrl chords: holding them changes the tooltip
(mod explanations, compare view) at the moment it is captured.
`debug_captures` writes every capture and its OCR panels to `profile/debug/`,
which is what to send when an item is misread. The profile directory is
looked up next to the executable and its parents (the repo's `profile/` in
development), falling back to `%APPDATA%\LE Gear Advisor\profile`.

### Building

The repo ignores the personal files under `profile/` (`settings.json` with
your API keys, `character.json`, `builds/`, `ai_cache.json`). On a fresh
clone copy `profile/settings.example.json` to `profile/settings.json` before
building; `character.json` is created on first run.

```
cd app
npm install
npm run tauri dev      # run against the repo's profile/
npm run tauri build    # Windows installer in app/src-tauri/target/release/bundle/nsis/
```

Requirements: Rust, Node 20+, WebView2 (part of Windows 11). No native build
tools: OCR is the Windows built-in engine, capture is `xcap`, both pure Rust.

Not built yet: the optional local WebSocket + PWA mirror for a second screen.
