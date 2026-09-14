//! Saved builds (`profile/builds/*.json`): one file per Character + Build,
//! holding the pasted guide, its hash, and the AI-generated guide profile.
//! A guide is analysed once; pasting the same text again just reuses the
//! stored profile, a changed text triggers a new analysis.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::guide_profile::{Fact, GuideProfile};

/// The embedded Maxroll Paladin leveling profile; always listed, never deleted.
pub const BUILTIN_ID: &str = "builtin-paladin-leveling";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub id: String,
    pub character_name: String,
    pub build_name: String,
    /// URL or "pasted guide"
    #[serde(default)]
    pub source: String,
    pub guide_hash: String,
    #[serde(default)]
    pub guide_text: String,
    /// unix seconds
    pub created: u64,
    pub updated: u64,
    /// model that produced the profile
    #[serde(default)]
    pub model: String,
    /// the GuideProfile as pretty JSON (kept as text so it round-trips untouched)
    pub profile_json: String,
    #[serde(default)]
    pub notes: String,
}

impl Build {
    pub fn profile(&self) -> Result<GuideProfile> {
        GuideProfile::from_json(&self.profile_json).with_context(|| format!("profile of build {}", self.id))
    }
}

/// What the Builds window lists.
#[derive(Debug, Clone, Serialize)]
pub struct BuildSummary {
    pub id: String,
    pub character_name: String,
    pub build_name: String,
    pub source: String,
    pub updated: u64,
    pub model: String,
    pub profile_name: String,
    pub stat_count: usize,
    pub facts: Vec<Fact>,
    pub builtin: bool,
    pub active: bool,
    pub guide_hash: String,
}

pub struct BuildLibrary {
    dir: PathBuf,
}

impl BuildLibrary {
    pub fn new(profile_dir: &Path) -> BuildLibrary {
        BuildLibrary { dir: profile_dir.join("builds") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    fn active_path(&self) -> PathBuf {
        self.dir.join("active.txt")
    }

    /// Stable id from the character and build names.
    pub fn make_id(character_name: &str, build_name: &str) -> String {
        let slug = |s: &str| {
            let mut out = String::new();
            for c in s.trim().to_lowercase().chars() {
                if c.is_ascii_alphanumeric() {
                    out.push(c);
                } else if !out.ends_with('-') && !out.is_empty() {
                    out.push('-');
                }
            }
            out.trim_end_matches('-').to_string()
        };
        let id = format!("{}--{}", slug(character_name), slug(build_name));
        if id == "--" { "build".into() } else { id }
    }

    /// FNV-1a of the whitespace-normalised text, so re-pasting the same page
    /// (with different line wrapping) does not trigger a new analysis.
    pub fn hash(text: &str) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for word in text.split_whitespace() {
            for b in word.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h ^= b' ' as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    pub fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }

    pub fn get(&self, id: &str) -> Option<Build> {
        if id == BUILTIN_ID {
            return Some(builtin_build());
        }
        let text = std::fs::read_to_string(self.path(id)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Stored builds, builtin first, then most recently updated first.
    pub fn list(&self) -> Vec<BuildSummary> {
        let active = self.active_id();
        let mut builds: Vec<Build> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        if let Ok(build) = serde_json::from_str::<Build>(&text) {
                            builds.push(build);
                        }
                    }
                }
            }
        }
        builds.sort_by(|a, b| b.updated.cmp(&a.updated));
        let mut out = vec![summarise(&builtin_build(), true, active == BUILTIN_ID)];
        out.extend(builds.iter().map(|b| summarise(b, false, b.id == active)));
        out
    }

    pub fn find(&self, character_name: &str, build_name: &str) -> Option<Build> {
        let id = Self::make_id(character_name, build_name);
        if id == BUILTIN_ID { None } else { self.get(&id) }
    }

