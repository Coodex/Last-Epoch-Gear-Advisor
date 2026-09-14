//! Parser + scoring on sample tooltip text for every slot type.
//! The texts follow the real in-game layout (title, type line, implicits,
//! affixes, "Requires: Level"), including OCR quirks seen on captures.

use le_core::character_state::{CharacterState, EquippedProfile};
use le_core::compare::{compare, VerdictLabel};
use le_core::game_data::{GameData, Slot};
use le_core::guide_profile::GuideProfile;
use le_core::item_parser::{parse_tooltip, parse_tooltip_text, TierSource};

fn data() -> &'static GameData {
    GameData::embedded()
}

fn state() -> CharacterState {
    let mut s = CharacterState { level: 49, ..Default::default() };
    for (e, v) in [("fire", 62.0), ("cold", 75.0), ("lightning", 70.0), ("physical", 40.0), ("necrotic", 55.0), ("void", 48.0), ("poison", 60.0)] {
        s.resistances.insert(e.into(), v);
    }
    s.healing_hands_specced = true;
    s
}

const HELMET: &str = "SPIKED HELM\nSENTINEL HELMET\n+45 Armor\n+20 Health\n+5% Stun Avoidance\n+2 to Level of Healing Hands\n+35 Health\n+18% Physical Resistance\nRequires: Level 27 Sentinel";
const BODY: &str = "FORGE PLATE\nBODY ARMOR\n+120 Armor\n+9% Health\n+3 to Level of Rive\n+40 Health\n+16% Physical Resistance\nRequires: Level 29";
const BELT: &str = "CHAIN BELT\nBELT\n+2 Potion Slots\n+12% Increased Cooldown Recovery Speed\n+30 Health\n+14% Increased Fire Damage\nRequires: Level 31";
const BOOTS: &str = "VITAL OUTCAST BOOTS OF EMBERS\nBOOTS\n+20 Armor\n8% Increased Movement Speed\n+1 Evade Charge\n19 Forging Potential\n+8 Vitality\n+3O% Fire Resistance\n+7% Critical Strike Avoidance\n+19 Dodge Rating\nRequires: Level 47";
const GLOVES: &str = "OUTCAST GLOVES\nGLOVES\n+15 Armor\n+12% Increased Melee Attack Speed\n+35% Increased Damage Over Time\n+22 Health\nRequires: Level 25";
const SWORD: &str = "NAGASA SCYMITAR\nONE-HANDED SWORD\n+45 Melee Physical Damage\n+30% Increased Damage Over Time\n+10% Increased Melee Attack Speed\n+20% Chance To Chill\nRequires: Level 54";
const AXE: &str = "CULTIST CHOPPER\nTWO-HANDED AXE\n+60 Melee Physical Damage\n+40% Increased Damage Over Time\n+12% Increased Melee Attack Speed\n+15% Chance to Inflict Bleed On Hit\nRequires: Level 14";
const SHIELD: &str = "PROTECTIVE TOWER SHIELD OF FORTIFICATION\nSHIELD\n+28% Block Chance\n+3OO Block Effectiveness\n18 Forging Potential\n+21% Increased Minion Health\n+169 Block Effectiveness\n+14% Poison Resistance\n+78 Armor\nRequires: Level 30";
const AMULET: &str = "JADE AMULET\nAMULET\n+20 Health\n+45% Increased Elemental Damage Over Time\n+60 Health\n+9 Ward Per Second\nRequires: Level 4";
const RING: &str = "SILVER RING\nRING\n+15% Elemental Resistance\n+30% Increased Healing Effectiveness\n+35% Increased Damage Over Time\n+25 Health\nRequires: Level 1";
const RELIC: &str = "ASSASSIN'S INSCRIBED\nTABLETOFREGROWTH\nSENTINEL RELIC\n+25 MANA\n-3MELEEATTACKMANACOST\nCORRUPTED-UNMODIFIABLE\n+40% Increased Health Regeneration\n+30% Cold Resistance\n+35% Increased Poison Damage\n+9% Increased Critical Strike Chance\nRequires:Level 39Sentinel\n-3 Melee Attack Mana Cost\n+12 Health";
const UNIQUE: &str = "AVARICE\nLEATHER GLOVES\nGLOVES\n+40% Increased Gold Drops\nRequires: Level 6";

