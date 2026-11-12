# NixOS module: services.stickpile
#
# `settings` mirrors the server's `Config` (crates/stickpile-server/src/config.rs)
# one-to-one: every field is an explicit, typed option with the same name, the
# same nesting and the same default. It is rendered to TOML with
# pkgs.formats.toml; options left at null are omitted, which is what the Rust
# `Option` fields expect. Three things keep the two from drifting:
#   - the `config-drift` flake check compares these defaults with the Rust
#     `Config::default()`, key by key (docs/nix.md);
#   - the `nixos-module` flake check runs `stickpile-server check-config` on the
#     example configuration;
#   - with `validateConfig` (default), every generated file is checked by the
#     real loader at build time, so a bad value fails `nixos-rebuild`, not the
#     service.
#
# Secrets are only ever given as paths (`*_file`). They are handed to the
# service with systemd LoadCredential, so they can stay root-only (agenix,
# sops-nix, a 0400 file) and never enter the Nix store.
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
    literalExpression
    ;

  cfg = config.services.stickpile;
  s = cfg.settings;
  settingsFormat = pkgs.formats.toml { };

  # A typed, documented option. Every one of them has the Rust default.
  opt =
    type: default: description:
    mkOption { inherit type default description; };
  optEx =
    type: default: example: description:
    mkOption {
      inherit
        type
        default
        example
        description
        ;
    };

  # A file path that must not end up in the world-readable store: a path
  # literal would be copied there by the TOML renderer, so it is turned into
  # a plain string first.
  hostPath = types.externalPath;
  toPathString = v: if v == null then null else toString v;
  secretOpt =
    example: description:
    mkOption {
      type = types.nullOr hostPath;
      default = null;
      inherit example;
      apply = toPathString;
      description = ''
        ${description}

        A path on the host, read by systemd (LoadCredential) as root and passed
        to the service as `/run/credentials/stickpile.service/<name>`. Never
        put the secret itself into the Nix configuration.
      '';
    };

  section =
    description: options:
    mkOption {
      type = types.submodule { inherit options; };
      default = { };
      inherit description;
    };

  u32 = types.ints.u32;
  u64 = types.ints.unsigned; # Nix integers are i64; the server rejects > u64 anyway.

  # settings path -> credential name, for every secret file option.
  secretFiles = [
    {
      path = [
        "trusted_proxy"
        "secret_file"
      ];
      name = "trusted-proxy-secret";
    }
    {
      path = [
        "auth"
        "initial_admin_password_file"
      ];
      name = "initial-admin-password";
    }
    {
      path = [
        "telegram"
        "bot_token_file"
      ];
      name = "telegram-bot-token";
    }
    {
      path = [
        "git"
        "ssh_key_file"
      ];
      name = "git-ssh-key";
    }
    {
      path = [
        "git"
        "https_token_file"
      ];
      name = "git-https-token";
    }
  ];
  activeSecrets = map (x: x // { source = lib.getAttrFromPath x.path s; }) (
    lib.filter (x: lib.getAttrFromPath x.path s != null) secretFiles
  );
  credentialsDir = "/run/credentials/stickpile.service";

  # What the service actually reads: secret paths point at its credentials.
  serviceSettings = lib.foldl' (
    acc: x: lib.recursiveUpdate acc (lib.setAttrByPath x.path "${credentialsDir}/${x.name}")
  ) s activeSecrets;

  # TOML has no null: unset options are left out and the server's default applies.
  dropNulls = lib.filterAttrsRecursive (_: v: v != null);
  renderedConfig = settingsFormat.generate "stickpile.toml" (dropNulls serviceSettings);

  canRunServer = pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform;
  validatedConfig =
    pkgs.runCommand "stickpile.toml"
      {
        nativeBuildInputs = [ cfg.package ];
        preferLocalBuild = true;
      }
      ''
        stickpile-server --config ${renderedConfig} check-config
        cp ${renderedConfig} $out
      '';

  dataDir = s.server.data_dir;
  # StateDirectory= handles anything below /var/lib; other locations get a
  # tmpfiles rule and an explicit ReadWritePaths= entry.
  stateDirectory =
    if lib.hasPrefix "/var/lib/" dataDir then lib.removePrefix "/var/lib/" dataDir else null;
  socketDir = if s.trusted_proxy.socket == null then null else dirOf s.trusted_proxy.socket;
  coveredByUnit =
    dir:
    lib.any (prefix: dir == prefix || lib.hasPrefix "${prefix}/" dir) [
      dataDir
      "/run/stickpile"
    ];
  listenPort = lib.toInt (lib.last (lib.splitString ":" s.server.listen));
