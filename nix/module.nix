# NixOS module for the Mise sync server. TLS stays Caddy's job; point a
# reverse_proxy at services.mise.listen.
self: { config, lib, pkgs, ... }:
let
  cfg = config.services.mise;
in
{
  options.services.mise = {
    enable = lib.mkEnableOption "the Mise cookbook server";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "mise.packages.\${system}.default";
      description = "The mise package providing the mise-server binary.";
    };

    listen = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:7920";
      description = "Address to bind; put Caddy in front for TLS.";
    };

    root = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/mise/cookbook";
      description = "Corpus root: mise.db, photos/, export/.";
    };

    tokenFile = lib.mkOption {
      type = lib.types.path;
      description = ''
        File containing the static bearer token (at least 16 characters),
        handed to the service via systemd credentials. Keep it out of the
        Nix store — with agenix:
        `services.mise.tokenFile = config.age.secrets.mise-token.path;`
      '';
    };

    init = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Create an empty corpus on first start.";
    };

    anthropicKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        File containing the Anthropic API key for the assistant, handed to
        the service via systemd credentials. Keep it out of the Nix store —
        with agenix:
        `services.mise.anthropicKeyFile = config.age.secrets.anthropic.path;`
        Leave null to run sync-only.
      '';
    };

    model = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        Model the assistant uses. Null (the default) omits --model, so
        the binary's own default stays authoritative — duplicating the
        constant here meant a bumped default never reached deployed
        servers.
      '';
    };

    webApp = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.web;
      defaultText = lib.literalExpression "mise.packages.\${system}.web";
      description = "Built web app served at /; null for sync/API only.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.mise = {
      isSystemUser = true;
      group = "mise";
      home = "/var/lib/mise";
    };
    users.groups.mise = { };

    systemd.services.mise = {
      description = "Mise cookbook server";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      # The markdown export shells out to system git.
      path = [ pkgs.git ];
      serviceConfig = {
        ExecStart =
          "${lib.getExe cfg.package} --root ${cfg.root} --listen ${cfg.listen}"
          + lib.optionalString (cfg.model != null) " --model ${cfg.model}"
          + lib.optionalString (cfg.webApp != null) " --static-dir ${cfg.webApp}"
          + lib.optionalString cfg.init " --init";
        User = "mise";
        Group = "mise";
        StateDirectory = "mise";
        # The corpus is the most private data here — pantry and fridge
        # contents, the cook log, full assistant transcripts, and the shelf
        # photos deliberately kept out of git. Without these it was 0755/0644
        # and every other local user could read all of it.
        StateDirectoryMode = "0700";
        UMask = "0077";
        LoadCredential =
          [ "token:${cfg.tokenFile}" ]
          ++ lib.optional (cfg.anthropicKeyFile != null) "anthropic:${cfg.anthropicKeyFile}";
        # Runs with full privileges (the "+" prefix), outside the sandbox:
        # it creates the corpus root wherever cfg.root points — so the
        # ReadWritePaths grant below always names an existing directory —
        # and tightens what an earlier, looser run already wrote (UMask
        # only governs files created from here on).
        ExecStartPre = "+" + pkgs.writeShellScript "mise-prepare-corpus" ''
          mkdir -p ${lib.escapeShellArg cfg.root}
          chown mise:mise ${lib.escapeShellArg cfg.root}
          chmod -R go-rwx ${lib.escapeShellArg cfg.root}
        '';
        Restart = "on-failure";
        RestartSec = 5;
        # The drain has a bounded budget; the worst case it exists to avoid
        # is a stop landing inside the export's rewrite-then-commit sequence.
        TimeoutStopSec = 30;

        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        # The sandbox follows the option: a root outside /var/lib/mise
        # (say /srv/cookbook) must be writable too, not a runtime EROFS
        # restart loop. StateDirectory already covers the default.
        ReadWritePaths = [ cfg.root ];
        CapabilityBoundingSet = [ ];
        RestrictSUIDSGID = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        LockPersonality = true;

        # This is the second line of defence for `fetch_url`, whose own guard
        # is documented as not a bulletproof SSRF boundary. All of it is
        # compatible with a Rust server that execs git.
        SystemCallFilter = [ "@system-service" ];
        SystemCallArchitectures = "native";
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        RestrictNamespaces = true;
        PrivateDevices = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictRealtime = true;
        ProtectProc = "invisible";
        RemoveIPC = true;
      };
    };
  };
}