#[test]
fn every_slot_type_is_recognised() {
    let cases = [
        (HELMET, Slot::Head, "Spiked Helm"), (BODY, Slot::Body, "Forge Plate"), (BELT, Slot::Waist, "Chain Belt"),
        (BOOTS, Slot::Feet, "Outcast Boots"), (GLOVES, Slot::Hands, "Outcast Gloves"), (SWORD, Slot::Weapon, "Nagasa Scymitar"),
        (AXE, Slot::Weapon, "Cultist Chopper"), (SHIELD, Slot::Offhand, "Tower Shield"), (AMULET, Slot::Neck, "Jade Amulet"),
        (RING, Slot::Ring1, "Silver Ring"), (RELIC, Slot::Relic, "Inscribed Tablet"),
    ];
    for (text, slot, base) in cases {
        let item = parse_tooltip_text(text, data());
        assert!(item.recognised(), "not recognised: {text}\n{:?}", item.warnings);
        assert_eq!(item.base_name, base, "base for {text}");
        assert!(item.slots().contains(&slot), "slot for {base}");
        assert!(!item.affixes.is_empty(), "no affixes for {base}: unmatched {:?}", item.unmatched);
        assert!(item.level_requirement > 0, "level for {base}");
    }
}

#[test]
fn implicits_are_separated_from_affixes() {
    let boots = parse_tooltip_text(BOOTS, data());
    assert_eq!(boots.implicit_lines.len(), data().sub_type(3, 3).unwrap().implicits.len());
    let names: Vec<&str> = boots.affixes.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"Vitality"));
    assert!(names.contains(&"Fire Resistance"));
    assert!(!names.iter().any(|n| n.contains("Movement Speed")), "implicit movement speed leaked: {names:?}");
    let fire = boots.affixes.iter().find(|a| a.name == "Fire Resistance").unwrap();
    assert_eq!(fire.value, Some(30.0), "OCR 'O' fixed");
    assert_eq!(fire.tier_source, TierSource::Value);
    // hybrid affix spanning two lines counted once
    assert_eq!(boots.affixes.iter().filter(|a| a.name.contains("Dodge")).count(), 1);
}

#[test]
fn compare_section_and_noise_are_ignored() {
    let relic = parse_tooltip_text(RELIC, data());
    assert_eq!(relic.level_requirement, 39);
    assert_eq!(relic.implicit_lines, vec!["+25 MANA", "-3MELEEATTACKMANACOST"]);
    assert!(relic.affixes.iter().any(|a| a.name.contains("Health Regen")));
    assert!(!relic.affixes.iter().any(|a| a.name == "Added Health"), "line after Requires must be ignored");
}

#[test]
fn unique_is_identified_and_reviewed() {
    let item = parse_tooltip_text(UNIQUE, data());
    assert!(item.is_unique());
    assert_eq!(item.unique_name.as_deref(), Some("Avarice"));
    assert_eq!(item.base_name, "Leather Gloves");
    let verdict = compare(&item, &EquippedProfile::default(), data(), &GuideProfile::embedded(), &state());
    assert_eq!(verdict.label, VerdictLabel::Review);
}

#[test]
fn garbage_never_panics() {
    for text in ["", "GPU 42℃\nMEM 6359\nCPU 74℃", "Health Potion\nRestores 120 health", "+++\n---\n%%%"] {
        let item = parse_tooltip_text(text, data());
        assert!(!item.recognised());
        let verdict = compare(&item, &EquippedProfile::default(), data(), &GuideProfile::embedded(), &state());
        assert_eq!(verdict.label, VerdictLabel::Unknown);
    }
}

#[test]
fn unrecognised_affixes_are_listed_not_fatal() {
    let text = "SPIKED HELM\nHELMET\n+45 Armor\n+20 Health\n+5% Stun Avoidance\n+35 Health\n+7% Chance to Summon a Purple Goat\nRequires: Level 27";
    let item = parse_tooltip_text(text, data());
    assert!(item.recognised());
    assert_eq!(item.affixes.len(), 1);
    assert_eq!(item.unmatched, vec!["+7% Chance to Summon a Purple Goat".to_string()]);
}

