{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.dictator;
  settingsFormat = pkgs.formats.json { };
  displayPackages =
    if cfg.displayServer == "x11" then
      [
        pkgs.xclip
        pkgs.xdotool
      ]
    else if cfg.displayServer == "wayland" then
      [
        pkgs.wl-clipboard
        pkgs.coreutils
        pkgs.wtype
      ]
    else
      [
        pkgs.xclip
        pkgs.xdotool
        pkgs.wl-clipboard
        pkgs.coreutils
        pkgs.wtype
      ];
  pathPackages = displayPackages ++ [ pkgs.pulseaudio ] ++ cfg.extraPathPackages;
  defaultPassEnvironment = [
    "DISPLAY"
    "XAUTHORITY"
    "DBUS_SESSION_BUS_ADDRESS"
    "WAYLAND_DISPLAY"
    "XDG_RUNTIME_DIR"
    "NIRI_SOCKET"
  ];
  configSource =
    if cfg.configFile != null then
      cfg.configFile
    else if cfg.settings != null then
      settingsFormat.generate "dictator-config.json" cfg.settings
    else
      null;
  extraArgs = lib.escapeShellArgs cfg.extraArgs;
  xdgRoot = variable: fallback: cfg.environment.${variable} or fallback;
  configRoot = xdgRoot "XDG_CONFIG_HOME" config.xdg.configHome;
  dataRoot = xdgRoot "XDG_DATA_HOME" config.xdg.dataHome;
  stateRoot = xdgRoot "XDG_STATE_HOME" config.xdg.stateHome;
  writableAppDirectories = [
    "${configRoot}/dictator"
    "${dataRoot}/dictator"
    "${stateRoot}/dictator"
  ];
  prepareWritableAppDirectories = lib.escapeShellArgs writableAppDirectories;
  serviceEnvironment = {
    XDG_CONFIG_HOME = configRoot;
    XDG_DATA_HOME = dataRoot;
    XDG_STATE_HOME = stateRoot;
  }
  // cfg.environment;
in
{
  options.services.dictator = {
    enable = lib.mkEnableOption "Dictator voice typing daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.default;
      defaultText = "dictator.packages.${pkgs.system}.default";
      description = "Dictator package to use.";
    };

    gui = {
      enable = lib.mkEnableOption "Dictator desktop and tray application";
      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.system}.gui;
        defaultText = "dictator.packages.${pkgs.system}.gui";
        description = "Package providing dictator-gui.";
      };
      autostart = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Start the tray application with the graphical session.";
      };
    };

    displayServer = lib.mkOption {
      type = lib.types.enum [
        "x11"
        "wayland"
        "auto"
      ];
      default = "auto";
      description = "Clipboard/typing backend selection used to set default runtime deps and environment.";
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "INFO";
      description = "Log level passed to the daemon (DEBUG, INFO, WARN, ERROR).";
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "Extra arguments passed to `dictator daemon`.";
    };

    settings = lib.mkOption {
      type = lib.types.nullOr settingsFormat.type;
      default = null;
      description = "Configuration for Dictator written to config.json.";
      example = {
        enable_osd = true;
        notifications = "errors_only";
        api = {
          active_provider = "openai";
          timeout = 60;
          providers = {
            openai = {
              endpoint = "https://api.openai.com/v1/audio/transcriptions";
              key = "sk-...";
              model = "gpt-4o-transcribe";
            };
          };
        };
        audio = {
          sample_rate = 16000;
          channels = 1;
          bit_depth = 16;
          frames_per_block = 1024;
          max_duration_min = 5;
        };
      };
    };

    configFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = "Path to an existing Dictator config.json. If set, settings are ignored.";
    };

    extraPathPackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      description = "Extra packages added to PATH for the Dictator service.";
    };

    extraPackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      description = "Extra packages installed alongside Dictator.";
    };

    environment = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      description = "Environment variables for the Dictator user service.";
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr (lib.types.either lib.types.path lib.types.str);
      default = null;
      description = "Path or systemd EnvironmentFile string for the Dictator user service.";
    };

    passEnvironment = lib.mkOption {
      type = lib.types.nullOr (lib.types.listOf lib.types.str);
      default = null;
      description = "Environment variables to pass through from the user session.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !(cfg.settings != null && cfg.configFile != null);
        message = "services.dictator: set only one of settings or configFile.";
      }
      {
        assertion = configSource != null;
        message = "services.dictator: set settings or configFile when enabling the service.";
      }
      {
        assertion = lib.all (path: lib.hasPrefix "/" path) [
          configRoot
          dataRoot
          stateRoot
        ];
        message = "services.dictator: XDG config, data, and state roots must be absolute paths.";
      }
      {
        assertion =
          configRoot == config.home.homeDirectory || lib.hasPrefix "${config.home.homeDirectory}/" configRoot;
        message = "services.dictator: XDG_CONFIG_HOME must be within home.homeDirectory so Home Manager can manage config.json.";
      }
    ];

    home.packages = [
      cfg.package
    ]
    ++ lib.optional cfg.gui.enable cfg.gui.package
    ++ pathPackages
    ++ cfg.extraPackages;

    home.file."${configRoot}/dictator/config.json" = lib.mkIf (configSource != null) {
      source = configSource;
    };

    xdg.configFile."autostart/dictator.desktop" = lib.mkIf (cfg.gui.enable && cfg.gui.autostart) {
      text = "[Desktop Entry]\nType=Application\nName=Dictator\nExec=${cfg.gui.package}/bin/dictator-gui --tray\nTerminal=false\n";
    };

    systemd.user.services.dictator = {
      Unit = {
        Description = "Dictator voice typing daemon";
        Documentation = "https://github.com/kabilan108/dictator";
        After = [
          "graphical-session.target"
          "sound.target"
        ];
      };
      Service = {
        ExecStart =
          "${lib.getExe cfg.package} daemon --log-level ${lib.escapeShellArg cfg.logLevel}"
          + lib.optionalString (cfg.extraArgs != [ ]) " ${extraArgs}";
        Restart = "on-failure";
        RestartSec = 5;
        ExecStartPre = "+${lib.getExe' pkgs.coreutils "install"} -d -m 0700 ${prepareWritableAppDirectories}";
        UMask = "0077";
        ProtectHome = "read-only";
        ReadOnlyPaths = [ config.home.homeDirectory ];
        ReadWritePaths = writableAppDirectories;
        RuntimeDirectory = "dictator";
        RuntimeDirectoryMode = "0700";
        Environment = [
          "PATH=${lib.makeBinPath pathPackages}"
        ]
        ++ lib.mapAttrsToList (name: value: "${name}=${value}") serviceEnvironment;
        EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;
        PassEnvironment =
          if cfg.passEnvironment != null then cfg.passEnvironment else defaultPassEnvironment;
      };
      Install = {
        WantedBy = [ "default.target" ];
      };
    };
  };
}
