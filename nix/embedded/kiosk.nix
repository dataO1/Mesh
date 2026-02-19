# Kiosk mode: cage Wayland compositor running mesh-player fullscreen
#
# cage is a single-application Wayland compositor (kiosk mode).
# mesh-player launches fullscreen on boot and restarts on crash.
# SSH is always available as a backdoor for debugging.
{ pkgs, lib, config, ... }:

let
  meshPlayer = config.services.mesh-embedded.package;
in
{
  options.services.mesh-embedded = {
    package = lib.mkOption {
      type = lib.types.package;
      description = "The mesh-player package to run in kiosk mode";
    };
  };

  config = {
    # Mesh user account (wheel for sudo access)
    users.users.mesh = {
      isNormalUser = true;
      extraGroups = [ "audio" "video" "input" "plugdev" "wheel" ];
      initialPassword = "mesh";
    };

    # Root account (for emergency console access)
    users.users.root.initialPassword = "mesh";

    # cage Wayland kiosk compositor
    services.cage = {
      enable = true;
      user = "mesh";
      program = "${meshPlayer}/bin/mesh-player";
      extraArguments = [ "-d" ];
      environment = {
        # Use GLES via Panthor (Mali-G610)
        WGPU_BACKEND = "gl";
        MESA_GL_VERSION_OVERRIDE = "3.1";
        WLR_NO_HARDWARE_CURSORS = "1";
        # winit loads wayland/xkbcommon via dlopen — not in RPATH on NixOS
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.wayland pkgs.libxkbcommon pkgs.libGL pkgs.vulkan-loader ];
      };
    };

    # Crash restart + pin to A76 big cores (cores 4-7)
    systemd.services."cage-tty1" = {
      serviceConfig = {
        Restart = "always";
        RestartSec = 2;
        CPUAffinity = "4-7";
      };
    };

    # SSH backdoor (always available, even if cage crashes)
    services.openssh = {
      enable = true;
      settings.PasswordAuthentication = true;
    };
    networking.firewall.allowedTCPPorts = [ 22 ];

    # Allow mesh user to trigger updates via systemd service
    systemd.services.mesh-update = {
      description = "Mesh Player System Update";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = pkgs.writeShellScript "mesh-update" ''
          set -euo pipefail
          VERSION=$(cat /var/lib/mesh/update-target 2>/dev/null || echo "")
          if [ -z "$VERSION" ]; then
            echo "No update target set"
            exit 1
          fi
          echo "Updating to $VERSION..."
          ${config.system.build.nixos-rebuild}/bin/nixos-rebuild switch \
            --flake "github:dataO1/Mesh/$VERSION#mesh-embedded" \
            --no-write-lock-file
          rm -f /var/lib/mesh/update-target
        '';
      };
    };

    # Polkit rule: allow mesh user to start the update service
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            action.lookup("unit") == "mesh-update.service" &&
            subject.user == "mesh") {
          return polkit.Result.YES;
        }
      });
    '';
  };
}
