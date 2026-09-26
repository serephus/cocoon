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
    types
    ;

  settingsEnv = lib.mapAttrs (_: toString) (filterAttrs (_: value: value != null) cfg.settings);

  # Port used for `openFirewall`, taken from the bind address.
  bindPort = lib.toInt (lib.last (lib.splitString ":" (toString cfg.settings.COCOON_BIND)));
in
{
  options.services.cocoon-paste = {
    enable = mkEnableOption "Cocoon, a time-locked pastebin (Telegram bot + web)";

    package = mkOption {
      type = types.package;
      description = "The cocoon package to run.";
    };

    settings = mkOption {
      default = { };
      description = ''
        Configuration rendered as `COCOON_*` environment variables. Must
        include `COCOON_PUBLIC_URL` so the bot can register its webhook.

        Do not put secrets here; use {option}`botTokenFile` and
        {option}`webhookSecretFile`.
      '';
      type = types.submodule {
        freeformType = types.attrsOf (types.nullOr (types.either types.str types.path));
        options = {
          COCOON_BIND = mkOption {
            type = types.str;
            default = "127.0.0.1:3000";
            description = "Address the HTTP server listens on.";
          };

          COCOON_DB = mkOption {
            type = types.str;
            default = "/var/lib/cocoon-paste/cocoon.db";
            description = "Path to the SQLite database.";
          };

          COCOON_PUBLIC_URL = mkOption {
            type = types.str;
            example = "https://paste.example.com";
            description = "Public HTTPS base URL used to build the webhook URL.";
          };

          COCOON_TELEGRAM_WEBHOOK_PATH = mkOption {
            type = types.str;
            default = "/telegram/webhook";
            description = "Path prefix under which the webhook is served.";
          };
        };
      };
    };

    botTokenFile = mkOption {
      type = types.path;
      example = "/run/secrets/cocoon_bot_token";
      description = "File containing the Telegram bot token.";
    };

    webhookSecretFile = mkOption {
      type = types.path;
      example = "/run/secrets/cocoon_webhook_secret";
      description = "File containing the Telegram webhook secret.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Open the bind port in the firewall. Only needed when the reverse proxy
        runs on another host; a local proxy can reach the loopback default.
      '';
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.settings ? COCOON_PUBLIC_URL;
        message = "services.cocoon-paste.settings.COCOON_PUBLIC_URL must be set.";
      }
    ];

    systemd.services.cocoon-paste = {
      description = "Cocoon time-locked pastebin (bot + web)";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      # The two secrets are systemd credentials; `%d` is their directory.
      environment = settingsEnv // {
        COCOON_TELEGRAM_BOT_TOKEN_FILE = "%d/bot_token";
        COCOON_TELEGRAM_WEBHOOK_SECRET_FILE = "%d/webhook_secret";
      };

      serviceConfig = {
        ExecStart = getExe cfg.package;
        LoadCredential = [
          "bot_token:${cfg.botTokenFile}"
          "webhook_secret:${cfg.webhookSecretFile}"
        ];

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
