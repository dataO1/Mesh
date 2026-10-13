//! Idiom-data family (design §6.2, D42): the fallback to procedural
//! generation.
//!
//! Idiom generators are the procedural generators with their importance
//! tables replaced by curated data from the style ([`IdiomSpec`]). Density
//! thresholds, velocity tiers, phrase shape, pitch pools and parameter names
//! are shared, so a style switches family without touching its controls.
//!
//! How level maps are read:
//!
//! - **Drum lane.** A map with `voice` `kick`, `snare`, `hats`, `ghost` or
//!   `perc` replaces that table of the archetype named in `archetype`. A map
//!   without `archetype` applies to every archetype of the lane; an
//!   archetype-specific map wins over it. Tables without a map keep the
//!   built-in values. An archetype name that is neither built in nor covered
//!   by maps is silent.
//! - **Instrument lanes.** Every map whose `voice` is unset or names the
//!   generator (`bass`; `lead` / `motif`; `arp`; `stab`; `pad`) is one rhythm
//!   cell. The seed picks the cell; `density` reveals its steps by level.
//!
//! Maps longer than one bar (32, 48, 64 steps) advance with the bar in the
//! phrase.

use super::blocks::LevelMap;
use super::drums::Archetype;
use crate::live::style::{IdiomSpec, LevelMapSpec};

/// Drum voices a level map can drive.
const VOICE_KICK: &str = "kick";
const VOICE_SNARE: &str = "snare";
const VOICE_HATS: [&str; 2] = ["hats", "hat"];
const VOICE_GHOST: [&str; 2] = ["ghost", "ghosts"];
const VOICE_PERC: &str = "perc";

fn voice_is(map: &LevelMapSpec, names: &[&str]) -> bool {
    map.voice.as_deref().is_some_and(|v| names.iter().any(|n| v.eq_ignore_ascii_case(n)))
}

/// Rhythm cells for an instrument lane: all non-empty maps whose voice is
/// unset or one of `voices`.
pub fn cells(idiom: &IdiomSpec, voices: &[&str]) -> Vec<LevelMap> {
    idiom
        .level_maps
        .iter()
        .filter(|m| m.voice.is_none() || voice_is(m, voices))
        .map(|m| LevelMap::from_slice(&m.levels))
        .filter(|m| !m.is_empty())
        .collect()
}

/// Apply every map of `idiom` that targets `arch`: generic maps first, then
/// the archetype's own. Returns whether one of its own maps applied.
fn apply_maps(arch: &mut Archetype, idiom: &IdiomSpec) -> bool {
    let mut any = false;
    for specific in [false, true] {
        for map in &idiom.level_maps {
            let matches = match &map.archetype {
                None => !specific,
                Some(a) => specific && a.eq_ignore_ascii_case(&arch.name),
            };
            if !matches {
                continue;
            }
            let levels = LevelMap::from_slice(&map.levels);
            if voice_is(map, &[VOICE_KICK]) {
                arch.kick = levels;
            } else if voice_is(map, &[VOICE_SNARE]) {
                arch.snare = levels;
            } else if voice_is(map, &VOICE_HATS) {
                arch.hats = levels;
            } else if voice_is(map, &VOICE_GHOST) {
                arch.ghost = levels;
            } else if voice_is(map, &[VOICE_PERC]) {
                arch.perc = Some(levels);
            } else {
                continue;
            }
            any |= specific;
        }
    }
    any
}

