# home-manager module: programs.stickpile
#
# `settings` mirrors the client's `ClientConfig`
# (crates/stickpile-core/src/config.rs) one-to-one: same names, same nesting,
# same defaults. It is written to $XDG_CONFIG_HOME/stickpile/config.toml, read
# by both the `stickpile` CLI and `stickpile-desktop`. Options left at null
# are omitted from the file. The `config-drift` flake check compares these
# defaults with `ClientConfig::default()`; with `validateConfig` (default) the
# generated file is checked by `stickpile config check` at build time.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkOption
    mkEnableOption
    mkIf
    types
    ;

  cfg = config.programs.stickpile;
  s = cfg.settings;
  settingsFormat = pkgs.formats.toml { };

  opt =
    type: default: description:
    mkOption { inherit type default description; };
  section =
    description: options:
    mkOption {
      type = types.submodule { inherit options; };
      default = { };
      inherit description;
    };
  u32 = types.ints.u32;
  u64 = types.ints.unsigned;

  # stickpile_api::TargetApp
  targetNames = [
    "telegram"
    "discord"
    "signal"
    "whatsapp"
    "slack"
    "generic"
  ];
  target = types.enum targetNames;

  # A path in the home directory, never copied into the store.
  hostPathOpt =
    description:
    mkOption {
      type = types.nullOr types.externalPath;
      default = null;
      apply = v: if v == null then null else toString v;
      inherit description;
    };

  # ClientConfig.clipboard: ClipboardOffers. Each list is the MIME types
  # offered on the clipboard, in offer order. The client accepts `image/png`,
  # `image/gif`, `image/webp`, `image/jpeg`, `text/uri-list`,
  # `x-special/gnome-copied-files` and `text/plain`; its loader (run at build
  # time, see validateConfig) rejects anything else.
  mime = types.str;
  offersOptions = {
    still = opt (types.listOf mime) [
      "image/png"
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers for still images.";
    animation = opt (types.listOf mime) [
      "image/gif"
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers for GIFs and animated WebP.";
    video = opt (types.listOf mime) [
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers for videos.";
    sticker = opt (types.listOf mime) [
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers when copying the sticker rendition (512 px WebP).";
  };
  offers = types.submodule { options = offersOptions; };

  focusRule = types.submodule {
    options = {
      contains = mkOption {
        type = types.listOf types.str;
        description = "Substrings; any match (case-insensitive) selects `target`.";
      };
      target = mkOption {
        type = target;
        description = "Target app for matching windows.";
      };
    };
  };
  rule = contains: target: { inherit contains target; };

  dropNulls = lib.filterAttrsRecursive (_: v: v != null);
  renderedConfig = settingsFormat.generate "stickpile-client.toml" (dropNulls s);
  canRunCli = pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform;
  validatedConfig =
    pkgs.runCommand "stickpile-client.toml"
      {
        nativeBuildInputs = [ cfg.package ];
        preferLocalBuild = true;
      }
      ''
        export HOME=$TMPDIR XDG_CONFIG_HOME=$TMPDIR XDG_STATE_HOME=$TMPDIR XDG_CACHE_HOME=$TMPDIR
        stickpile --config ${renderedConfig} config check
        cp ${renderedConfig} $out
      '';
in
{
  options.programs.stickpile = {
    enable = mkEnableOption "the stickpile client configuration and CLI";

    package = lib.mkPackageOption pkgs "stickpile-cli" { };

    desktop = {
      enable = mkEnableOption ''
        the stickpile-desktop quick-search popup, started hidden with the
        graphical session. Bind a compositor shortcut to `stickpile-desktop`
        to toggle it (running it again talks to the resident instance)'';
      package = lib.mkPackageOption pkgs "stickpile-desktop" { };
    };

    validateConfig = opt types.bool canRunCli ''
      Run `stickpile config check` on the generated file at build time.
      Defaults to on unless cross-compiling.
    '';

    configFile = mkOption {
      type = types.path;
      readOnly = true;
      default = if cfg.validateConfig then validatedConfig else renderedConfig;
      defaultText = "TOML generated from programs.stickpile.settings";
      description = "The generated configuration file.";
    };

    settings = mkOption {
      default = { };
      description = ''
        Client configuration, rendered to $XDG_CONFIG_HOME/stickpile/config.toml.
        Mirrors the client's `ClientConfig` one-to-one.
      '';
      type = types.submodule {
        options = {
          server_url = opt types.str "http://localhost:8080" "Base URL of the stickpile server.";
          token_file = hostPathOpt ''
            API token file (written by `stickpile login`, mode 0600).
            null: `$XDG_STATE_HOME/stickpile/token`.
          '';
          cache_dir = hostPathOpt "Offline cache. null: `$XDG_CACHE_HOME/stickpile`.";
          cache_max_mb = opt u64 512 ''
            Thumbnails and cached media files; least recently used are evicted
            beyond this.
          '';
          default_target = opt target "generic" ''
            Target app when the focused window is not recognised.
          '';
          prefer_stickers = opt types.bool false ''
            For targets with sticker support (Telegram), copy the sticker
            rendition by default. Toggle per copy with Shift+Enter in the popup
            or `--sticker`.
          '';
          timeout_secs = opt u64 5 ''
            Server request timeout before falling back to the offline cache.
          '';

          ui = section "Quick-search popup." {
            columns = opt types.ints.positive 6 "Grid columns.";
            thumb_px = opt (types.ints.between 32 4294967295) 128 "Thumbnail size (at least 32).";
            hide_on_copy = opt types.bool true "Hide the popup after copying.";
            start_hidden = opt types.bool false ''
              Start resident without showing the popup. The systemd user unit
              (`desktop.enable`) passes `--hidden` regardless, so a key-bound
              `stickpile-desktop` still shows the popup on first launch.
            '';
            recent_count = opt u32 24 "Items shown before typing: most recently used first.";
            max_results = opt u32 60 "Search results shown.";
          };

          focus = section "Focused-window detection (target app selection)." {
            detect = opt types.bool true ''
              Detect the focused window (Hyprland, Sway, X11) when the popup
              opens and map it to a target app with `rules`.
            '';
            rules =
              opt (types.listOf focusRule)
                [
                  (rule [
                    "telegram"
                    "ayugram"
                    "materialgram"
                    "64gram"
                    "kotatogram"
                  ] "telegram")
                  (rule [
                    "discord"
                    "vesktop"
                    "webcord"
                    "armcord"
                    "legcord"
                  ] "discord")
                  (rule [ "signal" ] "signal")
                  (rule [
                    "whatsapp"
                    "zapzap"
                    "wasistlos"
                  ] "whatsapp")
                  (rule [ "slack" ] "slack")
                ]
                ''
                  First match wins; matched case-insensitively against the
                  window class/app id and title. Setting this replaces the
                  whole default list.
                '';
          };

          clipboard = section "Clipboard offers (docs/clipboard.md)." {
            default = mkOption {
              type = offers;
              default = { };
              description = "Offers for any target without an override.";
            };
            targets = mkOption {
              type = types.attrsOf offers;
              default = { };
              example = {
                telegram.still = [ "text/uri-list" ];
              };
              description = ''
                Per-target overrides keyed by target name
                (${lib.concatStringsSep ", " targetNames}). Unset lists take the
                built-in defaults, not `default`.
              '';
            };
          };
        };
      };
    };
  };

  config = mkIf cfg.enable (
    lib.mkMerge [
      {
        assertions = [
          {
            assertion = lib.all (k: lib.elem k targetNames) (lib.attrNames s.clipboard.targets);
            message = "programs.stickpile.settings.clipboard.targets: keys must be one of ${lib.concatStringsSep ", " targetNames}.";
          }
        ];

        home.packages = [ cfg.package ];
        xdg.configFile."stickpile/config.toml".source = cfg.configFile;
      }

      (mkIf cfg.desktop.enable {
        home.packages = [ cfg.desktop.package ];

        # Resident process: it owns the Wayland clipboard after a copy, so it
        # must outlive the popup. Restarted when the configuration changes.
        systemd.user.services.stickpile-desktop = {
          Unit = {
            Description = "stickpile quick-search popup";
            Documentation = "https://github.com/Dolfost/stickpile";
            After = [ "graphical-session.target" ];
            PartOf = [ "graphical-session.target" ];
            X-Restart-Triggers = [ "${cfg.configFile}" ];
          };
          Service = {
            ExecStart = "${lib.getExe cfg.desktop.package} --hidden";
            Restart = "on-failure";
            RestartSec = 3;
          };
          Install.WantedBy = [ "graphical-session.target" ];
        };
      })
    ]
  );
}
