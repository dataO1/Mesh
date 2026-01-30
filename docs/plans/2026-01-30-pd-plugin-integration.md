# PD Plugin Integration - Implementation Plan

**Date:** 2026-01-30
**Status:** Draft
**Author:** Collaborative design session

---

## Overview

Integrate Pure Data effects into mesh-player via libpd-rs, enabling users to load custom PD patches (including RAVE neural effects) from their mesh-collection. Effects are completely standalone—mesh only needs to know inputs, outputs, and latency.

---

## Design Decisions (Confirmed)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **Threading model** | 4 separate libpd instances | One per deck for isolation and parallel processing |
| **Dependency organization** | Shared folders | `effects/externals/` and `effects/models/` |
| **Missing dependencies** | Skip with warning | Effect doesn't appear; log shows what's missing |
| **Latency reporting** | Static metadata | Declared in JSON, no runtime querying |
| **Discovery timing** | Startup only | No hot-reload during runtime |
| **Parameter count** | First 8 only | Maps to hardware knobs |
| **nn~ distribution** | GitHub release | Users download; nix script to build |

---

## Directory Structure

```
~/Music/mesh-collection/
├── tracks/                          # Audio files (existing)
├── playlists/                       # Playlists (existing)
├── effects/                         # NEW: PD effects folder
│   ├── externals/                   # Shared PD externals
│   │   └── nn~.pd_linux             # User downloads from GitHub release
│   ├── models/                      # Shared neural models
│   │   ├── percussion_b4096.ts      # RAVE TorchScript models
│   │   └── vintage_b4096.ts
│   └── rave-percussion/             # Effect: RAVE Percussion
│       ├── rave-percussion.pd       # PD patch
│       └── metadata.json            # Effect metadata
└── mesh.db                          # Database (existing)
```

---

## Metadata Schema (`metadata.json`)

```json
{
  "name": "RAVE Percussion",
  "category": "Neural",
  "author": "mesh",
  "version": "1.0.0",
  "description": "Neural timbral transformation using RAVE percussion model",
  "latency_samples": 4096,
  "sample_rate": 48000,
  "requires_externals": ["nn~"],
  "params": [
    { "name": "L1", "default": 0.5 },
    { "name": "L2", "default": 0.5 },
    { "name": "L3", "default": 0.5 },
    { "name": "L4", "default": 0.5 },
    { "name": "L5", "default": 0.5 },
    { "name": "L6", "default": 0.5 },
    { "name": "L7", "default": 0.5 },
    { "name": "L8", "default": 0.5 }
  ]
}
```

**Notes:**
- `latency_samples`: Fixed latency at the specified `sample_rate` (scales automatically if different)
- `requires_externals`: List of external names that must exist in `effects/externals/`
- `params`: Up to 8 parameters, normalized 0.0-1.0. Only `name` and `default` required.

---

## PD Patch Contract

Effects must follow this template for mesh compatibility:

```
┌─────────────────────────────────────────────────────────────┐
│                     mesh-effect.pd                           │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  [inlet~ ]           [inlet~ ]      ← Left, Right audio in  │
│      │                   │                                   │
│      │   [r $0-param0]   │          ← Instance-scoped        │
│      │   [r $0-param1]   │             parameters            │
│      │   ...             │             (0.0-1.0)             │
│      │   [r $0-param7]   │                                   │
│      │                   │                                   │
│      │   [r $0-bypass]   │          ← 0=process, 1=bypass    │
│      │                   │                                   │
│      ▼                   ▼                                   │
│  ┌───────────────────────────────────┐                       │
│  │      YOUR EFFECT PROCESSING       │                       │
│  │                                   │                       │
│  │  (e.g., nn~ encode/decode,        │                       │
│  │   delay, filter, granular...)     │                       │
│  └───────────────────────────────────┘                       │
│      │                   │                                   │
│      ▼                   ▼                                   │
│  [outlet~ ]         [outlet~ ]      ← Left, Right audio out  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Key requirements:**
1. **Two signal inlets**: Left channel first, right channel second
2. **Two signal outlets**: Left channel first, right channel second
3. **Parameter receives**: `[r $0-param0]` through `[r $0-param7]` (optional)
4. **Bypass receive**: `[r $0-bypass]` (optional, mesh handles bypass if missing)
5. **$0 prefix**: All receives must use `$0-` for instance isolation

---

## Implementation Phases

### Phase 1: Enable libpd-rs and Basic Infrastructure

**Goal:** Get libpd compiling and create the basic PdEffect wrapper.

**Tasks:**

1. **Uncomment libpd-rs dependency**
   - File: `Cargo.toml` (workspace)
   - File: `crates/mesh-core/Cargo.toml`
   - Verify build works with existing nix setup

2. **Create PD module structure**
   ```
   crates/mesh-core/src/pd/
   ├── mod.rs           # Module exports
   ├── instance.rs      # PdInstance wrapper (single libpd instance)
   ├── effect.rs        # PdEffect implementing Effect trait
   ├── metadata.rs      # Metadata JSON parsing
   └── discovery.rs     # Effect folder scanning
   ```

3. **Implement PdInstance wrapper**
   ```rust
   // crates/mesh-core/src/pd/instance.rs

   /// Wrapper around a single libpd instance
   /// Each deck gets its own instance for thread isolation
   pub struct PdInstance {
       // libpd instance handle (if libpd-rs supports multiple instances)
       // or global lock management if single-instance
   }

   impl PdInstance {
       pub fn new() -> Result<Self, PdError>;
       pub fn open_patch(&mut self, path: &Path) -> Result<PatchHandle, PdError>;
       pub fn close_patch(&mut self, handle: PatchHandle);
       pub fn process_float(&mut self, in_buffer: &[f32], out_buffer: &mut [f32]);
       pub fn send_float(&mut self, receiver: &str, value: f32);
       pub fn send_bang(&mut self, receiver: &str);
       pub fn set_search_path(&mut self, path: &Path);
   }
   ```

4. **Implement PdEffect**
   ```rust
   // crates/mesh-core/src/pd/effect.rs

   pub struct PdEffect {
       patch_handle: PatchHandle,
       info: EffectInfo,
       params: Vec<ParamValue>,
       latency: u32,
       bypassed: bool,
       instance_id: u32,  // $0 value for this patch

       // Temporary buffers for libpd processing
       in_buffer: Vec<f32>,
       out_buffer: Vec<f32>,
   }

   impl Effect for PdEffect {
       fn process(&mut self, buffer: &mut StereoBuffer) { ... }
       fn latency_samples(&self) -> u32 { self.latency }
       fn set_param(&mut self, index: usize, value: f32) { ... }
       // ... other trait methods
   }
   ```

**Deliverables:**
- [ ] libpd-rs compiles in mesh workspace
- [ ] PdInstance can load/unload patches
- [ ] PdEffect processes audio through a simple test patch
- [ ] Unit tests for PdInstance and PdEffect

---

### Phase 2: Metadata and Effect Discovery

**Goal:** Scan effects folder at startup, parse metadata, validate dependencies.

**Tasks:**

1. **Implement metadata parsing**
   ```rust
   // crates/mesh-core/src/pd/metadata.rs

   #[derive(Debug, Clone, Deserialize)]
   pub struct EffectMetadata {
       pub name: String,
       pub category: String,
       pub author: Option<String>,
       pub version: Option<String>,
       pub description: Option<String>,
       pub latency_samples: u32,
       pub sample_rate: Option<u32>,  // Default: 48000
       pub requires_externals: Vec<String>,
       pub params: Vec<ParamMetadata>,
   }

   #[derive(Debug, Clone, Deserialize)]
   pub struct ParamMetadata {
       pub name: String,
       #[serde(default = "default_param_value")]
       pub default: f32,
   }
   ```

2. **Implement effect discovery**
   ```rust
   // crates/mesh-core/src/pd/discovery.rs

   pub struct EffectDiscovery {
       effects_path: PathBuf,
       externals_path: PathBuf,
       models_path: PathBuf,
   }

   impl EffectDiscovery {
       pub fn new(collection_path: &Path) -> Self;

       /// Scan effects folder, validate, return available effects
       pub fn discover(&self) -> Vec<DiscoveredEffect>;

       /// Check if required externals exist
       fn validate_externals(&self, required: &[String]) -> Vec<String>; // missing
   }

   pub struct DiscoveredEffect {
       pub id: String,              // Folder name
       pub patch_path: PathBuf,
       pub metadata: EffectMetadata,
       pub missing_deps: Vec<String>,
       pub available: bool,         // false if missing deps
   }
   ```

3. **Integration with PlayerConfig**
   - Add `effects_path()` method to config
   - Call discovery at startup in `mesh-player/src/main.rs`
   - Store discovered effects in app state

**Deliverables:**
- [ ] Metadata JSON parsing with serde
- [ ] Effect folder scanning
- [ ] Dependency validation (externals check)
- [ ] Logging for skipped effects with reasons
- [ ] Integration tests with sample effect folder

---

### Phase 3: Per-Deck PD Integration

**Goal:** Give each deck its own PdInstance, allow loading PD effects into stem chains.

**Tasks:**

1. **Create PdManager for per-deck instances**
   ```rust
   // crates/mesh-core/src/pd/manager.rs

   pub struct PdManager {
       instances: [Option<PdInstance>; 4],  // One per deck
       discovered_effects: Vec<DiscoveredEffect>,
       externals_path: PathBuf,
   }

   impl PdManager {
       pub fn new(collection_path: &Path) -> Result<Self, PdError>;

       /// Initialize PD instance for a deck (lazy)
       pub fn init_deck(&mut self, deck_index: usize) -> Result<(), PdError>;

       /// Create effect instance on a specific deck
       pub fn create_effect(
           &mut self,
           deck_index: usize,
           effect_id: &str,
       ) -> Result<Box<dyn Effect>, PdError>;

       /// Get list of available effects
       pub fn available_effects(&self) -> &[DiscoveredEffect];
   }
   ```

2. **Integrate with Deck**
   - Add `pd_manager: Arc<Mutex<PdManager>>` to Engine
   - Expose effect creation via Engine API
   - Ensure PdEffect is `Send` for effect chain threading

3. **Handle audio processing thread safety**
   - libpd processes on the thread that calls `process_float()`
   - Each deck already processes in parallel (rayon)
   - PdInstance per deck ensures no contention

**Deliverables:**
- [ ] PdManager with per-deck instances
- [ ] Effect creation API
- [ ] Thread-safe effect processing
- [ ] Integration with existing EffectChain

---

### Phase 4: Template Effect and Testing

**Goal:** Create a working RAVE effect template and test end-to-end.

**Tasks:**

1. **Create effect template**
   ```
   effects/_template/
   ├── template.pd           # Minimal passthrough effect
   └── metadata.json
   ```

2. **Create RAVE effect**
   ```
   effects/rave-percussion/
   ├── rave-percussion.pd    # Uses nn~ encode/decode
   └── metadata.json
   ```

   The patch assumes:
   - `../externals/nn~.pd_linux` is in PD search path
   - `../models/percussion_b4096.ts` exists

3. **Create sample-delay effect** (for latency testing)
   ```
   effects/sample-delay/
   ├── sample-delay.pd       # Variable delay for testing compensation
   └── metadata.json
   ```

4. **Integration test**
   - Load track
   - Add PD effect to stem chain
   - Verify audio passes through
   - Verify latency compensation works

**Deliverables:**
- [ ] Working template effect
- [ ] Working RAVE effect (requires nn~ external)
- [ ] Sample-delay effect for testing
- [ ] End-to-end integration test

---

### Phase 5: nn~ Build Script and Documentation

**Goal:** Provide nix script to build nn~ external, document effect creation.

**Tasks:**

1. **Create nn~ build app**
   ```nix
   # nix/apps/build-nn-external.nix
   # Builds nn~.pd_linux from nn-tilde input
   # Output goes to current directory
   ```

   Usage: `nix run .#build-nn-external`

