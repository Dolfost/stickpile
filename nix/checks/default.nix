# Module checks: the Nix side of the "Nix-to-TOML mirror" (docs/nix.md).
#
#   config-drift         module defaults == Rust defaults, key by key, both ways
#                        (client: the documented defaults; nothing set renders
#                        an empty file)
#   nixos-module         example NixOS config: evaluates, renders, the real
#                        server loader accepts it, the unit is hardened
#   home-manager-module  example home-manager config: evaluates, renders, the
#                        real client loader accepts it, the user unit exists
#   deploy-configs       deploy/quadlet/config.toml and the image's built-in config
#                        pass the real loader
#   nixos-vm             (x86_64-linux) VM: service starts, /healthz answers
#                        on TCP and on the trusted unix socket
{
  pkgs,
  self,
  nixpkgs,
  home-manager,
  rust,
}:
let
  inherit (pkgs) lib;
  system = pkgs.stdenv.hostPlatform.system;
  inherit (self.packages.${system}) stickpile-server stickpile-cli;

  evalNixos =
    modules:
    nixpkgs.lib.nixosSystem {
      modules = [
        self.nixosModules.default
        {
          nixpkgs.hostPlatform = system;
          boot.isContainer = true;
          system.stateVersion = "26.05";
        }
      ]
      ++ modules;
    };

  evalHome =
    modules:
    home-manager.lib.homeManagerConfiguration {
      inherit pkgs;
      modules = [
        self.homeManagerModules.default
        {
          home = {
            username = "alice";
            homeDirectory = "/home/alice";
            stateVersion = "26.05";
          };
        }
      ]
      ++ modules;
    };

  # Nothing set: the module's defaults. Rendering only, no build-time check,
  # so the drift guard does not wait for the full server/CLI builds.
  serverDefaults =
    (evalNixos [
      {
        services.stickpile = {
          enable = true;
          validateConfig = false;
        };
      }
    ]).config.services.stickpile;
  clientHome = evalHome [
    {
      programs.stickpile = {
        enable = true;
        validateConfig = false;
      };
    }
  ];
  clientDefaults = clientHome.config.programs.stickpile;

  # The client's settings are null unless set (only set keys are rendered, so
  # the popup locks only those); each documents the client's default as JSON
  # in `defaultText`. Collected here, keyed like `settings`, for the drift
  # guard.
  documentedDefaults =
    opts:
    lib.mapAttrs (
      _: o:
      if !(lib.isOption o) then
        documentedDefaults o
      else if o.type.name == "submodule" then
        documentedDefaults (o.type.getSubOptions o.loc)
      else if o ? defaultText then
        builtins.fromJSON o.defaultText.text
      else
        o.default
    ) (removeAttrs opts [ "_module" ]);
  clientDocumentedDefaults = documentedDefaults (
    clientHome.options.programs.stickpile.settings.type.getSubOptions [ ]
  );

  # Built against the workspace lockfile and the real config sources.
  driftGuard = rust.craneLib.buildPackage {
    pname = "stickpile-config-drift";
    version = "0";
    src = lib.fileset.toSource {
      root = ../..;
      fileset = lib.fileset.unions [
        rust.srcFileset
        ../drift-guard
      ];
    };
    cargoVendorDir = rust.craneLib.vendorCargoDeps { cargoLock = ../../Cargo.lock; };
    cargoArtifacts = null;
    strictDeps = true;
    # Resolve offline against the vendored workspace dependencies, pinned to
    # the workspace's versions.
    postPatch = ''
      cp Cargo.lock nix/drift-guard/Cargo.lock
    '';
    cargoExtraArgs = "--offline --manifest-path nix/drift-guard/Cargo.toml --target-dir target";
    # crane's default install only picks binaries of the root workspace.
    installPhaseCommand = ''
      install -Dm755 target/release/stickpile-config-drift -t $out/bin
    '';
    doCheck = false;
    meta.mainProgram = "stickpile-config-drift";
  };

  example = evalNixos [ ../examples/nixos.nix ];
  exampleCfg = example.config.services.stickpile;
  exampleUnit = example.config.systemd.units."stickpile.service".unit;

  home = evalHome [ ../examples/home-manager.nix ];
  homeCfg = home.config.programs.stickpile;
  homeFiles = home.config.xdg.configFile;
