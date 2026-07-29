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
          + lib.optionalString cfg.init " --init";
        User = "mise";
        Group = "mise";
        StateDirectory = "mise";
        LoadCredential = "token:${cfg.tokenFile}";
        Restart = "on-failure";
        RestartSec = 5;

        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [ "/var/lib/mise" ];
        CapabilityBoundingSet = [ ];
        RestrictSUIDSGID = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        LockPersonality = true;
      };
    };
  };
}
