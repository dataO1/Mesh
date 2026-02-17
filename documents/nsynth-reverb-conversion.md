# NSynth Reverb Model: TensorFlow to ONNX Conversion

## Status: Deferred

The `nsynth_reverb-discogs-effnet-1` model exists only as a TensorFlow `.pb`
(SavedModel) on the Essentia model hub. All other EffNet classification heads
ship as ONNX, but the NSynth family was never converted upstream.

The model definition and download URL are already wired into the codebase
(`MlModelType::NsynthReverb`), but the ONNX file does not exist yet. The
inference engine gracefully skips it when the file is missing.

## Model Details

| Property | Value |
|----------|-------|
| Source | `https://essentia.upf.edu/models/classification-heads/nsynth_reverb/` |
| TF file | `nsynth_reverb-discogs-effnet-1.pb` |
| Input | `"embeddings"` `[1, 1280]` (EffNet embedding) |
| Output | `"activations"` `[1, 2]` — softmax: `[wet, dry]` |
| Positive class ("wet") | Index **0** |
| Accuracy | ~82% (5-fold CV) |

## Conversion Steps

When ready to enable this model:

### 1. Install tf2onnx

```bash
pip install tf2onnx tensorflow
```

### 2. Download the TF SavedModel

```bash
mkdir -p /tmp/nsynth_reverb
wget -O /tmp/nsynth_reverb/nsynth_reverb-discogs-effnet-1.pb \
  https://essentia.upf.edu/models/classification-heads/nsynth_reverb/nsynth_reverb-discogs-effnet-1.pb
```

### 3. Convert to ONNX

```bash
python -m tf2onnx.convert \
  --graphdef /tmp/nsynth_reverb/nsynth_reverb-discogs-effnet-1.pb \
  --output nsynth_reverb-discogs-effnet-1.onnx \
  --inputs embeddings:0[1,1280] \
  --outputs activations:0
```

**Note:** The TF SavedModel uses `serving_default_*` prefixed names internally,
but tf2onnx strips these prefixes. The resulting ONNX model will have input
name `embeddings` and output name `activations`, matching all other Essentia
classification heads.

### 4. Upload to GitHub Releases

```bash
gh release upload models nsynth_reverb-discogs-effnet-1.onnx \
  --repo dataO1/Mesh
```

The download URL in `MlModelType::NsynthReverb` already points to:
`https://github.com/dataO1/Mesh/releases/download/models/nsynth_reverb-discogs-effnet-1.onnx`

### 5. Verify

After uploading, `MlModelManager::ensure_all_models()` will automatically
download and cache the model. The `MlAnalyzer` will then populate the `reverb`
field in `MlAnalysisData` (score only, no tag generated).

## Alternative: Nix Flake App

A `convert-reverb-model` Nix app could automate steps 1-3:

```nix
convert-reverb-model = {
  type = "app";
  program = "${pkgs.writeShellScript "convert-reverb-model" ''
    ${pkgs.python3.withPackages (p: [p.tf2onnx p.tensorflow])}/bin/python \
      -m tf2onnx.convert \
      --graphdef <(${pkgs.curl}/bin/curl -sL "$URL") \
      --output nsynth_reverb-discogs-effnet-1.onnx \
      --inputs embeddings:0[1,1280] \
      --outputs activations:0
  ''}";
};
```

This is deferred until the model is actually needed for production use.
