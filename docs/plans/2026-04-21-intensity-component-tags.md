# Intensity Component Tags Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Show per-track intensity tags (Choppy/Smooth, Gritty/Clean, Dense/Punchy, Bright/Dark) derived from individual IntensityComponents, displayed as colored pills using the "Other" stem color from the active iced theme.

**Architecture:** Four tag groups combine the 7 raw IntensityComponents into musically meaningful axes. Tags are generated at two levels: (1) in suggestion scoring, relative to other candidates (top/bottom 20% of deltas from seed); (2) in the mesh-cue browser, relative to the whole library (top/bottom 20% of absolute values). A new sentinel color `TAG_COLOR_INTENSITY` maps to the Other stem color via `resolve_tag_color()`. Max 2 tags per track.

**Tech Stack:** Rust, mesh-core (scoring), mesh-widgets (TrackTag rendering), mesh-cue (domain enrichment)

---

## Tag Group Definitions

| Group | Components (weighted avg within group) | High label | Low label |
|-------|---------------------------------------|------------|-----------|
| **Texture** | 0.25\*flux + 0.10\*energy_variance → normalize by 0.35 | Choppy | Smooth |
| **Grit** | 0.20\*flatness + 0.15\*dissonance + 0.05\*(1-harmonic_complexity) → normalize by 0.40 | Gritty | Clean |
| **Density** | 0.10\*(1-crest_factor) → normalize by 0.10 | Dense | Punchy |
| **Brightness** | 0.15\*spectral_centroid → normalize by 0.15 | Bright | Dark |

Each group value is in [0, 1]. Higher = more aggressive on that axis.

---

### Task 1: Add intensity tag group computation to scoring.rs

**Files:**
- Modify: `crates/mesh-core/src/suggestions/scoring.rs`

**Step 1: Add TAG_COLOR_INTENSITY sentinel and IntensityTagGroup enum**

After the existing `TAG_COLOR_POOR` constant (~line 393), add:

```rust
/// Intensity component tag — maps to Other stem color in theme
pub const TAG_COLOR_INTENSITY: &str = "#00AA04";

/// The four intensity tag groups derived from IntensityComponents.
#[derive(Debug, Clone, Copy)]
pub enum IntensityTagGroup {
    Texture,
    Grit,
    Density,
    Brightness,
}

impl IntensityTagGroup {
    /// Compute the group value from raw intensity components.
    /// Returns a value in [0, 1] where higher = more aggressive.
    pub fn value(&self, ic: &crate::db::IntensityComponents) -> f32 {
        match self {
            Self::Texture => {
                (0.25 * ic.spectral_flux + 0.10 * ic.energy_variance) / 0.35
            }
            Self::Grit => {
                (0.20 * ic.flatness + 0.15 * ic.dissonance
                 + 0.05 * (1.0 - ic.harmonic_complexity)) / 0.40
            }
            Self::Density => {
                1.0 - ic.crest_factor
            }
            Self::Brightness => {
                ic.spectral_centroid
            }
        }
    }

    /// Human-readable label when candidate is MORE intense than reference.
    pub fn high_label(&self) -> &'static str {
        match self {
            Self::Texture => "Choppy",
            Self::Grit => "Gritty",
            Self::Density => "Dense",
            Self::Brightness => "Bright",
        }
    }

    /// Human-readable label when candidate is LESS intense than reference.
    pub fn low_label(&self) -> &'static str {
        match self {
            Self::Texture => "Smooth",
            Self::Grit => "Clean",
            Self::Density => "Punchy",
            Self::Brightness => "Dark",
        }
    }

    pub const ALL: [IntensityTagGroup; 4] = [
        Self::Texture, Self::Grit, Self::Density, Self::Brightness,
    ];
}
```

**Step 2: Add function to generate intensity tags from component deltas**

Below the `IntensityTagGroup` impl, add:

