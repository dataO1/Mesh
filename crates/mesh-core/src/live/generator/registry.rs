//! Builds a lane generator from its style spec.
//!
//! Generator names a `style.yaml` may use (`generator.name`), for either
//! `family`:
//!
//! | Name | Generator | Performance parameters |
//! |---|---|---|
//! | `drums` | [`DrumGenerator`] | `style`, `density`, `overlay_hats`, `overlay_ghost`, `overlay_perc`, `variation`, `chop`, `swing` |
//! | `bass` | [`BassGenerator`] | `density`, `complexity`, `variation`, `kick_mode`, `register`, `slide`, `accent`, `gate` |
//! | `lead` (alias `motif`), `arp`, `stab`, `pad` | [`LeadGenerator`] presets | `density`, `complexity`, `variation`, `mode`, `register`, `rate`, `gate` |
//! | `fixed` | empty [`FixedPattern`] | none |
//! | `phrase` | [`PhraseGenerator`] (a sound pack's MIDI phrases) | `density`, `length`, `expression`, `tone` |
//!
//! `phrase` built here has no phrases and stays silent; the loader gives each
//! sound-pack option its own generator with the pack's phrases.
//!
//! `generator.params` sets the default of any performance parameter and the
//! construction options each generator documents (`blend`, `break_layer`,
//! `rule_a2` … `rule_a7`).
//!
//! `family: procedural` uses the built-in tables. `family: idiom` reads the
//! spec's `idiom` data, see [`super::idiom`].
//!
//! The drum generator needs the lane's target layout and archetype order, so
//! the engine should build lanes with [`build_lane`]. [`build`] has only the
//! generator spec and falls back to [`DrumLayout::standard`] and the built-in
//! archetype order.

use super::bass::{self, BassGenerator, BassRules};
use super::blocks::defaults_from_spec;
use super::drums::{self, DrumGenerator, DrumRules};
use super::fixed::FixedPattern;
use super::idiom;
use super::layout::DrumLayout;
use super::lead::{self, LeadGenerator, LeadKind, LeadRules};
use super::phrase::{self, PhraseGenerator};
use super::LaneGenerator;
use crate::live::glue::Role;
use crate::live::motif::Motif;
use crate::live::style::{GeneratorFamily, GeneratorSpec, IdiomSpec, LaneSpec};

/// Which implementation a generator name selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorKind {
    Drums,
    Bass,
    Lead(LeadKind),
    Fixed,
    Phrase,
}

impl GeneratorKind {
    /// Resolve a style generator name. `None` for unknown names.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "drums" => GeneratorKind::Drums,
            "bass" => GeneratorKind::Bass,
            "lead" | "motif" => GeneratorKind::Lead(LeadKind::Lead),
            "arp" => GeneratorKind::Lead(LeadKind::Arp),
            "stab" => GeneratorKind::Lead(LeadKind::Stab),
            "pad" => GeneratorKind::Lead(LeadKind::Pad),
            "fixed" => GeneratorKind::Fixed,
            "phrase" => GeneratorKind::Phrase,
            _ => return None,
        })
    }

    /// The generator a lane of this role gets when its name is unknown.
    pub fn default_for(role: Role) -> Self {
        match role {
            Role::Drums => GeneratorKind::Drums,
            Role::Bass => GeneratorKind::Bass,
            Role::Lead => GeneratorKind::Lead(LeadKind::Lead),
            Role::Extra => GeneratorKind::Lead(LeadKind::Pad),
        }
    }

    /// Resolve `name`, falling back to the role default with a warning.
    pub fn resolve(name: &str, role: Role) -> Self {
        Self::from_name(name).unwrap_or_else(|| {
            let fallback = Self::default_for(role);
            log::warn!("live: unknown generator '{name}' on a {role:?} lane, using {fallback:?}");
            fallback
        })
    }
}