2. **Document effect creation**
   - README in `effects/_template/`
   - Explain patch contract
   - Explain metadata schema
   - Explain externals/models organization

3. **GitHub release setup** (manual for now)
   - Build nn~ with nix script
   - Upload to GitHub releases
   - Document download location in README

**Deliverables:**
- [ ] `nix run .#build-nn-external` working
- [ ] Effect creation documentation
- [ ] README updates

---

## Implementation Details

### libpd-rs Threading Model

libpd traditionally uses global state. Options:

**Option A: libpd-rs multiple instance support**
- Check if libpd-rs wraps `libpd_new_instance()` (libpd 0.12+)
- Each deck gets truly separate instance
- Best isolation

**Option B: Single instance with locking**
- If libpd-rs is single-instance only
- Serialize PD processing across decks
- Less parallel but still functional

**Recommendation:** Start with Option B (safer), upgrade to Option A if available.

### Parameter Mapping

```rust
impl PdEffect {
    fn set_param(&mut self, index: usize, value: f32) {
        if index < 8 {
            let receiver = format!("{}-param{}", self.instance_id, index);
            // libpd send_float expects $0 to be replaced by actual instance ID
            self.instance.send_float(&receiver, value.clamp(0.0, 1.0));
        }
    }
}
```

### Latency Scaling