```rust
/// Generate up to 2 intensity component tags for a candidate track.
///
/// **Suggestion mode** (seed_ic is Some): Tags show direction relative to seed.
/// Only shown when this track's delta is an outlier (top/bottom 20%) among
/// all candidate deltas for that group.
///
/// `group_percentiles` contains (p20, p80) thresholds for each group's delta
/// across all candidates. A delta below p20 or above p80 is an outlier.
pub fn generate_intensity_tags(
    cand_ic: &crate::db::IntensityComponents,
    seed_ic: &crate::db::IntensityComponents,
    group_percentiles: &[(f32, f32); 4],  // (p20, p80) per group, ordered as ALL
) -> Vec<(String, Option<String>)> {
    let mut tags: Vec<(String, Option<String>, f32)> = Vec::new();

    for (i, group) in IntensityTagGroup::ALL.iter().enumerate() {
        let cand_val = group.value(cand_ic);
        let seed_val = group.value(seed_ic);
        let delta = cand_val - seed_val;
        let (p20, p80) = group_percentiles[i];

        if delta > p80 {
            tags.push((group.high_label().to_string(), Some(TAG_COLOR_INTENSITY.to_string()), delta.abs()));
        } else if delta < p20 {
            tags.push((group.low_label().to_string(), Some(TAG_COLOR_INTENSITY.to_string()), delta.abs()));
        }
    }

    // Keep max 2 tags, sorted by largest absolute delta
    tags.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    tags.truncate(2);
    tags.into_iter().map(|(label, color, _)| (label, color)).collect()
}

/// Generate up to 2 intensity tags for browser display (absolute, no seed).
///
/// Tags show when a track is in the top/bottom 20% of the library for a group.
/// `library_percentiles` contains (p20, p80) for each group across the whole collection.
pub fn generate_intensity_tags_absolute(
    ic: &crate::db::IntensityComponents,
    library_percentiles: &[(f32, f32); 4],
) -> Vec<(String, Option<String>)> {
    let mut tags: Vec<(String, Option<String>, f32)> = Vec::new();

    for (i, group) in IntensityTagGroup::ALL.iter().enumerate() {
        let val = group.value(ic);
        let (p20, p80) = library_percentiles[i];

        // Distance from the nearer threshold — measures how extreme this value is
        if val > p80 {
            tags.push((group.high_label().to_string(), Some(TAG_COLOR_INTENSITY.to_string()), val - p80));
        } else if val < p20 {
            tags.push((group.low_label().to_string(), Some(TAG_COLOR_INTENSITY.to_string()), p20 - val));
        }
    }

    tags.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    tags.truncate(2);
    tags.into_iter().map(|(label, color, _)| (label, color)).collect()
}

/// Compute (p20, p80) percentile thresholds for each intensity tag group
/// from a collection of IntensityComponents.
pub fn compute_intensity_percentiles(
    components: &[&crate::db::IntensityComponents],
) -> [(f32, f32); 4] {
    let mut result = [(0.0f32, 1.0f32); 4];
    if components.is_empty() { return result; }

    for (i, group) in IntensityTagGroup::ALL.iter().enumerate() {
        let mut values: Vec<f32> = components.iter().map(|ic| group.value(ic)).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = values.len();
        let p20 = values[n / 5];          // 20th percentile
        let p80 = values[n * 4 / 5];      // 80th percentile
        result[i] = (p20, p80);
    }
    result
}
```

**Step 3: Commit**

```bash
git add crates/mesh-core/src/suggestions/scoring.rs
git commit -m "feat: intensity tag group computation (Texture/Grit/Density/Brightness)"
```

---

### Task 2: Wire intensity tags into suggestion scoring

**Files:**
- Modify: `crates/mesh-core/src/suggestions/query.rs`

**Step 1: Store raw IntensityComponents alongside composite score**

Change the `intensity_map` from `HashMap<(usize, i64), f32>` to store both
composite AND raw components. The cleanest approach: add a parallel map.

After the existing `intensity_map` construction (~line 441), add:

```rust
// Also keep raw components for per-group tag generation
let mut intensity_components_map: HashMap<(usize, i64), crate::db::IntensityComponents> = HashMap::new();
for (src_idx, source) in sources.iter().enumerate() {
    let all_ids: Vec<i64> = candidates.keys()
        .filter(|(si, _)| *si == src_idx)
        .map(|(_, id)| *id)
        .collect();
    if all_ids.is_empty() { continue; }
    if let Ok(components) = source.db.batch_get_intensity_components(&all_ids) {
        for (id, ic) in components {
            intensity_components_map.insert((src_idx, id), ic);
        }
    }
}
```

Wait — `batch_get_intensity_components` is already called to build `intensity_map`. Refactor to avoid the double fetch: store the raw components, derive the composite from them.

Replace the existing intensity_map block with:

```rust
let mut intensity_components_map: HashMap<(usize, i64), crate::db::IntensityComponents> = HashMap::new();
let mut intensity_map: HashMap<(usize, i64), f32> = HashMap::new();
for (src_idx, source) in sources.iter().enumerate() {
    let all_ids: Vec<i64> = candidates.keys()
        .filter(|(si, _)| *si == src_idx)
        .map(|(_, id)| *id)
        .collect();
    if all_ids.is_empty() { continue; }
    if let Ok(components) = source.db.batch_get_intensity_components(&all_ids) {
        for (id, ic) in components {
            intensity_map.insert((src_idx, id), composite_intensity_v2(&ic));
            intensity_components_map.insert((src_idx, id), ic);
        }
    }
}
```

**Step 2: Compute seed average IntensityComponents**

After `avg_seed_intensity`, add:

```rust
let avg_seed_ic: crate::db::IntensityComponents = {
    let seed_ics: Vec<&crate::db::IntensityComponents> = seed_tracks.iter()
        .filter_map(|(idx, t)| t.id.map(|id| (*idx, id)))
        .filter_map(|key| intensity_components_map.get(&key))
        .collect();
    if seed_ics.is_empty() {
        crate::db::IntensityComponents::default()  // all 0.0 → neutral
    } else {
        let n = seed_ics.len() as f32;
        crate::db::IntensityComponents {
            spectral_flux: seed_ics.iter().map(|ic| ic.spectral_flux).sum::<f32>() / n,
            flatness: seed_ics.iter().map(|ic| ic.flatness).sum::<f32>() / n,
            spectral_centroid: seed_ics.iter().map(|ic| ic.spectral_centroid).sum::<f32>() / n,
            dissonance: seed_ics.iter().map(|ic| ic.dissonance).sum::<f32>() / n,
            crest_factor: seed_ics.iter().map(|ic| ic.crest_factor).sum::<f32>() / n,
            energy_variance: seed_ics.iter().map(|ic| ic.energy_variance).sum::<f32>() / n,
            harmonic_complexity: seed_ics.iter().map(|ic| ic.harmonic_complexity).sum::<f32>() / n,
            spectral_rolloff: seed_ics.iter().map(|ic| ic.spectral_rolloff).sum::<f32>() / n,
        }
    }
};
```

**Step 3: Compute group delta percentiles across all candidates**

Before the scoring loop, add:

```rust
let intensity_group_percentiles: [(f32, f32); 4] = {
    // Collect deltas from seed for each group
    let cand_ics: Vec<&crate::db::IntensityComponents> = candidates.keys()
        .filter_map(|key| intensity_components_map.get(key))
        .collect();
    if cand_ics.len() < 5 {
        // Too few candidates for meaningful percentiles — no tags
        [(f32::MIN, f32::MAX); 4]
    } else {
        let mut result = [(0.0f32, 1.0f32); 4];
        for (i, group) in IntensityTagGroup::ALL.iter().enumerate() {
            let seed_val = group.value(&avg_seed_ic);
            let mut deltas: Vec<f32> = cand_ics.iter()
                .map(|ic| group.value(ic) - seed_val)
                .collect();
            deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = deltas.len();
            result[i] = (deltas[n / 5], deltas[n * 4 / 5]);
        }
        result
    }
};
```

**Step 4: Append intensity tags in the scoring closure**

Inside the `candidates.into_iter().filter_map(...)` closure, after
`generate_reason_tags()` and the multi-source tag, append intensity tags:

```rust
// Intensity component tags (only for candidates with raw components)
if let Some(cand_ic) = intensity_components_map.get(&(src_idx, track_id)) {
    let int_tags = generate_intensity_tags(cand_ic, &avg_seed_ic, &intensity_group_percentiles);
    for tag in int_tags {
        reason_tags.push(tag);
    }
}
```

**Step 5: Commit**

```bash
git add crates/mesh-core/src/suggestions/query.rs
git commit -m "feat: wire intensity component tags into suggestion scoring"
```

---

### Task 3: Map TAG_COLOR_INTENSITY sentinel to Other stem color

**Files:**
- Modify: `crates/mesh-widgets/src/track_table/mod.rs`

**Step 1: Add sentinel mapping**

In `resolve_tag_color()` (~line 92), add the new sentinel before the catch-all:

```rust
        (0, 170, 3) => cats[0],  // TAG_COLOR_SOURCE → Drums stem
        (0, 170, 4) => cats[2],  // TAG_COLOR_INTENSITY → Other stem
        // Score-based suggestion poor (#a63d40) + custom — keep as-is
```

**Step 2: Commit**

```bash
git add crates/mesh-widgets/src/track_table/mod.rs
git commit -m "feat: map TAG_COLOR_INTENSITY sentinel to Other stem color"
```

---

### Task 4: Add intensity tags to mesh-cue browser (library percentiles)

**Files:**
- Modify: `crates/mesh-cue/src/domain/mod.rs`

**Step 1: Compute library-wide percentiles and generate tags in `enrich_with_intensity()`**

The existing `enrich_with_intensity()` already fetches `batch_get_intensity_components()`.
Extend it to:
1. Compute library percentiles from all fetched components
2. Generate up to 2 intensity tags per track
3. Append them to `row.tags`

Replace the existing `enrich_with_intensity` body. After building `intensity_map`
and populating `row.intensity`, add:

```rust
// Compute library-wide percentiles for intensity tag groups
let all_ics: Vec<&mesh_core::db::IntensityComponents> = intensity_map.values().collect();
let library_percentiles = mesh_core::suggestions::scoring::compute_intensity_percentiles(&all_ics);

// Generate intensity tags for each row
for row in rows.iter_mut() {
    if let Some(path) = &row.track_path {
        if let Some(&id) = path_to_id.get(path) {
            if let Some(ic) = intensity_map.get(&id) {
                let tags = mesh_core::suggestions::scoring::generate_intensity_tags_absolute(
                    ic, &library_percentiles,
                );
                for (label, color) in tags {
                    let tag_color = color.and_then(|c| mesh_widgets::track_table::parse_hex_color(&c));
                    row.tags.push(mesh_widgets::TrackTag {
                        label,
                        color: tag_color,
                    });
                }
            }
        }
    }
}
```

Note: the `intensity_map` variable type needs to change from `HashMap<i64, f32>` to
`HashMap<i64, IntensityComponents>` so we can access raw components. Derive composite
inline when setting `row.intensity`.

**Step 2: Commit**

```bash
git add crates/mesh-cue/src/domain/mod.rs
git commit -m "feat: intensity component tags in mesh-cue browser (library percentiles)"
```

---

### Task 5: Add intensity tags to mesh-player browser

**Files:**
- Modify: `crates/mesh-player/src/ui/collection_browser.rs`

Same pattern as Task 4. The mesh-player `enrich_rows` function (~line 1313) already
calls `batch_get_intensity_components`. Extend with library percentiles + tag generation.

**Step 1: Commit**

```bash
git add crates/mesh-player/src/ui/collection_browser.rs
git commit -m "feat: intensity component tags in mesh-player browser"
```

---

### Task 6: Verify and clean up

**Step 1: Run cargo check**

```bash
cargo check -p mesh-core -p mesh-cue -p mesh-player -p mesh-widgets
```

**Step 2: Run tests**

```bash
cargo test -p mesh-core
```

**Step 3: Final commit if any fixes needed**
