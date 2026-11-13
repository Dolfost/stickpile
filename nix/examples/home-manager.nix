# Example home-manager configuration for programs.stickpile, evaluated by the
# `home-manager-module` flake check. It sets every option (no nulls), one of
# them with `lib.mkDefault`, which counts as set and is rendered too.
{ lib, ... }:
{
  programs.stickpile = {
    enable = true;
    desktop.enable = true;
    settings = {
      server_url = "https://stickpile.example.ts.net";
      token_file = "/home/alice/.local/state/stickpile/token";
      cache_dir = "/home/alice/.cache/stickpile";
      cache_max_mb = 1024;
      default_target = "generic";
      prefer_stickers = true;
      timeout_secs = lib.mkDefault 7;
      ui = {
        columns = 8;
        thumb_px = 96;
        hide_on_copy = true;
        start_hidden = false;
        recent_count = 32;
        max_results = 80;
        modal = false;
        start_tab = "favorites";
      };
      copy = {
        square = "white";
        background = "white";
      };
      focus = {
        detect = true;
        rules = [
          {
            contains = [
              "ayugram"
              "telegram"
            ];
            target = "telegram";
          }
          {
            contains = [ "vesktop" ];
            target = "discord";
          }
        ];
      };
      clipboard = {
        default = {
          still = [
            "image/png"
            "text/uri-list"
          ];
          animation = [
            "image/gif"
            "text/uri-list"
          ];
          video = [ "text/uri-list" ];
          sticker = [
            "text/uri-list"
            "x-special/gnome-copied-files"
          ];
        };
        targets.telegram = {
          still = [ "text/uri-list" ];
          animation = [ "text/uri-list" ];
          video = [ "text/uri-list" ];
          sticker = [ "text/uri-list" ];
        };
        # Unset lists take the built-in defaults.
        targets.discord.still = [ "image/png" ];
      };
    };
  };
}
