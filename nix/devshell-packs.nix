# Sound-pack shell: tools to turn raw WAVs and mudpies into live-mode packs
# (docs/plans/2026-10-16-live-sound-packs.md).
#
#   nix develop .#packs
#
#   polyphone     sample instrument editor: WAVs -> key zones, loops,
#                 envelopes -> export SFZ
#   qtractor      MIDI sequencer: piano roll, velocity / pitch bend / CC lanes,
#                 hosts sfizz (LV2 / VST3); export the phrases as .mid
#   sfizz_jack    standalone SFZ player on JACK (play it from a MIDI keyboard)
#   sfizz_render  render SFZ + MIDI to WAV offline (quick checks)
{ pkgs }:

pkgs.mkShell {
  name = "mesh-packs-shell";

  packages = with pkgs; [
    polyphone
    qtractor
    sfizz-ui
  ];

  shellHook = ''
    # Same JACK setup as the main devshell: PipeWire's libjack from the pinned
    # nixpkgs, so the apps reach the laptop's PipeWire without pw-jack.
    export LD_LIBRARY_PATH="${pkgs.pipewire.jack}/lib:''${LD_LIBRARY_PATH:-}"
    # Qtractor finds the sfizz plugins here.
    export LV2_PATH="${pkgs.sfizz-ui}/lib/lv2''${LV2_PATH:+:$LV2_PATH}"
    export VST3_PATH="${pkgs.sfizz-ui}/lib/vst3''${VST3_PATH:+:$VST3_PATH}"
    echo "mesh packs shell: polyphone, qtractor, sfizz_jack, sfizz_render (sfizz LV2/VST3 on LV2_PATH/VST3_PATH)"
  '';
}
