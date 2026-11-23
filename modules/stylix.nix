{ self }:
{
  config,
  lib,
  ...
}:

let
  cfg = config.stylix.targets.nowayprompt;

  mkSubTarget =
    sourceExpr:
    {
      enable = lib.mkEnableOption "`${sourceExpr}` for nowayprompt" // {
        default = true;
        example = false;
      };

      override = lib.mkOption {
        default = null;
        type = lib.types.anything;
        description = ''
          Attribute sets are recursively merged with `${sourceExpr}`,
          while all other non-`null` types override `${sourceExpr}`.
        '';
      };
    };

  applyOverride =
    name: value: override:
    if override == null then
      value
    else if builtins.typeOf override != builtins.typeOf value then
      throw "stylix: expected `config.stylix.targets.nowayprompt.${name}.override` to be a ${builtins.typeOf value}, but got: ${builtins.typeOf override}"
    else if builtins.isAttrs override then
      lib.recursiveUpdate value override
    else
      override;

  clamp =
    lo: hi: x:
    if x < lo then
      lo
    else if x > hi then
      hi
    else
      x;

  # Two-digit uppercase hex alpha byte for 0xRRGGBBAA colours.
  # Padding avoids single-digit hex (e.g. "0" or "D") when opacity < 16/255.
  opacityHex =
    alpha:
    lib.fixedWidthString 2 "0" (lib.toHexString (clamp 0 255 (builtins.ceil (alpha * 255))));

  colorsAvailable = (config.lib.stylix.colors or null) != null;
  opacityAvailable = (config.stylix.opacity or null) != null;
  fontsAvailable = (config.stylix.fonts or null) != null;
in
{
  key = ./stylix.nix;
  _file = ./stylix.nix;

  imports = [ self.homeModules.nowayprompt ];

  meta.maintainers = lib.attrVals (lib.filter (m: lib.maintainers ? ${m}) [
    "nilp0inter"
    "panchoh"
  ]) lib.maintainers;

  options.stylix.targets.nowayprompt = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = config.stylix.autoEnable or true;
      defaultText = lib.literalExpression "config.stylix.autoEnable";
      example = false;
      description = "Whether to enable theming for nowayprompt.";
    };

    colors = mkSubTarget "config.lib.stylix.colors";
    opacity = mkSubTarget "config.stylix.opacity";
    fonts = mkSubTarget "config.stylix.fonts";
  };

  config = lib.mkIf ((config.stylix.enable or false) && cfg.enable) (
    lib.mkMerge [
      (lib.mkIf (cfg.colors.enable && colorsAvailable) (
        let
          colors = applyOverride "colors" config.lib.stylix.colors cfg.colors.override;
          useOpacity = cfg.opacity.enable && opacityAvailable;
          opacity = applyOverride "opacity" config.stylix.opacity cfg.opacity.override;
          background =
            if useOpacity then "${colors.base00}${opacityHex opacity.popups}" else colors.base00;
          green = colors.green or colors.base0B;
          yellow = colors.yellow or colors.base0A;
          red = colors.red or colors.base08;
        in
        {
          programs.nowayprompt.settings.colours = lib.mapAttrs (_: lib.mkDefault) {
            inherit background;
            border = colors.base0D;
            text = colors.base05;
            error-text = colors.base08;

            pin-background = colors.base01;
            pin-border = colors.base05;
            pin-square = colors.base05;

            ok-button = green;
            ok-button-border = green;
            ok-button-text = colors.base00;

            not-ok-button = yellow;
            not-ok-button-border = yellow;
            not-ok-button-text = colors.base00;

            cancel-button = red;
            cancel-button-border = red;
            cancel-button-text = colors.base00;
          };
        }
      ))

      (lib.mkIf (cfg.fonts.enable && fontsAvailable) (
        let
          fonts = applyOverride "fonts" config.stylix.fonts cfg.fonts.override;
          # Stylix popup font sizes are in points (default 10pt); nowayprompt
          # interprets `size=` as logical pixels (defaults 14px / 20px).
          # Multiply before dividing so exact ratios avoid float drift.
          regularSize = builtins.ceil ((fonts.sizes.popups * 4.0) / 3.0);
          largeSize = builtins.ceil ((regularSize * 10.0) / 7.0);
        in
        {
          programs.nowayprompt.settings.general = lib.mapAttrs (_: lib.mkDefault) {
            font-regular = "${fonts.sansSerif.name}:size=${toString regularSize}";
            font-large = "${fonts.sansSerif.name}:size=${toString largeSize}";
          };
        }
      ))
    ]
  );
}
