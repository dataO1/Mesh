# Mesh Effects System

Mesh supports two types of audio effects:
- **Pure Data (PD) effects** - Custom effects built with Pure Data
- **CLAP plugins** - Industry-standard audio plugins

## Directory Structure

```
mesh-collection/effects/
├── pd/                      # Pure Data effects
│   ├── <effect-name>/       # Effect folders
│   │   ├── metadata.json    # Effect configuration
│   │   └── <effect-name>.pd # Pure Data patch
│   ├── externals/           # PD external objects
│   │   ├── nn~.pd_linux     # Neural network external
│   │   └── lib/             # Runtime libs (libtorch, etc.)
│   └── models/              # ML models for nn~ external
│       └── *.ts             # TorchScript models
└── clap/                    # CLAP audio plugins
    ├── *.clap               # Plugin files
    └── lib/                 # Bundled runtime dependencies
        └── *.so             # Shared libraries (for portability)
```

## PD Effects

Each PD effect lives in its own folder with:
- `metadata.json` - Effect name, category, parameters, latency
- `main.pd` - The Pure Data patch

### metadata.json Example

```json
{
  "name": "Test Gain",
  "category": "Utility",
  "description": "Simple gain control for testing",
  "latency_samples": 0,
  "requires_externals": [],
  "parameters": [
    {
      "name": "Gain",
      "min": 0.0,
      "max": 2.0,
      "default": 1.0,
      "unit": "linear"
    }
  ]
}
```

### Creating a PD Effect

1. Create a folder: `effects/pd/<your-effect>/`
2. Create `metadata.json` with effect info
3. Create `<your-effect>.pd` with your patch (filename must match folder name)
4. The patch receives audio on `[adc~ 1]` and `[adc~ 2]`
5. Output audio via `[dac~ 1]` and `[dac~ 2]`
6. Receive parameters via `[r param1]`, `[r param2]`, etc.
7. Place any required externals in `effects/pd/externals/`
8. Place any required models in `effects/pd/models/`

## CLAP Plugins

CLAP (CLever Audio Plugin) is an open standard for audio plugins.

### Installing CLAP Plugins

Place `.clap` files in `effects/clap/`. Mesh scans this directory on startup.

### Bundled Dependencies (Portable Setup)

For portability (especially on NixOS or non-FHS systems), CLAP plugins can bundle
their runtime dependencies in a `lib/` subdirectory:

```
effects/clap/
├── my-plugin.clap
└── lib/
    ├── libsndfile.so.1
    ├── libcairo.so.2
    └── ... (other dependencies)
```

To make plugins standalone:

1. **Find dependencies**: `ldd plugin.clap | grep "not found"`
2. **Copy missing libs** to `lib/`
3. **Patch RPATH** so libraries find each other:
   ```bash
   # For each library in lib/
   patchelf --set-rpath '$ORIGIN' lib/*.so*

   # For the plugin itself
   patchelf --set-rpath '$ORIGIN/lib' plugin.clap
   ```

Mesh automatically adds `effects/clap/lib/` to `LD_LIBRARY_PATH` when loading plugins.

### LSP Plugins

[LSP Plugins](https://lsp-plug.in/) is a collection of 200+ high-quality open-source
audio plugins available in CLAP format.

To install:
```bash
# Download latest release
wget https://github.com/lsp-plugins/lsp-plugins/releases/download/1.2.26/lsp-plugins-clap-1.2.26-Linux-x86_64.tar.gz

# Extract to effects/clap/
tar xzf lsp-plugins-clap-*.tar.gz -C effects/clap/ --strip-components=1

# On NixOS: bundle dependencies (see scripts/setup-lsp-plugins.sh)
```

## Multiband Processing

Effects can be assigned to frequency bands in the multiband processor:
- **Pre-FX**: Processes full signal before band splitting
- **Bands 1-8**: Individual frequency bands
- **Post-FX**: Processes full signal after band recombination

## Known Limitations

### PD Effects
- **libpd architecture**: All PD patches run in a single global DSP graph
- Multiple PD effects process in **parallel**, not series
- For serial processing, use a single patch with multiple stages

### CLAP Plugins
- GUI support is not yet implemented
- Parameters are controlled via the 8 macro knobs in the UI
