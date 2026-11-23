{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.nowayprompt;

  # Matches 6-hex-digit RGB or 8-hex-digit RGBA values, optionally prefixed
  # with `0x`/`0X` or `#`. Bare 6/8-digit strings match upstream Home Manager
  # and Stylix palettes; accepting `#` prevents nowayprompt's INI parser from
  # treating `#RRGGBB` as an inline comment.
  colourHexPattern = "^(0[xX]|#)?([0-9a-fA-F]{6}([0-9a-fA-F]{2})?)$";

  matchColourHex = str: builtins.match colourHexPattern str;

  iniFormat = pkgs.formats.ini {
    mkKeyValue = lib.generators.mkKeyValueDefault {
      mkValueString =
        v:
        if lib.isString v then
          let
            colourMatch = matchColourHex v;
          in
          if colourMatch != null then
            "0x${lib.strings.toUpper (builtins.elemAt colourMatch 1)};"
          else
            ''"${v}";''
        else
          lib.generators.mkValueStringDefault { } v + ";";
    } " = ";
  };

  platformAssertion =
    if lib ? hm && lib.hm ? assertions && lib.hm.assertions ? assertPlatform then
      lib.hm.assertions.assertPlatform "programs.nowayprompt" pkgs lib.platforms.linux
    else
      {
        assertion = pkgs.stdenv.hostPlatform.isLinux;
        message = "programs.nowayprompt is only supported on Linux";
      };

  nonEmptySettings = lib.filterAttrs (_: section: section != { }) cfg.settings;
in
{
  key = ./home-manager.nix;
  _file = ./home-manager.nix;

  meta.maintainers = lib.attrVals (lib.filter (m: lib.maintainers ? ${m}) [
    "nilp0inter"
    "panchoh"
  ]) lib.maintainers;

  options.programs.nowayprompt = {
    enable = lib.mkEnableOption "nowayprompt, a Wayland prompt tool";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.nowayprompt;
      defaultText = lib.literalExpression "inputs.nowayprompt.packages.\${pkgs.stdenv.hostPlatform.system}.nowayprompt";
      description = ''
        The nowayprompt package to install. Set to `null` to skip installing the
        package while still generating the configuration file.
      '';
    };

    settings = lib.mkOption {
      inherit (iniFormat) type;
      default = { };
      example = {
        general = {
          font-regular = "sans:size=14";
          font-large = "sans:size=20";
          pin-square-amount = 32;
        };
        colours = {
          background = "ffffffaa";
        };
      };
      description = ''
        Configuration for nowayprompt written to
        {file}`$XDG_CONFIG_HOME/nowayprompt/config.ini`.
        See {manpage}`nowayprompt.conf(5)` for a list of available options.
        Colours can be 6-hex-digit RGB or 8-hex-digit RGBA values, with or
        without a leading `0x` or `#`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [ platformAssertion ];

    home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];

    xdg.configFile."nowayprompt/config.ini" = lib.mkIf (nonEmptySettings != { }) {
      source = iniFormat.generate "config.ini" nonEmptySettings;
    };
  };
}