in
{
  inherit driftGuard;

  checks = {
    config-drift =
      pkgs.runCommand "stickpile-config-drift"
        {
          nativeBuildInputs = [ driftGuard ];
          serverJson = pkgs.writeText "server-defaults.json" (builtins.toJSON serverDefaults.settings);
          clientJson = pkgs.writeText "client-defaults.json" (builtins.toJSON clientDocumentedDefaults);
        }
        ''
          rc=0
          stickpile-config-drift server $serverJson ${serverDefaults.configFile} || rc=1
          stickpile-config-drift client $clientJson ${clientDefaults.configFile} || rc=1
          # Nothing set renders nothing, so the popup locks nothing.
          if grep -q '[^[:space:]]' ${clientDefaults.configFile}; then
            echo "programs.stickpile.settings with nothing set renders keys:"
            cat ${clientDefaults.configFile}
            rc=1
          fi
          [ $rc = 0 ] && touch $out
        '';

    nixos-module =
      pkgs.runCommand "stickpile-nixos-module"
        {
          nativeBuildInputs = [ stickpile-server ];
          # Forces evaluation of the whole system, assertions included.
          toplevel = builtins.unsafeDiscardStringContext example.config.system.build.toplevel.drvPath;
          configFile = exampleCfg.configFile;
          unit = "${exampleUnit}/stickpile.service";
        }
        ''
          set -euo pipefail
          echo "system: $toplevel"
          cat "$configFile"
          stickpile-server --config "$configFile" check-config

          # Secrets reach the service as credentials, never as values.
          grep -qx 'bot_token_file = "/run/credentials/stickpile.service/telegram-bot-token"' "$configFile"
          grep -q '^LoadCredential=telegram-bot-token:/run/secrets/stickpile-telegram-bot-token$' "$unit"
          grep -q '^LoadCredential=initial-admin-password:/run/secrets/stickpile-admin-password$' "$unit"
          for line in 'ProtectSystem=strict' 'ProtectHome=true' 'PrivateTmp=true' 'NoNewPrivileges=true' \
                      'MemoryDenyWriteExecute=true' 'StateDirectory=stickpile' 'CapabilityBoundingSet=' \
                      'RestrictAddressFamilies=AF_UNIX' 'RestrictAddressFamilies=AF_INET' \
                      'RestrictAddressFamilies=AF_INET6' 'SystemCallFilter=@system-service'; do
            grep -qx "$line" "$unit" || { echo "missing from unit: $line"; cat "$unit"; exit 1; }
          done
          grep -q "^ExecStart=.*--config $configFile serve" "$unit"
          touch $out
        '';

    home-manager-module =
      pkgs.runCommand "stickpile-home-manager-module"
        {
          nativeBuildInputs = [ stickpile-cli ];
          activation = builtins.unsafeDiscardStringContext home.activationPackage.drvPath;
          configFile = homeFiles."stickpile/config.toml".source;
          unit = homeFiles."systemd/user/stickpile-desktop.service".source;
        }
        ''
          set -euo pipefail
          echo "home: $activation"
          cat "$configFile"
          export HOME=$TMPDIR XDG_CONFIG_HOME=$TMPDIR XDG_STATE_HOME=$TMPDIR XDG_CACHE_HOME=$TMPDIR
          stickpile --config "$configFile" config check
          [ "$configFile" = "${homeCfg.configFile}" ]
          # Set with lib.mkDefault: still set, so rendered (and locked in the popup).
          grep -qx 'timeout_secs = 7' "$configFile"
          grep -qx 'start_tab = "favorites"' "$configFile"
          grep -q '^ExecStart=.*/bin/stickpile-desktop --hidden$' "$unit"
          grep -q '^PartOf=graphical-session.target$' "$unit"
          touch $out
        '';

    # The configs shipped for the container: the quadlet's example and the
    # one baked into the image.
    deploy-configs =
      pkgs.runCommand "stickpile-deploy-configs"
        {
          nativeBuildInputs = [ stickpile-server ];
          quadletConfig = ../../deploy/quadlet/config.toml;
          imageConfig = self.packages.${system}.container.defaultConfig;
        }
        ''
          stickpile-server --config "$quadletConfig" check-config
          stickpile-server --config "$imageConfig" check-config
          touch $out
        '';
  }
  // lib.optionalAttrs (system == "x86_64-linux") {
    nixos-vm = import ./vm.nix { inherit pkgs self; };
  };
}
