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
    # Quantum locked to 256 — no adaptive sizing, no client overrides
    extraConfig.pipewire."92-low-latency" = {
      "context.properties" = {
        "default.clock.rate" = 48000;
        "default.clock.quantum" = 256;
        "default.clock.min-quantum" = 256;    # Lock quantum (was 64)
        "default.clock.max-quantum" = 256;    # Lock quantum (was 1024)
        "default.clock.force-quantum" = 256;  # Override client requests
        # RT module settings (read by the default-loaded libpipewire-module-rt)
        # NOTE: Do NOT use context.modules here — SPA JSON arrays in fragments
        # REPLACE the base config's module list, breaking ALSA/JACK/protocol support
        "nice.level" = -15;
        "rt.prio" = 88;         # High RT priority for PipeWire data thread
        "rt.time.soft" = -1;     # No soft RT time limit
        "rt.time.hard" = -1;     # No hard RT time limit
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

  # WirePlumber ALSA tuning rules.
  # NixOS's services.pipewire.wireplumber.extraConfig doesn't generate files
  # on this system, so we write the config directly via environment.etc.
  # Routing is handled by pw-link in the kiosk wrapper (see kiosk.nix).
  environment.etc."wireplumber/wireplumber.conf.d/99-mesh-audio.conf".text = ''
    monitor.alsa.rules = [
      {
        matches = [
          { node.name = "~alsa_output.*es8388*" }
        ]
        actions = {
          update-props = {
            node.description = "Headphone (ES8388)"
            node.nick = "Headphone"
            session.suspend-timeout-seconds = 0
            audio.rate = 48000
            api.alsa.period-size = 256
            api.alsa.headroom = 0
            api.alsa.disable-batch = true
            node.always-process = true
            resample.quality = 0
            priority.driver = 10000
            priority.session = 10000
          }
        }
      }
      {
        matches = [
          { node.name = "~alsa_output.*PCM5102A*" }
        ]
        actions = {
          update-props = {
            node.description = "Mains Out (PCM5102A)"
            node.nick = "Mains Out"
            session.suspend-timeout-seconds = 0
            audio.rate = 48000
            api.alsa.period-size = 256
            api.alsa.headroom = 0
            api.alsa.disable-batch = true
            node.always-process = true
            resample.quality = 0
            priority.driver = 3000
            priority.session = 3000
          }
        }
      }
    ]
  '';

  # Pin audio IRQs to A55 audio core (CPU 0), move all others to A76 (4-7)
  # Reduces jitter from non-audio IRQ contention on the audio processing cores
  systemd.services.mesh-irq-affinity = {
    description = "Pin audio IRQs to A55 audio core, move others to A76";
    after = [ "sound.target" "mesh-audio-init.service" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "mesh-irq-affinity" ''
        for irqdir in /proc/irq/*/; do
          irq=$(basename "$irqdir")
          [ "$irq" = "default_smp_affinity" ] && continue
          actions=$(cat "$irqdir/actions" 2>/dev/null || echo "")
          case "$actions" in
            *i2s*|*es8388*|*rockchip-i2s*|*dma*)
              # Audio IRQs -> CPU 0 (A55, dedicated audio core)
              echo 01 > "$irqdir/smp_affinity" 2>/dev/null
              echo "Pinned IRQ $irq ($actions) to CPU 0 (A55)"
              ;;
            *)
              # Everything else -> A76 cores 4-7
              echo f0 > "$irqdir/smp_affinity" 2>/dev/null
              ;;
          esac
        done
        echo f0 > /proc/irq/default_smp_affinity
      '';
    };
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