#[test]
fn upgrade_worse_and_sidegrade_verdicts() {
    let data = data();
    let profile = GuideProfile::embedded();
    let state = state();
    let mut equipped = EquippedProfile::default();
    equipped.set(Slot::Hands, parse_tooltip_text("HIDE GLOVES\nGLOVES\n+5 Armor\n+4% Increased Melee Attack Speed\n+8 Health\nRequires: Level 1", data), String::new());

    let better = compare(&parse_tooltip_text(GLOVES, data), &equipped, data, &profile, &state);
    assert_eq!(better.label, VerdictLabel::Upgrade, "{:?}", better.reasons);
    assert!(better.delta > 0.3);
    assert!(better.reasons.iter().any(|r| r.contains("Damage Over Time") || r.contains("DoT")), "{:?}", better.reasons);

    let same = compare(&equipped.get(Slot::Hands).unwrap().clone(), &equipped, data, &profile, &state);
    assert_eq!(same.label, VerdictLabel::Sidegrade);

    let worse = compare(&parse_tooltip_text("HIDE GLOVES\nGLOVES\n+5 Armor\n+3% Increased Dodge Rating\nRequires: Level 1", data), &equipped, data, &profile, &state);
    assert_eq!(worse.label, VerdictLabel::Worse);
}

#[test]
fn resistance_warning_when_swap_breaks_the_cap() {
    let data = data();
    let profile = GuideProfile::embedded();
    let state = state(); // cold at 75 (capped)
    let mut equipped = EquippedProfile::default();
    equipped.set(Slot::Relic, parse_tooltip_text("MOURNFUL PENNANT\nRELIC\n+10 Mana\n+5% Cast Speed\n+30% Cold Resistance\n+20 Health\nRequires: Level 12", data), String::new());
    let candidate = parse_tooltip_text("MOURNFUL PENNANT\nRELIC\n+10 Mana\n+5% Cast Speed\n+40% Increased Damage Over Time\n+40 Health\nRequires: Level 12", data);
    let verdict = compare(&candidate, &equipped, data, &profile, &state);
    assert!(verdict.warnings.iter().any(|w| w.contains("Cold Res below cap by 30%")), "{:?}", verdict.warnings);
}

#[test]
fn rings_are_compared_against_the_weaker_ring() {
    let data = data();
    let profile = GuideProfile::embedded();
    let state = state();
    let mut equipped = EquippedProfile::default();
    equipped.set(Slot::Ring1, parse_tooltip_text(RING, data), String::new());
    equipped.set(Slot::Ring2, parse_tooltip_text("COPPER RING\nRING\n+5% Elemental Resistance\n+6 Health\nRequires: Level 1", data), String::new());
    let verdict = compare(&parse_tooltip_text("SILVER RING\nRING\n+15% Elemental Resistance\n+40 Health\n+30% Increased Healing Effectiveness\nRequires: Level 1", data), &equipped, data, &profile, &state);
    assert_eq!(verdict.slot, Some(Slot::Ring2));
    assert_eq!(verdict.label, VerdictLabel::Upgrade);
}

#[test]
fn conditions_from_character_state_change_the_verdict() {
    let data = data();
    let profile = GuideProfile::embedded();
    let mut state = state();
    state.healing_hands_specced = false;
    let regen = parse_tooltip_text("MOURNFUL PENNANT\nRELIC\n+10 Mana\n+5% Cast Speed\n+40% Increased Health Regeneration\nRequires: Level 12", data);
    let with_regen_wanted = compare(&regen, &EquippedProfile::default(), data, &profile, &state);
    state.healing_hands_specced = true;
    let with_regen_unwanted = compare(&regen, &EquippedProfile::default(), data, &profile, &state);
    assert!(with_regen_wanted.candidate_score > 0.5);
    assert!(with_regen_unwanted.candidate_score < 0.1);

    let mut nagasa = state.clone();
    nagasa.nagasa_scymitar_equipped = true;
    let spell = parse_tooltip_text("SILVER RING\nRING\n+5% Elemental Resistance\n+25% Increased Spell Damage\nRequires: Level 1", data);
    let without = compare(&spell, &EquippedProfile::default(), data, &profile, &state);
    let with = compare(&spell, &EquippedProfile::default(), data, &profile, &nagasa);
    assert!(with.candidate_score > without.candidate_score);
}


#[test]
fn relic_with_skill_level_and_icon_glyphs() {
    // Real capture: the anvil icon before "Forging Potential" is OCR'd as a stray glyph,
    // "+1 to Smite" is the game's format for "Level of Smite", and spell damage is missing
    // from the relic roll table in the data.
    let text = "SENTINEL'S SUNRISE\nEMBLEM OF PURITY\nSENTINEL RELIC\n+14% CHANCE TO IGNITE ON HIT\nW 19 FORGING POTENTIAL\n+1 TO SMITE\n13% INCREASED SPELL DAMAGE\n+16% NECROTIC RESISTANCE\nRequires: Level 11 Sentinel";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.base_name, "Sunrise Emblem");
    assert_eq!(item.implicit_lines, vec!["+14% CHANCE TO IGNITE ON HIT"]);
    let names: Vec<&str> = item.affixes.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Level of Smite", "Increased Spell Damage", "Necrotic Resistance"], "unmatched: {:?}", item.unmatched);
    assert!(item.unmatched.is_empty());
}


