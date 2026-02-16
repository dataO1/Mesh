# Base NixOS configuration for the mesh embedded player
#
# This is the top-level NixOS module that imports all embedded subsystems.
# Board-specific support (kernel, bootloader) comes from nixos-rk3588
# which is imported in flake.nix.
{ pkgs, lib, ... }:

{
  imports = [
    ./hardware.nix
    ./audio.nix
    ./kiosk.nix
  ];

  system.stateVersion = "24.11";
  networking.hostName = "mesh-embedded";
  time.timeZone = "UTC";

  # WiFi (built-in on OPi 5 Pro)
  networking.networkmanager.enable = true;

  # Nix configuration
  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "mesh" ];

    # Binary caches: custom mesh cache (GitHub Pages) + official NixOS cache
    # The mesh cache hosts pre-built aarch64 packages (mesh-player, essentia, etc.)
    # Standard NixOS packages come from cache.nixos.org
    substituters = [
      # TODO: replace with actual GitHub Pages URL after first CI run
      # "https://username.github.io/mesh-cache/"
      "https://cache.nixos.org/"
    ];
    trusted-public-keys = [
      # TODO: replace with actual public key after generating keypair
      # "mesh-embedded:<public-key>"
      "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
    ];
  };

  # System packages for debugging and maintenance
  environment.systemPackages = with pkgs; [
    vim
    htop
    alsa-utils
    usbutils
    pciutils
    dtc
    wlr-randr
    evtest
  ];

  # Persistent state directory for mesh-player
  systemd.tmpfiles.rules = [
    "d /var/lib/mesh 0755 mesh mesh -"
  ];
}
