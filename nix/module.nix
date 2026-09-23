{
  config,
  lib,
  ...
}:

let
  cfg = config.services.cocoon-paste;

  inherit (lib)
    filterAttrs
    getExe
    mkEnableOption
    mkIf
    mkOption
    optionalAttrs
    types
    ;

  # `settings` rendered as environment variables, dropping nulls first.
  settingsEnv = lib.mapAttrs (_: value: toString value) (
    filterAttrs (_: value: value != null) cfg.settings
  );

  # Port used for `openFirewall`, taken from the bind address.
  bindPort = lib.toInt (lib.last (lib.splitString ":" (toString cfg.settings.COCOON_BIND)));
in
{
  options.services.cocoon-paste = {
    enable = mkEnableOption "Cocoon, a self-hostable time-locked pastebin";

    package = mkOption {
      type = types.package;
      description = "The cocoon package to run.";
    };

    settings = mkOption {
      default = { };
      description = ''
        Common configuration, rendered as `COCOON_*` environment variables.
        `COCOON_BIND` and `COCOON_DB` are the usual ones; any other
        `COCOON_*` variable can be supplied through the free-form attribute
        set.

        Secrets do not belong here: these values are written to the Nix
        store. Use {option}`secretFile` instead.
      '';
      type = types.submodule {
        freeformType = types.attrsOf (types.nullOr (types.either types.str types.path));
        options = {
          COCOON_BIND = mkOption {
            type = types.str;
            default = "127.0.0.1:3000";
            example = "0.0.0.0:3000";
            description = "Address the HTTP server listens on.";
          };

          COCOON_DB = mkOption {
            type = types.str;
            default = "/var/lib/cocoon-paste/cocoon.db";
            description = "Path to the SQLite database file.";
          };
        };
      };
    };

    secretFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/cocoon_hmac";
      description = ''
        File containing the HMAC secret (at least 16 bytes). It is passed to
        the service as a systemd credential named `hmac`, so it never enters
        the Nix store.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/cocoon.env";
      description = ''
        Optional systemd environment file for additional variables, useful
        for secrets other than a credential (for example `COCOON_HMAC_SECRET`).
        It is fine to combine this with `secretFile` as long as the file does
        not also define the HMAC variable.
      '';
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the configured port in the firewall.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion =
          cfg.secretFile != null
          || cfg.environmentFile != null
          || cfg.settings ? COCOON_HMAC_SECRET;
        message = ''
          services.cocoon-paste needs an HMAC secret: set `secretFile`
          (recommended), `environmentFile`, or `settings.COCOON_HMAC_SECRET`.
        '';
      }
    ];

    systemd.services.cocoon-paste = {
      description = "Cocoon time-locked pastebin";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      environment =
        settingsEnv
        // optionalAttrs (cfg.secretFile != null) {
          # `%d` expands to the systemd credentials directory.
          COCOON_HMAC_SECRET_FILE = "%d/hmac";
        };

      serviceConfig = {
        ExecStart = getExe cfg.package;
        EnvironmentFile = mkIf (cfg.environmentFile != null) cfg.environmentFile;
        LoadCredential = mkIf (cfg.secretFile != null) [ "hmac:${cfg.secretFile}" ];

        Restart = "on-failure";
        RestartSec = 2;

        DynamicUser = true;
        StateDirectory = "cocoon-paste";

        # Hardening.
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
      };
    };

    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ bindPort ];
  };
}
