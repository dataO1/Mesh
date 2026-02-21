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

  # PAM limits for audio group (memlock, rtprio, nice)
  security.pam.loginLimits = [
    { domain = "@audio"; type = "-"; item = "memlock"; value = "unlimited"; }
    { domain = "@audio"; type = "-"; item = "rtprio";  value = "99"; }
    { domain = "@audio"; type = "-"; item = "nice";    value = "-19"; }
  ];

  # PipeWire as the audio server (ALSA + JACK compatibility)
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = false;
    jack.enable = true;

    # Low-latency clock: 256 samples @ 48kHz = 5.33ms per period
    extraConfig.pipewire."92-low-latency" = {
      "context.properties" = {
        "default.clock.rate" = 48000;
        "default.clock.quantum" = 256;
        "default.clock.min-quantum" = 64;
        "default.clock.max-quantum" = 1024;
      };
    };
  };

  # Named ALSA aliases for stable device references (with plug wrapper
  # for automatic format/channel conversion — ES8388 only accepts stereo)
  # Usage: aplay -D mesh_master / aplay -D mesh_cue
  environment.etc."alsa/conf.d/99-mesh.conf".text = ''
    pcm.mesh_master {
      type plug
      slave.pcm {
        type hw
        card "PCM5102A"
        device 0
      }
    }
    pcm.mesh_cue {
      type plug
      slave.pcm {
        type hw
        card "rockchipes8388"
        device 0
      }
    }
  '';

  # WirePlumber: per-node ALSA tuning
  # DP/HDMI sinks are kept alive (device.disabled breaks the ALSA monitor)
  # but deprioritized. Actual routing is forced by jack.rules + PIPEWIRE_NODE.
  services.pipewire.wireplumber.extraConfig."99-mesh-audio" = {
    "monitor.alsa.rules" = [
      # ES8388 onboard codec (3.5mm headphone jack)
      {
        matches = [{ "node.name" = "~alsa_output.*es8388*"; }];
        actions.update-props = {
          "session.suspend-timeout-seconds" = 0;
          "audio.rate" = 48000;
          "api.alsa.period-size" = 256;
          "api.alsa.headroom" = 0;
          "priority.driver" = 10000;
          "priority.session" = 10000;
        };
      }
      # PCM5102A I2S DAC (master output to PA)
      {
        matches = [{ "node.name" = "~alsa_output.*PCM5102A*"; }];
        actions.update-props = {
          "session.suspend-timeout-seconds" = 0;
          "audio.rate" = 48000;
          "api.alsa.period-size" = 256;
          "api.alsa.headroom" = 0;
          "priority.driver" = 3000;
          "priority.session" = 3000;
        };
      }
    ];
  };

  # PipeWire JACK rules: route mesh-player to the ES8388 output.
  # priority.driver only controls clock source, NOT audio routing —
  # JACK client routing requires an explicit target.object.
  services.pipewire.extraConfig.jack."99-mesh-target" = {
    "jack.rules" = [
      {
        matches = [{ "application.process.binary" = "mesh-player"; }];
        actions.update-props = {
          "target.object" = "alsa_output.platform-es8388-sound.stereo-fallback";
        };
      }
    ];
  };

  # Initialize ES8388 mixer on boot: enable headphone path, set volumes,
  # disable 3D processing for faithful audio reproduction
  systemd.services.mesh-audio-init = {
    description = "Initialize audio card volumes";
    after = [ "sound.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${pkgs.writeShellScript "mesh-audio-init" ''
        AMIXER="${pkgs.alsa-utils}/bin/amixer -c rockchipes8388"
        # Enable headphone output path
        $AMIXER set 'Headphone' on
        $AMIXER set 'hp switch' on
        # Set playback volumes
        $AMIXER set 'PCM' 85%
        $AMIXER set 'Output 1' 100%
        $AMIXER set 'Output 2' 100%
        # Disable 3D processing for faithful reproduction
        $AMIXER set '3D Mode' 'No 3D  '
        # Ensure mixer paths are enabled
        $AMIXER set 'Left Mixer Left' on
        $AMIXER set 'Right Mixer Right' on
      ''}";
    };
  };
}