in
{
  options.services.stickpile = {
    enable = mkEnableOption "stickpile, the self-hosted sticker and meme library";

    package = lib.mkPackageOption pkgs "stickpile-server" { };

    user = opt types.str "stickpile" ''
      User the server runs as. The default user is created by the module; a
      different one must exist already.
    '';

    group = opt types.str "stickpile" ''
      Group of the service. Members can reach the trusted unix socket when
      `settings.trusted_proxy.socket_mode` grants group access (default 0660).
    '';

    openFirewall = opt types.bool false ''
      Open the TCP port of `settings.server.listen` in the firewall. Not needed
      behind `tailscale serve` or a reverse proxy on the same host.
    '';

    validateConfig = opt types.bool canRunServer ''
      Run `stickpile-server check-config` on the generated file at build time,
      so an invalid configuration fails the rebuild instead of the service.
      Defaults to on unless cross-compiling.
    '';

    configFile = mkOption {
      type = types.path;
      readOnly = true;
      default = if cfg.validateConfig then validatedConfig else renderedConfig;
      defaultText = literalExpression "TOML generated from config.services.stickpile.settings";
      description = ''
        The generated configuration file passed to the service (also linked
        to /etc/stickpile/config.toml, the server's default `--config`).
        Secret paths in it point at the service's credentials directory.
      '';
    };

    settings = mkOption {
      default = { };
      description = ''
        Server configuration, rendered to TOML. Mirrors the server's `Config`
        one-to-one; see crates/stickpile-server/config.example.toml and
        docs/deployment.md.
      '';
      type = types.submodule {
        options = {
          server = section "Listeners, URLs and the state directory." {
            listen = opt types.str "127.0.0.1:8080" ''
              TCP listener (`address:port`). Tailscale identity headers are never
              trusted on it, unless `trusted_proxy.secret_file` is set and the
              request carries the secret from a loopback address.
            '';
            public_url = optEx types.str "http://localhost:8080" "https://stickpile.example.ts.net" ''
              Absolute external base URL, used in webhook and invitation links.
            '';
            data_dir = mkOption {
              type = hostPath;
              default = "/var/lib/stickpile";
              apply = toString;
              description = ''
                All state: `stickpile.db`, `media/`, `gitpacks/`, `tmp/`. Paths
                below /var/lib are created by systemd (StateDirectory=); anything
                else is created by a tmpfiles rule.
              '';
            };
            log = optEx types.str "info" "stickpile_server=debug,info" ''
              tracing filter.
            '';
            secure_cookies = opt types.bool true ''
              Mark cookies `Secure`. Only disable for plain-http local development.
            '';
          };

          trusted_proxy = section "The trusted `tailscale serve` path (identity headers)." {
            socket = mkOption {
              type = types.nullOr hostPath;
              default = null;
              example = "/run/stickpile/tailscale.sock";
              apply = toPathString;
              description = ''
                Extra unix-socket listener for `tailscale serve unix:<socket>`.
                Requests arriving here have their `Tailscale-User-Login` /
                `Tailscale-User-Name` headers trusted. Only root and the service
                user (and group, see `socket_mode`) can connect.
                /run/stickpile (RuntimeDirectory=) and `server.data_dir` are
                writable by the service.
              '';
            };
            socket_mode = opt types.str "0660" ''
              Octal permission bits for `socket`, e.g. "0660".
            '';
            secret_file = secretOpt "/run/secrets/stickpile-proxy-secret" ''
              Alternative for a TCP reverse proxy on loopback: the proxy must send
              `secret_header: <contents of this file>`; only then, and only from a
              loopback peer, are identity headers trusted on the TCP listener.
            '';
            secret_header = opt types.str "X-Stickpile-Proxy-Secret" ''
              Header carrying the shared secret of `secret_file`.
            '';
          };

          auth = section "Accounts, sessions and tailnet logins." {
            registration =
              opt
                (types.enum [
                  "closed"
                  "invite"
                  "open"
                ])
                "closed"
                ''
                  `closed`: only admins create accounts (plus tailscale auto-create
                  if enabled). `invite`: anyone with an invitation link. `open`:
                  anyone who can reach the server.
                '';
            tailscale_autocreate = opt types.bool true ''
              Create a stickpile user the first time an unknown tailnet login
              arrives through the trusted proxy.
            '';
            tailscale_admins = optEx (types.listOf types.str) [ ] [ "alice@github" ] ''
              Tailnet logins that get the admin role when auto-created or on
              every login.
            '';
            session_ttl_days = opt types.ints.positive 30 ''
              Lifetime of web sessions.
            '';
            min_password_length = opt (types.ints.between 4 128) 8 ''
              Shortest password accepted when one is set (registration,
              settings, admin). Existing passwords are not affected.
            '';
            totp_issuer = opt types.str "stickpile" ''
              Issuer shown in authenticator apps.
            '';
            initial_admin_username = optEx (types.nullOr types.str) null "admin" ''
              Bootstrap: if the database has no users, create this admin on
              start. Requires `initial_admin_password_file`.
            '';
            initial_admin_password_file = secretOpt "/run/secrets/stickpile-admin-password" ''
              Password of `initial_admin_username`.
            '';
          };

          limits = section "Upload, decode and rate limits." {
            max_upload_mb = opt u64 50 "Per uploaded file.";
            max_request_mb = opt u64 500 ''
              Per request (multi-file uploads, imports). Must be at least
              `max_upload_mb`.
            '';
            max_pixels = opt u64 40000000 "Decoded still/frame size limit (width × height).";
            max_animation_frames = opt u32 3000 "Frames per animation.";
            max_video_seconds = opt u32 120 "Video duration.";
            login_per_minute = opt u32 10 "Login attempts per client IP and per username.";
            upload_per_minute = opt u32 120 "Uploads per user.";
          };

          media = section "Media pipeline." {
            ffmpeg = opt types.str "ffmpeg" ''
              ffmpeg binary, invoked with fixed arguments on server-generated
              paths only (videos and animated variants). The packaged server has
              ffmpeg-headless on its PATH.
            '';
            ffprobe = opt types.str "ffprobe" "ffprobe binary.";
            ffmpeg_timeout_secs = opt u64 120 "Timeout for one ffmpeg/ffprobe run.";
            thumb_px = opt u32 256 "Longest side of grid thumbnails.";
            discord_gif_max_mb = opt u64 10 "Size budget of the `discord_gif` variant.";
            workers = opt types.ints.positive 2 ''
              Concurrent background jobs (variants, enrichment, git, telegram).
            '';
            default_recommended_px = opt u32 512 ''
              Default recommended longest side for sets that do not set one.
            '';
            near_duplicate_distance = opt (types.ints.between 0 32) 8 ''
              Max pHash Hamming distance considered a near-duplicate.
            '';
          };

          ai = section "CLIP enrichment (search by image content, keyword/emoji/vibe suggestions)." {
            enabled = opt types.bool false "Run CLIP on new items.";
            model_dir = mkOption {
              type = types.path;
              default = "/var/lib/stickpile/models";
              example = literalExpression "inputs.stickpile.packages.\${pkgs.system}.models";
              description = ''
                Directory with the ONNX models and tokenizers (layout: docs/ai.md).
                The flake's `models` package is such a directory.
              '';
            };
            text_encoder =
              opt
                (types.enum [
                  "clip"
                  "multilingual"
                ])
                "clip"
                ''
                  `clip`: OpenAI CLIP ViT-B/32 text tower (English).
                  `multilingual`: sentence-transformers clip-ViT-B-32-multilingual-v1
                  (50+ languages), aligned to the same image embedding space.
                '';
            onnxruntime_library = mkOption {
              type = types.nullOr types.path;
              default = null;
              example = literalExpression ''"''${pkgs.onnxruntime}/lib/libonnxruntime.so"'';
              description = ''
                Path to libonnxruntime. Default: `ORT_DYLIB_PATH`, which the
                packaged server sets to nixpkgs' onnxruntime.
              '';
            };
            threads = opt u32 2 "Inference threads.";
            vocabulary_file = mkOption {
              type = types.nullOr types.path;
              default = null;
              description = ''
                TOML with `keywords`, `emojis`, `vibes` tables replacing the
                built-in zero-shot vocabulary (format: docs/ai.md).
              '';
            };
            max_keywords = opt u32 5 "Keywords suggested per item.";
            max_emojis = opt u32 3 "Emojis suggested per item.";
          };

          telegram = section "Telegram sticker pack import/export (Bot API)." {
            enabled = opt types.bool false "Enable Telegram import and export. Needs `bot_token_file`.";
            bot_token_file = secretOpt "/run/secrets/stickpile-telegram-bot-token" "Bot API token.";
            api_url = opt types.str "https://api.telegram.org" ''
              Bot API base URL (override for a local Bot API server or tests).
            '';
            default_owner_user_id = optEx (types.nullOr types.int) null 123456789 ''
              Telegram user id that owns packs created on export, unless the
              exporting user has set their own in the web UI.
            '';
          };

          git = section "Git packs: sets bound to a remote repository." {
            enabled = opt types.bool true "Allow binding sets to git repositories.";
            pull_interval_minutes = opt u32 60 "Re-pull interval.";
            allowed_schemes =
              opt
                (types.listOf (
                  types.enum [
                    "https"
                    "http"
                    "ssh"
                    "file"
                  ]
                ))
                [ "https" ]
                ''
                  URL schemes users may bind: `https`, `http`, `ssh`, `file`
                  (tests only).
                '';
            ssh_key_file = secretOpt "/run/secrets/stickpile-git-ssh-key" ''
              Private key for `ssh` remotes and pushing.
            '';
            https_token_file = secretOpt "/run/secrets/stickpile-git-token" ''
              HTTPS token used as password when pushing (`push` mode).
            '';
            https_username = opt types.str "x-access-token" "HTTPS username used with `https_token_file`.";
            author_name = opt types.str "stickpile" "Commit author name.";
            author_email = opt types.str "stickpile@localhost" "Commit author email.";
            max_repo_mb = opt u64 1024 "Refuse repositories whose checkout exceeds this size.";
          };
        };
      };
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = (s.auth.initial_admin_username == null) == (s.auth.initial_admin_password_file == null);
        message = "services.stickpile.settings.auth: initial_admin_username and initial_admin_password_file go together.";
      }
      {
        assertion = !s.telegram.enabled || s.telegram.bot_token_file != null;
        message = "services.stickpile.settings.telegram.enabled requires telegram.bot_token_file.";
      }
    ];

    users.users = mkIf (cfg.user == "stickpile") {
      stickpile = {
        isSystemUser = true;
        inherit (cfg) group;
        home = dataDir;
        description = "stickpile server";
      };
    };
    users.groups = mkIf (cfg.group == "stickpile") { stickpile = { }; };

    # Admin subcommands (`sudo -u stickpile stickpile-server …`) find the
    # configuration at the binary's default path.
    environment.etc."stickpile/config.toml".source = cfg.configFile;
    environment.systemPackages = [ cfg.package ];

    systemd.tmpfiles.settings."10-stickpile" = mkIf (stateDirectory == null) {
      ${dataDir}.d = {
        inherit (cfg) user group;
        mode = "0750";
      };
    };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ listenPort ];

    systemd.services.stickpile = {
      description = "stickpile sticker and meme library";
      documentation = [ "https://github.com/Dolfost/stickpile/blob/main/docs/deployment.md" ];
      wantedBy = [ "multi-user.target" ];
      # Git pulls and Telegram calls start right away.
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];

      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} --config ${cfg.configFile} serve";
        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = 5;

        StateDirectory = mkIf (stateDirectory != null) stateDirectory;
        StateDirectoryMode = "0750";
        # Home of the trusted socket when it is not in data_dir.
        RuntimeDirectory = "stickpile";
        RuntimeDirectoryMode = "0750";
        WorkingDirectory = dataDir;
        ReadWritePaths =
          lib.optional (stateDirectory == null) dataDir
          ++ lib.optional (socketDir != null && !coveredByUnit socketDir) socketDir;
        LoadCredential = map (x: "${x.name}:${x.source}") activeSecrets;
        UMask = "0027";

        # Hardening. `systemd-analyze security stickpile` rates this 1.4 (OK);
        # what remains is network access, which the server needs.
        CapabilityBoundingSet = if listenPort < 1024 then [ "CAP_NET_BIND_SERVICE" ] else [ "" ];
        AmbientCapabilities = lib.optional (listenPort < 1024) "CAP_NET_BIND_SERVICE";
        NoNewPrivileges = true;
        PrivateTmp = true;
        PrivateDevices = true;
        DevicePolicy = "closed";
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        # Not ProcSubset=pid: ONNX Runtime reads /proc/cpuinfo to pick kernels.
        ProtectProc = "invisible";
        RestrictAddressFamilies = [
          "AF_UNIX"
          "AF_INET"
          "AF_INET6"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        RemoveIPC = true;
        LockPersonality = true;
        # Neither ONNX Runtime's CPU provider nor ffmpeg generates code at
        # runtime (no JIT), so W^X holds.
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
        ];
        SystemCallErrorNumber = "EPERM";
      };
    };
  };
}