/// The spec's idiom data when its family asks for it.
fn idiom_data(spec: &GeneratorSpec) -> Option<&IdiomSpec> {
    if spec.family != GeneratorFamily::Idiom {
        return None;
    }
    if spec.idiom.is_none() {
        log::warn!("live: generator '{}' is family idiom without idiom data, using built-in tables", spec.name);
    }
    spec.idiom.as_ref()
}

/// Build the generator named by `spec` for a lane of the given role.
///
/// Unknown names fall back to the role's default generator with a warning.
/// A drum generator built here uses [`DrumLayout::standard`] and the built-in
/// archetype order; use [`build_lane`] to match the style's kit.
pub fn build(spec: &GeneratorSpec, role: Role) -> Box<dyn LaneGenerator> {
    build_with(spec, role, &DrumLayout::standard(), &[])
}

/// Build a lane's generator, deriving the drum layout (default kit options)
/// and the archetype order from the lane.
pub fn build_lane(lane: &LaneSpec) -> Box<dyn LaneGenerator> {
    build_with(&lane.generator, lane.role, &DrumLayout::from_lane(lane), &lane.archetypes)
}

/// Build with an explicit drum layout and archetype order (both only used by
/// the drum generator). Use this after a kit swap changed the layout.
pub fn build_with(
    spec: &GeneratorSpec,
    role: Role,
    layout: &DrumLayout,
    archetypes: &[String],
) -> Box<dyn LaneGenerator> {
    build_with_motifs(spec, role, layout, archetypes, Vec::new())
}

/// [`build_lane`] with the lane's motif clips (bass and lead lanes play them).
pub fn build_lane_motifs(lane: &LaneSpec, motifs: Vec<Motif>) -> Box<dyn LaneGenerator> {
    build_with_motifs(&lane.generator, lane.role, &DrumLayout::from_lane(lane), &lane.archetypes, motifs)
}

/// [`build_with`] with motif clips for the bass and lead generators.
pub fn build_with_motifs(
    spec: &GeneratorSpec,
    role: Role,
    layout: &DrumLayout,
    archetypes: &[String],
    motifs: Vec<Motif>,
) -> Box<dyn LaneGenerator> {
    match GeneratorKind::resolve(&spec.name, role) {
        GeneratorKind::Drums => Box::new(build_drums(spec, layout, archetypes)),
        GeneratorKind::Bass => {
            let cells = idiom_data(spec).map(|i| idiom::cells(i, &["bass"])).unwrap_or_default();
            Box::new(BassGenerator::new(
                defaults_from_spec(&bass::PARAM_NAMES, &bass::PARAM_DEFAULTS, &spec.params),
                BassRules::from_spec(&spec.params),
                cells,
            ).with_motifs(motifs))
        }
        GeneratorKind::Lead(kind) => {
            let cells = idiom_data(spec).map(|i| idiom::cells(i, kind.voices())).unwrap_or_default();
            Box::new(LeadGenerator::new(
                kind,
                defaults_from_spec(&lead::PARAM_NAMES, &kind.defaults(), &spec.params),
                LeadRules::from_spec(&spec.params),
                cells,
            ).with_motifs(motifs))
        }
        GeneratorKind::Fixed => Box::new(FixedPattern::default()),
        GeneratorKind::Phrase => Box::new(build_phrase(spec, Vec::new(), None)),
    }
}

/// A `phrase` generator over `phrases`, with the spec's parameter defaults
/// and the pack's glide time.
pub fn build_phrase(spec: &GeneratorSpec, phrases: Vec<crate::live::phrase::Phrase>, glide: Option<f32>) -> PhraseGenerator {
    let g = PhraseGenerator::new(phrases).with_defaults(defaults_from_spec(&phrase::PARAM_NAMES, &phrase::PARAM_DEFAULTS, &spec.params));
    match glide {
        Some(secs) => g.with_glide(secs),
        None => g,
    }
}

