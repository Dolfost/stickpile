# home-manager module: programs.stickpile
#
# `settings` mirrors the client's `ClientConfig`
# (crates/stickpile-core/src/config.rs) one-to-one: same names, same nesting.
# It is written to $XDG_CONFIG_HOME/stickpile/config.toml, read by both the
# `stickpile` CLI and `stickpile-desktop`. Every setting defaults to null
# (unset) and only the ones set are written, because the popup's Settings page
# locks the keys this read-only file sets and saves the rest to settings.toml
# next to it. Each option documents the client's default in `defaultText`
# (JSON); the `config-drift` flake check compares those with
# `ClientConfig::default()`. With `validateConfig` (default) the generated
# file is checked by `stickpile config check` at build time.
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
  # A client setting: unset (null) unless defined, so only what the user sets
  # reaches config.toml. `default` is the client's own default.
  setting =
    type: default: description:
    mkOption {
      type = types.nullOr type;
      default = null;
      defaultText = lib.literalExpression (builtins.toJSON default);
      inherit description;
    };
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

  # stickpile_core::config::TABS
  tabNames = [
    "search"
    "recent"
    "most_used"
    "packs"
    "favorites"
    "me"
    "settings"
  ];

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
    still = setting (types.listOf mime) [
      "image/png"
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers for still images.";
    animation = setting (types.listOf mime) [
      "image/gif"
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers for GIFs and animated WebP.";
    video = setting (types.listOf mime) [
      "text/uri-list"
      "x-special/gnome-copied-files"
    ] "Offers for videos.";
    sticker = setting (types.listOf mime) [
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

  # Unset settings are null: drop them, then the fixed sections they leave
  # empty. An empty `clipboard.targets.<name>` stays, since it means "the
  # built-in offers for that target".
  dropNulls = lib.filterAttrsRecursive (_: v: v != null);
  dropEmpty = lib.filterAttrs (_: v: v != { });
  rendered =
    let
      set = dropNulls s;
    in
    dropEmpty (set // { clipboard = dropEmpty set.clipboard; });
  renderedConfig = settingsFormat.generate "stickpile-client.toml" rendered;
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
        Mirrors the client's `ClientConfig` one-to-one. Settings left unset
        (null) are not written: the client uses its default (shown as each
        option's default) and the popup's Settings page can change them,
        saving to settings.toml next to config.toml. Settings set here are
        locked in the popup.
      '';
      type = types.submodule {
        options = {
          server_url = setting types.str "http://localhost:8080" "Base URL of the stickpile server.";
          token_file = hostPathOpt ''
            API token file (written by `stickpile login`, mode 0600).
            null: `$XDG_STATE_HOME/stickpile/token`.
          '';
          cache_dir = hostPathOpt "Offline cache. null: `$XDG_CACHE_HOME/stickpile`.";
          cache_max_mb = setting u64 512 ''
            Thumbnails and cached media files; least recently used are evicted
            beyond this.
          '';
          default_target = setting target "generic" ''
            Target app when the focused window is not recognised.
          '';
          prefer_stickers = setting types.bool false ''
            For targets with sticker support (Telegram), copy the sticker
            rendition by default. Toggle per copy with Shift+Enter in the popup
            or `--sticker`.
          '';
          timeout_secs = setting u64 5 ''
            Server request timeout before falling back to the offline cache.
          '';

          ui = section "Quick-search popup." {
            columns = setting types.ints.positive 6 "Grid columns.";
            thumb_px = setting (types.ints.between 32 4294967295) 128 "Thumbnail size (at least 32).";
            hide_on_copy = setting types.bool true "Hide the popup after copying.";
            start_hidden = setting types.bool false ''
              Start resident without showing the popup. The systemd user unit
              (`desktop.enable`) passes `--hidden` regardless, so a key-bound
              `stickpile-desktop` still shows the popup on first launch.
            '';
            recent_count = setting u32 24 "Items shown before typing: most recently used first.";
            max_results = setting u32 60 "Search results shown.";
            modal = setting types.bool true ''
              Vim-style Insert/Normal modes in the popup; false keeps the plain
              keys.
            '';
            start_tab = setting (types.enum tabNames) "search" ''
              Tab shown when the popup opens.
            '';
          };

          copy = section "Copy-time image adjustments (popup and `stickpile copy`)." {
            square =
              setting
                (types.enum [
                  "off"
                  "transparent"
                  "white"
                ])
                "off"
                ''
                  Pad non-square stills and Telegram sticker renditions to a
                  centred square with transparent or white margins. Nothing is
                  scaled or cropped; stickers stay 512 px WebP.
                '';
            background =
              setting
                (types.enum [
                  "keep"
                  "white"
                ])
                "keep"
                ''
                  `white` fills the transparent pixels of stills copied as
                  images (the image data and the copied file). Stickers keep
                  their transparency.
                '';
          };

          focus = section "Focused-window detection (target app selection)." {
            detect = setting types.bool true ''
              Detect the focused window (Hyprland, Sway, X11) when the popup
              opens and map it to a target app with `rules`.
            '';
            rules =
              setting (types.listOf focusRule)
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
