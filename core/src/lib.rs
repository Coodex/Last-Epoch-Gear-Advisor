//! Last Epoch gear advisor core.
//!
//! Pipeline: tooltip text (pasted or OCR'd) -> [`item_parser::ParsedItem`] ->
//! [`scorer`] (weights and conditions from the data-driven
//! [`guide_profile::GuideProfile`], saturation from [`character_state::CharacterState`])
//! -> [`compare`] against the item equipped in the same slot.
//!
//! Everything except [`ocr`] is pure logic and runs on any platform; `ocr`
//! uses the Windows built-in OCR engine (pure-Rust bindings, no native build
//! tools) with an optional `tesseract.exe` subprocess fallback.

pub mod ai;
pub mod build_library;
pub mod character_state;
pub mod compare;
pub mod game_data;
pub mod guide_profile;
pub mod item_parser;
pub mod ocr;
pub mod planner;
pub mod scorer;
pub mod sheet_reader;
pub mod text;

pub use character_state::{CharacterState, EquippedProfile, Phase};
pub use compare::{compare, Verdict, VerdictLabel};
pub use game_data::{GameData, Slot};
pub use guide_profile::GuideProfile;
pub use item_parser::{parse_tooltip, ParsedItem};
pub use planner::GearPlan;
pub use scorer::{score_item, ItemScore, ScoreContext};