/// Build the drum generator as its concrete type (the engine needs
/// [`DrumGenerator::layout`] for the onset mask).
pub fn build_drums(spec: &GeneratorSpec, layout: &DrumLayout, archetypes: &[String]) -> DrumGenerator {
    let (archetypes, unknown) = idiom::drum_archetypes(archetypes, idiom_data(spec));
    for name in unknown {
        log::warn!("live: unknown drum archetype '{name}' has no level maps and stays silent");
    }
    DrumGenerator::new(
        archetypes,
        *layout,
        defaults_from_spec(&drums::PARAM_NAMES, &drums::PARAM_DEFAULTS, &spec.params),
        DrumRules::from_spec(&spec.params),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::event::EventKind;
    use crate::live::generator::test_util::{assert_well_formed, ctx, render};
    use crate::live::generator::MAX_GEN_PARAMS;
    use crate::live::style::{LevelMapSpec, StyleSpec};
    use std::collections::BTreeMap;

    const EMPTY: [u8; 16] = [0; 16];

    fn spec(family: GeneratorFamily, name: &str, params: &[(&str, f32)]) -> GeneratorSpec {
        GeneratorSpec {
            family,
            name: name.to_string(),
            params: params.iter().map(|(k, v)| (k.to_string(), *v)).collect::<BTreeMap<_, _>>(),
            idiom: None,
        }
    }

    fn fixture() -> StyleSpec {
        StyleSpec::from_yaml(include_str!("../../../tests/fixtures/live-style/style.yaml")).unwrap()
    }

    #[test]
    fn names_resolve() {
        use GeneratorKind::*;
        assert_eq!(GeneratorKind::from_name("drums"), Some(Drums));
        assert_eq!(GeneratorKind::from_name(" Bass "), Some(Bass));
        assert_eq!(GeneratorKind::from_name("motif"), Some(Lead(LeadKind::Lead)));
        assert_eq!(GeneratorKind::from_name("arp"), Some(Lead(LeadKind::Arp)));
        assert_eq!(GeneratorKind::from_name("stab"), Some(Lead(LeadKind::Stab)));
        assert_eq!(GeneratorKind::from_name("pad"), Some(Lead(LeadKind::Pad)));
        assert_eq!(GeneratorKind::from_name("fixed"), Some(Fixed));
        assert_eq!(GeneratorKind::from_name("nope"), None);
        // Unknown names fall back by role.
        assert_eq!(GeneratorKind::resolve("nope", Role::Drums), Drums);
        assert_eq!(GeneratorKind::resolve("nope", Role::Bass), Bass);
        assert_eq!(GeneratorKind::resolve("nope", Role::Lead), Lead(LeadKind::Lead));
        assert_eq!(GeneratorKind::resolve("nope", Role::Extra), Lead(LeadKind::Pad));
    }

    #[test]
    fn param_names_and_defaults_from_the_spec() {
        let g = build(&spec(GeneratorFamily::Procedural, "drums", &[("style", 0.4), ("swing", 0.1)]), Role::Drums);
        assert_eq!(g.param_names(), drums::PARAM_NAMES);
        let d = g.param_defaults();
        assert_eq!((d[0], d[1], d[7]), (0.4, drums::PARAM_DEFAULTS[1], 0.1));

        let g = build(&spec(GeneratorFamily::Procedural, "bass", &[("complexity", 0.9)]), Role::Bass);
        assert_eq!(g.param_names(), bass::PARAM_NAMES);
        assert_eq!(g.param_defaults()[1], 0.9);

        let g = build(&spec(GeneratorFamily::Procedural, "arp", &[("rate", 1.0)]), Role::Lead);
        assert_eq!(g.param_names(), lead::PARAM_NAMES);
        assert_eq!(g.param_defaults()[5], 1.0);
        assert_eq!(g.param_defaults()[3], LeadKind::Arp.defaults()[3]);

        let g = build(&spec(GeneratorFamily::Procedural, "fixed", &[]), Role::Lead);
        assert!(g.param_names().is_empty());
        for name in ["drums", "bass", "lead", "arp", "stab", "pad"] {
            let g = build(&spec(GeneratorFamily::Procedural, name, &[]), Role::Extra);
            assert!(g.param_names().len() <= MAX_GEN_PARAMS);
        }
    }

    /// Render a bar with the generator's own defaults.
    fn default_bar(g: &dyn LaneGenerator, lane: u8, mask: &[u8; 16]) -> crate::live::event::BarEvents {
        let p = g.param_defaults();
        let mut c = ctx(1, 0, &p, mask);
        c.lane = lane;
        let out = render(g, &c);
        assert_well_formed(&out);
        out
    }

    #[test]
    fn unknown_names_get_a_working_role_default() {
        for (role, kind) in [
            (Role::Drums, EventKind::Trigger),
            (Role::Bass, EventKind::NoteOn),
            (Role::Lead, EventKind::NoteOn),
            (Role::Extra, EventKind::NoteOn),
        ] {
            let g = build(&spec(GeneratorFamily::Procedural, "does-not-exist", &[]), role);
            let out = default_bar(g.as_ref(), 0, &EMPTY);
            assert!(!out.is_empty(), "{role:?}");
            assert!(out.as_slice().iter().all(|e| e.kind == kind));
        }
    }

    #[test]
    fn every_fixture_lane_builds_and_renders() {
        let style = fixture();
        let drums = build_lane(&style.lanes[0]);
        let drum_bar = default_bar(drums.as_ref(), 0, &EMPTY);
        assert!(!drum_bar.is_empty());
        let layout = DrumLayout::from_lane(&style.lanes[0]);
        let mask = layout.onset_mask(drum_bar.as_slice());
        assert!(mask.iter().any(|&v| v > 0));
        // Only targets the fixture kit has.
        for e in drum_bar.as_slice() {
            assert_ne!(layout.class(e.target), crate::live::generator::layout::DrumClass::Unknown, "target {}", e.target);
        }
        for (i, lane) in style.lanes.iter().enumerate().skip(1) {
            let g = build_lane(lane);
            // Control targets of type generator name real parameters.
            for control in &lane.controls {
                if let crate::live::style::ControlTarget::Generator { param } = &control.target {
                    assert!(g.param_names().contains(&param.as_str()), "lane {i}: {param}");
                }
            }
            assert!(!default_bar(g.as_ref(), i as u8, &mask).is_empty(), "lane {i}");
        }
        for control in &style.lanes[0].controls {
            if let crate::live::style::ControlTarget::Generator { param } = &control.target {
                assert!(drums.param_names().contains(&param.as_str()));
            }
        }
    }

    #[test]
    fn fixture_pad_follows_its_idiom_map() {
        // Levels 255 / 128 / 64 on steps 0 / 8 / 14, density 0.3.
        let style = fixture();
        let g = build_lane(&style.lanes[3]);
        let steps = |density: f32| -> Vec<u16> {
            let mut p = g.param_defaults();
            p[0] = density;
            p[2] = 0.0;
            let mut c = ctx(1, 0, &p, &EMPTY);
            c.lane = 3;
            let mut s: Vec<u16> = render(g.as_ref(), &c).as_slice().iter().map(|e| e.tick / 24).collect();
            s.dedup();
            s
        };
        assert_eq!(g.param_defaults()[0], 0.3);
        assert_eq!(steps(0.3), [0]);
        assert_eq!(steps(0.6), [0, 8]);
        assert_eq!(steps(1.0), [0, 8, 14]);
    }

    #[test]
    fn idiom_drums_are_a_drop_in() {
        let mut kick = vec![0u8; 16];
        for s in [0, 3, 6] {
            kick[s] = 255;
        }
        let mut idiom_spec = spec(GeneratorFamily::Idiom, "drums", &[("style", 0.25), ("overlay_ghost", 0.0)]);
        idiom_spec.idiom = Some(IdiomSpec {
            level_maps: vec![LevelMapSpec {
                name: "tresillo".into(),
                archetype: Some("two_step".into()),
                voice: Some("kick".into()),
                levels: kick,
            }],
        });
        let layout = DrumLayout::empty().with_backbone(2);
        let idiom_gen = build_with(&idiom_spec, Role::Drums, &layout, &[]);
        let mut procedural_spec = idiom_spec.clone();
        procedural_spec.family = GeneratorFamily::Procedural;
        let procedural = build_with(&procedural_spec, Role::Drums, &layout, &[]);
        // Same parameters either way.
        assert_eq!(idiom_gen.param_names(), procedural.param_names());
        assert_eq!(idiom_gen.param_defaults(), procedural.param_defaults());
        let kicks = |g: &dyn LaneGenerator| -> Vec<u16> {
            default_bar(g, 0, &EMPTY).as_slice().iter().filter(|e| e.target == 0).map(|e| e.tick / 24).collect()
        };
        assert_eq!(kicks(idiom_gen.as_ref()), [0, 3, 6]);
        // The procedural family ignores the data.
        assert_eq!(kicks(procedural.as_ref()), [0, 10]);
        // Idiom family without data falls back to the built-in tables.
        let mut bare = idiom_spec.clone();
        bare.idiom = None;
        assert_eq!(kicks(build_with(&bare, Role::Drums, &layout, &[]).as_ref()), [0, 10]);
    }

    #[test]
    fn rules_come_from_the_spec() {
        // Rule A2 (the backbone lock) keeps percussion off the kick.
        let layout = DrumLayout::empty().with_backbone(2).with_perc(2);
        let perc_on_kick = |params: &[(&str, f32)]| {
            let g = build_with(&spec(GeneratorFamily::Procedural, "drums", params), Role::Drums, &layout, &[]);
            (0..40).any(|seed| {
                let ev = default_bar(g.as_ref(), seed, &EMPTY);
                ev.as_slice().iter().any(|e| e.target >= crate::live::generator::layout::PERC_BASE && e.tick == 0)
            })
        };
        assert!(!perc_on_kick(&[("density", 1.0)]));
        assert!(perc_on_kick(&[("density", 1.0), ("rule_a2", 0.0)]));
    }

    /// One generator of every kind.
    fn all_generators() -> Vec<(Role, Box<dyn LaneGenerator>)> {
        [
            ("drums", Role::Drums),
            ("bass", Role::Bass),
            ("lead", Role::Lead),
            ("arp", Role::Lead),
            ("stab", Role::Lead),
            ("pad", Role::Extra),
        ]
        .into_iter()
        .map(|(name, role)| (role, build(&spec(GeneratorFamily::Procedural, name, &[]), role)))
        .chain(motif_generators())
        .collect()
    }

    /// A bass and a lead that play the starter motif clips.
    fn motif_generators() -> Vec<(Role, Box<dyn LaneGenerator>)> {
        use crate::live::generator::test_util::starter_motifs;
        let (bass, lead): (Vec<_>, Vec<_>) = starter_motifs().into_iter().partition(|m| m.name.starts_with("bass-"));
        let layout = DrumLayout::standard();
        vec![
            (Role::Bass, build_with_motifs(&spec(GeneratorFamily::Procedural, "bass", &[]), Role::Bass, &layout, &[], bass)),
            (Role::Lead, build_with_motifs(&spec(GeneratorFamily::Procedural, "lead", &[]), Role::Lead, &layout, &[], lead)),
        ]
    }

    /// Render a phrase (normal and fill bars) of every generator with a busy
    /// parameter set. Returns the number of events.
    fn render_phrase(
        generators: &[(Role, Box<dyn LaneGenerator>)],
        seed: u32,
        out: &mut crate::live::event::BarEvents,
    ) -> usize {
        const MASK: [u8; 16] = [120, 0, 0, 0, 80, 0, 0, 0, 0, 0, 118, 0, 80, 0, 0, 0];
        let params = [0.8f32; MAX_GEN_PARAMS];
        let mut events = 0;
        for (lane, (_, g)) in generators.iter().enumerate() {
            for bar in 0..4u8 {
                for fill in [false, true] {
                    let mut c = ctx(seed, bar, &params, &MASK);
                    c.lane = lane as u8;
                    c.fill = fill;
                    out.clear();
                    g.render_bar(&c, out);
                    events += out.len();
                }
            }
        }
        events
    }

    #[test]
    #[cfg(debug_assertions)]
    fn render_bar_does_not_allocate() {
        let generators = all_generators();
        let mut out = crate::live::event::BarEvents::default();
        let (events, allocs) = crate::live::test_support::count_allocs(|| {
            (0..8).map(|seed| render_phrase(&generators, seed, &mut out)).sum::<usize>()
        });
        assert!(events > 1000);
        assert_eq!(allocs, 0, "render_bar allocated");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn motif_render_does_not_allocate() {
        // Motif clips are built at load time; rendering only reads them.
        let generators = motif_generators();
        let mut out = crate::live::event::BarEvents::default();
        let mut params = [0.7f32; MAX_GEN_PARAMS];
        params[3] = 0.0; // the lead's motif mode
        let (events, allocs) = crate::live::test_support::count_allocs(|| {
            let mut events = 0;
            for seed in 0..60u32 {
                for (lane, (_, g)) in generators.iter().enumerate() {
                    for bar in 0..4u8 {
                        let mut c = ctx(seed, bar, &params, &[0; 16]);
                        c.lane = lane as u8 + 1;
                        c.bass_phrase = [0x1111, 0x0101, 0x4444, 0x1001];
                        c.bass_onsets = 0x1111;
                        out.clear();
                        g.render_bar(&c, &mut out);
                        events += out.len();
                    }
                }
            }
            events
        });
        assert!(events > 500, "{events}");
        assert_eq!(allocs, 0, "motif render_bar allocated");
    }

    #[test]
    fn a_bar_renders_well_under_a_millisecond() {
        // Holds in debug builds. Best of several runs, so a busy test machine
        // does not fail it.
        let generators = all_generators();
        let mut out = crate::live::event::BarEvents::default();
        let bars = (generators.len() * 4 * 2) as u32;
        let best = (0..10)
            .map(|seed| {
                let start = std::time::Instant::now();
                render_phrase(&generators, seed, &mut out);
                start.elapsed() / bars
            })
            .min()
            .unwrap();
        assert!(best < std::time::Duration::from_micros(250), "{best:?} per bar");
    }

    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored bench_render_bar --nocapture"]
    fn bench_render_bar() {
        let mut out = crate::live::event::BarEvents::default();
        for (role, g) in all_generators() {
            let one = vec![(role, g)];
            let rounds = 2000u32;
            let start = std::time::Instant::now();
            let events: usize = (0..rounds).map(|seed| render_phrase(&one, seed, &mut out)).sum();
            let per_bar = start.elapsed() / (rounds * 8);
            println!("{role:?}: {per_bar:?} per bar, {} events per bar", events / (rounds as usize * 8));
        }
    }

    #[test]
    fn lane_archetype_order_is_used() {
        let mut lane = fixture().lanes.remove(0);
        lane.archetypes = vec!["hardcore".into(), "half_time".into()];
        lane.generator.params.insert("style".into(), 0.0);
        let g = build_lane(&lane);
        let kicks: Vec<u16> =
            default_bar(g.as_ref(), 0, &EMPTY).as_slice().iter().filter(|e| e.target == 0).map(|e| e.tick / 24).collect();
        assert_eq!(kicks, [0, 4, 8, 12]);
    }
}