#[test]
fn belt_with_misread_type_line_and_footer() {
    // "BELT" misread as "BEIT", "Requires" misread, footer hints present.
    let text = "NOMAD BELT OF\nDEFLECTION\nBEIT\n+5 POTION SLOTS\nW 14 FORGING POTENTIAL\n+39% PHYSICAL RESISTANCE\nReguires: Level 45\n19% REDUCED FIRE DAMAGE\nALT Mod Explanations";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.base_name, "Nomad Belt");
    assert_eq!(item.implicit_lines, vec!["+5 POTION SLOTS"]);
    assert_eq!(item.affixes.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Physical Resistance"]);
    assert_eq!(item.level_requirement, 45);
    assert!(item.unmatched.is_empty(), "{:?}", item.unmatched);
}

#[test]
fn footer_ends_the_item_when_requires_is_missing() {
    let text = "NOMAD BELT\nBELT\n+5 POTION SLOTS\n+39% PHYSICAL RESISTANCE\nCTRL Compare Items\n19% REDUCED FIRE DAMAGE";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.affixes.len(), 1);
    assert!(item.unmatched.is_empty());
}


#[test]
fn repeated_affix_marks_the_compare_block() {
    let text = "NOMAD BELT OF\nDEFLECTION\nBELT\n+5 POTION SLOTS\n14 FORGING POTENTIAL\n+39% PHYSICAL RESISTANCE\n\u{2022}1 POTION SLOTS\n+33% PHYSICAL RESISTANCE\n19% REDUCED FIRE DAMAGE\n21% REDUCED LIGHTNING DAMAGE\nMod Explanations";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.implicit_lines, vec!["+5 POTION SLOTS"]);
    assert_eq!(item.affixes.len(), 1);
    assert_eq!(item.affixes[0].name, "Physical Resistance");
    assert!(item.unmatched.iter().all(|u| !u.contains("REDUCED")), "{:?}", item.unmatched);
}


#[test]
fn compare_block_starting_with_an_implicit_is_cut() {
    let text = "NOMAD BELT OF\nDEFLECTION\nBELT\n+5 POTION SLOTS\n14 FORGING POTENTIAL\n+39% PHYSICAL RESISTANCE\n\u{2022}1 POTION SLOTS\n+33% PHYSICAL RESISTANCE\n19% REDUCED FIRE DAMAGE";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.affixes.len(), 1);
    assert!(item.unmatched.is_empty(), "{:?}", item.unmatched);
}

#[test]
fn capped_resistance_is_explained_in_the_reasons() {
    let data = data();
    let profile = GuideProfile::embedded();
    let mut state = state();
    state.resistances.insert("physical".into(), 142.0);
    let belt = parse_tooltip_text("NOMAD BELT\nBELT\n+5 POTION SLOTS\n+39% PHYSICAL RESISTANCE\nRequires: Level 45", data);
    let verdict = compare(&belt, &EquippedProfile::default(), data, &profile, &state);
    assert!(verdict.reasons.iter().any(|r| r.contains("Physical Resistance") && r.contains("capped")), "{:?}", verdict.reasons);
}


#[test]
fn belt_without_type_line_is_found_from_the_title() {
    let text = "NOMAD BELT OF
DEFLECTION
+5 POTION SLOTS
14 FORGING POTENTIAL
+39% PHYSICAL RESISTANCE
Requires: Level 45";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.base_name, "Nomad Belt");
    assert_eq!(item.implicit_lines, vec!["+5 POTION SLOTS"]);
    assert_eq!(item.affixes.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Physical Resistance"]);
    // an affix line must never be mistaken for a base
    let garbage = parse_tooltip_text("+1 EVADE CHARGE
+8 VITALITY
+30% FIRE RESISTANCE", data());
    assert!(!garbage.recognised());
}


#[test]
fn hybrid_affix_second_line_and_unknown_two_line_affix() {
    let text = "WHIRLPOOL CHAMPION'S
COPPER AMULET
AMULET
+9% Lightning Resistance
+2 Spell Lightning Damage
21 Forging Potential
+5% Physical Penetration
+21 Ward per Second
+124 Ward Decay Threshold
+13% Chance to Shock on Hit
+65 Ward Decay Threshold
1 Ward per second per active Maelstrom
Requires: Level 36";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.base_name, "Copper Amulet");
    let names: Vec<&str> = item.affixes.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"Ward Per Second and Ward Decay Threshold"), "{names:?}");
    assert_eq!(names.iter().filter(|n| n.contains("Ward")).count(), 1, "{names:?}");
    assert_eq!(item.unmatched, vec!["+65 Ward Decay Threshold / 1 Ward per second per active Maelstrom".to_string()]);
}