```rust
impl PdEffect {
    fn latency_samples(&self) -> u32 {
        // Scale latency if patch was designed for different sample rate
        let patch_sr = self.metadata.sample_rate.unwrap_or(48000);
        let current_sr = SAMPLE_RATE; // 48000

        (self.metadata.latency_samples as f64 * current_sr as f64 / patch_sr as f64) as u32
    }
}
```

### Search Path Configuration

```rust
impl PdInstance {
    fn configure_search_paths(&mut self, collection_path: &Path) {
        let effects_path = collection_path.join("effects");

        // Add externals path (for nn~, cyclone, etc.)
        self.add_search_path(&effects_path.join("externals"));

        // Add models path (for nn~ model loading)
        self.add_search_path(&effects_path.join("models"));
    }
}
```

---

## File Changes Summary

### New Files

| File | Purpose |
|------|---------|
| `crates/mesh-core/src/pd/mod.rs` | Module exports |
| `crates/mesh-core/src/pd/instance.rs` | PdInstance wrapper |
| `crates/mesh-core/src/pd/effect.rs` | PdEffect implementation |
| `crates/mesh-core/src/pd/metadata.rs` | Metadata parsing |
| `crates/mesh-core/src/pd/discovery.rs` | Effect folder scanning |
| `crates/mesh-core/src/pd/manager.rs` | Per-deck instance management |
| `nix/apps/build-nn-external.nix` | nn~ build script |

### Modified Files

| File | Change |
|------|--------|
| `Cargo.toml` | Uncomment libpd-rs |
| `crates/mesh-core/Cargo.toml` | Uncomment libpd-rs, add serde |
| `crates/mesh-core/src/lib.rs` | Uncomment `pub mod pd;` |
| `crates/mesh-player/src/main.rs` | Initialize PdManager at startup |
| `crates/mesh-core/src/engine/engine.rs` | Add pd_manager field |
| `flake.nix` | Add build-nn-external app |

---

## Testing Strategy

### Unit Tests
- `pd/metadata.rs`: Parse valid/invalid JSON
- `pd/discovery.rs`: Scan mock folder structure
- `pd/effect.rs`: Parameter setting, bypass

### Integration Tests
- Load actual PD patch, process audio buffer
- Verify latency reporting matches metadata
- Test missing dependency handling

### Manual Testing
- Load RAVE effect in mesh-player
- Verify audio transformation
- Verify latency compensation alignment

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| libpd-rs build fails | Already have gnumake/libffi setup in devshell; fall back to manual build |
| libpd single-instance limitation | Use mutex for serialized access; still functional |
| RAVE models too large | Recommend users download models separately; not bundled |
| PD processing too slow | Profile and optimize buffer sizes; PD is efficient |
| Thread safety issues | Each deck has own instance; careful mutex usage |

---

## Success Criteria

- [ ] User can place PD patch in `~/Music/mesh-collection/effects/`
- [ ] Effect appears in mesh-player effect selector (if dependencies met)
- [ ] Audio processes through PD patch correctly
- [ ] Latency compensation works across stems
- [ ] RAVE effect works with downloaded nn~ and models
- [ ] Missing dependencies logged clearly

---

## Future Enhancements (Out of Scope)

- Hot-reload effects during runtime
- Effect parameter presets
- Visual PD patch editor integration
- Multi-channel effects (beyond stereo)
- MIDI control from PD patches