/// The drum lane's archetypes in major-style order.
///
/// `order` are the lane's archetype names (empty = the built-in order). With
/// `idiom` data the maps override the built-in tables as described in the
/// module docs. Names that resolve to nothing are reported in the second
/// return value and render silence.
pub fn drum_archetypes(order: &[String], idiom: Option<&IdiomSpec>) -> (Vec<Archetype>, Vec<String>) {
    let mut unknown = Vec::new();
    let names: Vec<&str> = if order.is_empty() {
        super::drums::DEFAULT_ARCHETYPE_ORDER.to_vec()
    } else {
        order.iter().map(String::as_str).collect()
    };
    let archetypes = names
        .into_iter()
        .map(|name| {
            let builtin = Archetype::builtin(name);
            let known = builtin.is_some();
            let mut arch = builtin.unwrap_or_else(|| Archetype::blank(name));
            let mapped = idiom.is_some_and(|i| apply_maps(&mut arch, i));
            if !known && !mapped {
                unknown.push(name.to_string());
            }
            arch
        })
        .collect();
    (archetypes, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(name: &str, archetype: Option<&str>, voice: Option<&str>, levels: Vec<u8>) -> LevelMapSpec {
        LevelMapSpec {
            name: name.to_string(),
            archetype: archetype.map(str::to_string),
            voice: voice.map(str::to_string),
            levels,
        }
    }

    #[test]
    fn cells_filter_by_voice() {
        let idiom = IdiomSpec {
            level_maps: vec![
                map("a", None, None, vec![255; 16]),
                map("b", None, Some("Bass"), vec![200; 16]),
                map("c", None, Some("lead"), vec![100; 16]),
                map("empty", None, Some("bass"), vec![0; 16]),
                map("long", None, Some("bass"), vec![9; 32]),
            ],
        };
        let bass = cells(&idiom, &["bass"]);
        assert_eq!(bass.len(), 3);
        assert_eq!(bass[1].at(0, 0), 200);
        assert_eq!(bass[2].len(), 32);
        assert_eq!(cells(&idiom, &["lead", "motif"]).len(), 2);
        assert_eq!(cells(&idiom, &["pad"]).len(), 1);
    }

    #[test]
    fn builtin_order_without_data() {
        let (archetypes, unknown) = drum_archetypes(&[], None);
        let names: Vec<&str> = archetypes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, super::super::drums::DEFAULT_ARCHETYPE_ORDER);
        assert!(unknown.is_empty());
        assert_eq!(archetypes, Archetype::default_order());
    }

    #[test]
    fn maps_override_tables() {
        let mut kick = vec![0u8; 16];
        kick[0] = 255;
        kick[6] = 255;
        let idiom = IdiomSpec {
            level_maps: vec![
                map("all-hats", None, Some("hats"), vec![77; 16]),
                map("ts-hats", Some("two_step"), Some("hats"), vec![99; 16]),
                map("ts-kick", Some("Two_Step"), Some("kick"), kick),
                map("mine-snare", Some("mine"), Some("snare"), vec![255; 16]),
                map("perc", None, Some("perc"), vec![50; 16]),
                map("ignored", Some("two_step"), Some("cowbell"), vec![1; 16]),
            ],
        };
        let order: Vec<String> = ["two_step", "jungle", "mine", "nothing"].iter().map(|s| s.to_string()).collect();
        let (archetypes, unknown) = drum_archetypes(&order, Some(&idiom));
        assert_eq!(unknown, ["nothing"]);
        let builtin = Archetype::builtin("two_step").unwrap();
        // The archetype's own map wins over the generic one.
        assert_eq!(archetypes[0].hats.at(0, 0), 99);
        assert_eq!(archetypes[0].kick.at(0, 6), 255);
        assert_eq!(archetypes[0].kick.at(0, 10), 0);
        // Untouched tables keep the built-in values.
        assert_eq!(archetypes[0].snare, builtin.snare);
        assert_eq!(archetypes[1].hats.at(0, 0), 77);
        assert_eq!(archetypes[1].kick, Archetype::builtin("jungle").unwrap().kick);
        assert_eq!(archetypes[1].perc.unwrap().at(0, 3), 50);
        // A new archetype defined only by data.
        assert_eq!(archetypes[2].snare.at(0, 9), 255);
        assert!(archetypes[2].kick.is_empty());
        // Generic maps do not make an unknown name known... but still apply.
        assert_eq!(archetypes[3].hats.at(0, 0), 77);
    }
}