#[test]
fn wrapped_affix_name_and_no_false_continuation() {
    let text = "BLIGHTED HERETICAL
SCRIPT OF DEFLECTION
SENTINEL RELIC
9% INCREASED ARMOR
+11% VOID RESISTANCE
55 FORGING POTENTIAL
13% INCREASED DAMAGE OVER TIME
12% INCREASED DAMAGE
WHILE CHANNELLING
+5% LIGHTNING RESISTANCE
+65% PHYSICAL RESISTANCE
Requires: Level 43 Sentinel";
    let item = parse_tooltip_text(text, data());
    let names: Vec<(String, u8, Option<f64>)> = item.affixes.iter().map(|a| (a.name.clone(), a.tier, a.value)).collect();
    assert_eq!(names.len(), 4, "{names:?}");
    assert_eq!(names[0].0, "Increased Damage Over Time");
    assert!(names[1].0.to_lowercase().contains("channelling"), "{names:?}");
    assert_eq!(names[1].2, Some(12.0), "value comes from the first line");
    assert!(item.warnings.is_empty(), "{:?}", item.warnings);
}


#[test]
fn wand_with_weapon_stat_lines_and_negative_implicit() {
    let text = "INFERNAL CORAL WAND OF
CONFLAGRATION
WAND
RANGE 1.7M
BASE ATTACK RATE 1.02 - Average
+48 Spell Lightning Damage
+49% Chance to Shock on Spell Hit
-3 Spell Mana Cost
21 Forging Potential
23% increased Cold Damage
86% increased Elemental Damage Over Time
+36% Chance to Ignite on Hit
Requires: Level 51";
    let item = parse_tooltip_text(text, data());
    assert_eq!(item.base_name, "Coral Wand");
    let names: Vec<&str> = item.affixes.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"Increased Elemental Damage Over Time"), "{names:?} unmatched {:?}", item.unmatched);
    assert!(names.contains(&"Chance To Ignite"), "{names:?}");
    assert!(item.unmatched.is_empty(), "{:?}", item.unmatched);
    let dot = item.affixes.iter().find(|a| a.name == "Increased Elemental Damage Over Time").unwrap();
    assert!(dot.tier >= 5, "86% should be a high tier, got T{}", dot.tier);
}

/// A tooltip drawn over the inventory grid arrives merged with the grid's
/// labels: the item is found by its type line and parsed from there.
#[test]
fn item_is_located_inside_a_merged_inventory_panel() {
    let data = GameData::embedded();
    let lines: Vec<String> = [
        "2055 an . 89", "9502", "4625", "BLESSINGS", "{INVENTORY", "APPEARAN", "131 FPS",
        "CLERIC'S CURSED COIN AMULET", "OF INSULATION", "AMULET",
        "+73% LIGHTNING RESISTANCE", "+1 TO ALL SKILLS", "5% INCREASED DAMAGE TAKEN", "\" CORRUPTED - UNMODIFIABLE",
        "69% INCREASED HEALING EFFECTIVENESS", "+13% LIGHTNING RESISTANCE", "CRAFTI", "+11% ELEMENTAL RESISTANCE",
        "+28% COLD RESISTANCE", "1,049", "+6 DEXTERITY", "ITEMS", "RESOURCES",
    ].iter().map(|s| s.to_string()).collect();
    let item = parse_tooltip(&lines, data);
    assert!(item.recognised(), "warnings: {:?}", item.warnings);
    assert_eq!(item.type_name.to_lowercase(), "amulet");
    assert!(item.affixes.iter().any(|a| a.name.to_lowercase().contains("healing effectiveness")), "affixes: {:?}", item.affixes.iter().map(|a| &a.name).collect::<Vec<_>>());
}
