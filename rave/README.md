# Pure Data + RAVE + nn~ Development Environment

A Nix devshell for neural audio processing with RAVE models in Pure Data.

## Architecture

**RAVE** (Realtime Audio Variational autoEncoder) trains neural models for audio synthesis and transformation. **nn~** is a Pure Data external that loads these trained models as TorchScript (.ts) files for real-time processing. This setup provides:

- **Training pipeline**: Use RAVE to train models on audio datasets
- **Real-time inference**: Load trained models into Pure Data via nn~
- **Model distribution**: Download pretrained models from IRCAM's repository

`★ Insight ─────────────────────────────────────`
• RAVE generates neural audio models, nn~ loads them into Pure Data for real-time use
• No Python runtime needed in Pure Data - models are exported as standalone TorchScript files  
• nn~ is built from source for full Nix environment compatibility
`─────────────────────────────────────────────────`

## Getting Started

1. **Enter the devshell:**
   ```bash
   nix develop
   ```
   *First run will build nn~ from source (takes a few minutes)*

2. **Start Pure Data:**
   ```bash
   pd
   ```

3. **Use nn~ in Pure Data:**
   ```
   # Create nn~ object
   [nn~]
   
   # Load a model (after downloading)
   [load rave-models/your-model.ts(
   ```

4. **Download pretrained models:**
   ```bash
   wget -P rave-models/ "https://play.forum.ircam.fr/rave-vst-api/get_model/darbouka_onnx"
   ```

5. **Install RAVE for training (optional):**
   ```bash
   nix-shell -p python3Packages.pipx --run "pipx install acids-rave"
   ```

The devshell automatically sets up:
- Pure Data with audio drivers  
- nn~ external (source-built for compatibility)
- PyTorch and libtorch for neural processing
- Model storage directory at `rave-models/`
- All required dependencies and library paths

<function_calls>
<invoke name="TodoWrite">
<parameter name="todos">[{"id": "8", "content": "Create example usage documentation", "status": "completed"}]