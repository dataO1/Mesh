# Pure Data Effects for Mesh

This folder contains example PD effects that demonstrate the mesh PD effect format.

## Creating a New Effect

1. Create a folder in your `mesh-collection/effects/` directory
2. The folder name becomes the effect ID (e.g., `my-effect`)
3. Create two required files:
   - `metadata.json` - Effect configuration and parameters
   - `{folder-name}.pd` - The Pure Data patch (must match folder name)

## Folder Structure

```
mesh-collection/
  effects/
    externals/           # Shared PD externals (.pd_linux, .pd_darwin, .dll)
    models/              # Shared model files (.ts, .onnx, etc.)
    my-effect/
      metadata.json      # Effect metadata
      my-effect.pd       # Main patch file (must match folder name)
      helper.pd          # Optional: additional abstractions
```

## metadata.json Format

```json
{
  "name": "Display Name",
  "category": "Category",
  "description": "Optional description",
  "latency_samples": 0,
  "sample_rate": 48000,
  "requires_externals": ["nn~"],
  "params": [
    {
      "name": "Param Name",
      "min": 0.0,
      "max": 1.0,
      "default": 0.5,
      "unit": "%"
    }
  ]
}
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Display name shown in UI |
| `category` | Yes | Category for grouping (e.g., "Delay", "Filter", "Neural") |
| `description` | No | Brief description of the effect |
| `latency_samples` | Yes | Processing latency at the specified sample rate |
| `sample_rate` | No | Sample rate for latency calculation (default: 48000) |
| `requires_externals` | No | List of required PD externals |
| `params` | No | Up to 8 parameters for hardware knob mapping |

### Parameter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Parameter name |
| `min` | No | Minimum value (default: 0.0) |
| `max` | No | Maximum value (default: 1.0) |
| `default` | Yes | Default value (should be normalized 0-1) |
| `unit` | No | Display unit (e.g., "%", "ms", "Hz") |

## Patch Interface

Your patch MUST have:
- `inlet~` / `inlet~` - Stereo audio input (left, right)
- `outlet~` / `outlet~` - Stereo audio output (left, right)

Your patch SHOULD receive parameters via:
- `r $0-param0` through `r $0-param7` - Normalized parameter values (0-1)
- `r $0-bypass` - Bypass state (0 = active, 1 = bypassed)

The `$0` prefix is critical - it creates instance-scoped receives so multiple
instances of your effect can run independently.

## Example: Simple Gain

```
#N canvas 0 0 400 300 12;
#X obj 100 50 inlet~;
#X obj 200 50 inlet~;
#X obj 100 200 outlet~;
#X obj 200 200 outlet~;
#X obj 100 100 *~;
#X obj 200 100 *~;
#X obj 300 100 r \$0-param0;
#X connect 0 0 4 0;
#X connect 1 0 5 0;
#X connect 4 0 2 0;
#X connect 5 0 3 0;
#X connect 6 0 4 1;
#X connect 6 0 5 1;
```

## Required Externals

If your effect uses external objects (like `nn~` for RAVE), list them in
`requires_externals`. Users must place the external files in:
- `effects/externals/` folder

External file naming:
- Linux: `external~.pd_linux`
- macOS: `external~.pd_darwin`
- Windows: `external~.dll`

## RAVE Effects

For neural audio effects using RAVE:

1. Place `nn~` external in `effects/externals/`
2. Place your `.ts` model file either:
   - In `effects/models/` (shared)
   - In your effect folder (bundled)
3. Reference the model in your patch: `nn~ my_model.ts encode`

See `rave-percussion/` for a complete example.

## Discovery

Effects are discovered at mesh startup. To add new effects:
1. Place the effect folder in `mesh-collection/effects/`
2. Restart mesh

Effects with missing dependencies (externals) will be marked unavailable
but still shown in the effect list.