    pub fn save(&self, build: &Build) -> Result<()> {
        if build.id == BUILTIN_ID {
            bail!("the built-in profile cannot be overwritten");
        }
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.path(&build.id), serde_json::to_string_pretty(build)?)?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if id == BUILTIN_ID {
            bail!("the built-in profile cannot be deleted");
        }
        std::fs::remove_file(self.path(id)).with_context(|| format!("deleting build {id}"))?;
        if self.active_id() == id {
            self.set_active(BUILTIN_ID)?;
        }
        Ok(())
    }

    pub fn active_id(&self) -> String {
        std::fs::read_to_string(self.active_path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && (s == BUILTIN_ID || self.path(s).exists()))
            .unwrap_or_else(|| BUILTIN_ID.to_string())
    }

    pub fn set_active(&self, id: &str) -> Result<()> {
        if id != BUILTIN_ID && !self.path(id).exists() {
            bail!("no build {id:?}");
        }
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.active_path(), id)?;
        Ok(())
    }

    /// The active build's profile; the built-in one when the stored profile
    /// is missing or broken (the error is returned alongside for the log).
    pub fn active_profile(&self) -> (String, GuideProfile, Option<String>) {
        let id = self.active_id();
        if id == BUILTIN_ID {
            return (id, GuideProfile::embedded(), None);
        }
        match self.get(&id).ok_or_else(|| anyhow::anyhow!("build file missing")).and_then(|b| b.profile()) {
            Ok(profile) => (id, profile, None),
            Err(err) => (BUILTIN_ID.to_string(), GuideProfile::embedded(), Some(format!("build {id}: {err}; using the built-in profile"))),
        }
    }
}

fn builtin_build() -> Build {
    let profile = GuideProfile::embedded();
    Build {
        id: BUILTIN_ID.into(),
        character_name: "(any Paladin)".into(),
        build_name: "Maxroll Paladin Leveling".into(),
        source: profile.source.clone(),
        guide_hash: String::new(),
        guide_text: String::new(),
        created: 0,
        updated: 0,
        model: "built-in".into(),
        profile_json: crate::guide_profile::embedded_json().to_string(),
        notes: String::new(),
    }
}

fn summarise(build: &Build, builtin: bool, active: bool) -> BuildSummary {
    let profile = build.profile().ok();
    BuildSummary {
        id: build.id.clone(),
        character_name: build.character_name.clone(),
        build_name: build.build_name.clone(),
        source: build.source.clone(),
        updated: build.updated,
        model: build.model.clone(),
        profile_name: profile.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "(profile does not load)".into()),
        stat_count: profile.as_ref().map(|p| p.stats.len()).unwrap_or(0),
        facts: profile.map(|p| p.facts).unwrap_or_default(),
        builtin,
        active,
        guide_hash: build.guide_hash.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_ignores_whitespace_layout() {
        assert_eq!(BuildLibrary::hash("a  b\nc"), BuildLibrary::hash("a b c"));
        assert_ne!(BuildLibrary::hash("a b c"), BuildLibrary::hash("a b d"));
    }

    #[test]
    fn ids_are_slugs() {
        assert_eq!(BuildLibrary::make_id("My Pally!", "Maxroll Paladin (1.4)"), "my-pally--maxroll-paladin-1-4");
    }

    #[test]
    fn library_round_trip() {
        let dir = std::env::temp_dir().join(format!("le-builds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = BuildLibrary::new(&dir);
        assert_eq!(lib.active_id(), BUILTIN_ID);
        assert_eq!(lib.list().len(), 1);
        let build = Build {
            id: BuildLibrary::make_id("Pally", "Test"),
            character_name: "Pally".into(),
            build_name: "Test".into(),
            source: "pasted guide".into(),
            guide_hash: BuildLibrary::hash("guide"),
            guide_text: "guide".into(),
            created: 1,
            updated: 1,
            model: "test".into(),
            profile_json: crate::guide_profile::embedded_json().to_string(),
            notes: String::new(),
        };
        lib.save(&build).unwrap();
        lib.set_active(&build.id).unwrap();
        assert_eq!(lib.list().len(), 2);
        assert!(lib.list()[1].active);
        assert!(lib.find("Pally", "Test").is_some());
        let (id, profile, err) = lib.active_profile();
        assert_eq!(id, build.id);
        assert!(err.is_none());
        assert!(!profile.stats.is_empty());
        lib.delete(&build.id).unwrap();
        assert_eq!(lib.active_id(), BUILTIN_ID);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
