# Audio configuration for dual-output setup
#
# Master output: PCM5102A I2S DAC on GPIO → PA system
# Cue output:    ES8388 onboard codec → headphones (3.5mm TRRS jack)
#
# Named ALSA aliases provide stable device names regardless of card numbering.
# mesh-player selects devices by card name substring, never by card number.
{ pkgs, ... }:

{
  # Real-time audio priority
  security.rtkit.enable = true;

  # PipeWire as the audio server (ALSA + JACK compatibility)
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = false;
    jack.enable = true;
  };

  # Named ALSA aliases for stable device references
  # Usage: aplay -D mesh_master / aplay -D mesh_cue
  environment.etc."alsa/conf.d/99-mesh.conf".text = ''
    pcm.mesh_master {
      type hw
      card "PCM5102A"
      device 0
    }
    pcm.mesh_cue {
      type hw
      card "rockchipes8388"
      device 0
    }
  '';

  # WirePlumber: prevent suspend and set low-latency buffer sizes
  services.pipewire.wireplumber.extraConfig."99-mesh-audio" = {
    "monitor.alsa.rules" = [
      {
        matches = [
          { "node.name" = "~alsa_output.*es8388*"; }
          { "node.name" = "~alsa_output.*PCM5102A*"; }
        ];
        actions = {
          update-props = {
            "session.suspend-timeout-seconds" = 0;
            "api.alsa.period-size" = 256;
            "api.alsa.headroom" = 256;
          };
        };
      }
    ];
  };

  # Initialize headphone volume on boot
  systemd.services.mesh-audio-init = {
    description = "Initialize audio card volumes";
    after = [ "sound.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${pkgs.alsa-utils}/bin/amixer -c rockchipes8388 set Headphone 80%";
    };
  };
}
