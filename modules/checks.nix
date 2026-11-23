# Flake checks for the Home Manager module, Stylix target module, and overlay.
#
# Evaluates the modules directly via `lib.evalModules` (keeping flake inputs
# free of home-manager and stylix) and validates generated `config.ini` files
# both by exact content assertions and by running the real `nowayprompt`
# binary's `Config::parse()` against them.
{
  self,
  pkgs,
  lib,
}:

let
  targetPkg = self.packages.${pkgs.stdenv.hostPlatform.system}.nowayprompt;

  # Minimal Home Manager harness declaring only the options consumed by our
  # modules (`home.packages`, `xdg.configFile`, `assertions`, `lib`, `meta`).
  hmStub = {
    options = {
      assertions = lib.mkOption {
        type = lib.types.listOf lib.types.attrs;
        default = [ ];
      };
      meta.maintainers = lib.mkOption {
        type = lib.types.listOf lib.types.attrs;
        default = [ ];
      };
      home.packages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [ ];
      };
      xdg.configFile = lib.mkOption {
        type = lib.types.attrsOf (
          lib.types.submodule {
            options.source = lib.mkOption { type = lib.types.path; };
          }
        );
        default = { };
      };
      lib = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = { };
      };
    };
  };

  # Minimal Stylix harness matching the option surface provided by Stylix's
  # Home Manager module (`stylix.enable`, `autoEnable`, `opacity`, `fonts`,
  # `lib.stylix.colors`).
  stylixStub = {
    options.stylix = {
      enable = lib.mkEnableOption "Stylix";
      autoEnable = lib.mkOption {
        type = lib.types.bool;
        default = true;
      };
      opacity.popups = lib.mkOption {
        type = lib.types.float;
        default = 1.0;
      };
      fonts = {
        sansSerif.name = lib.mkOption {
          type = lib.types.str;
          default = "DejaVu Sans";
        };
        sizes.popups = lib.mkOption {
          type = lib.types.either lib.types.ints.unsigned lib.types.float;
          default = 10;
        };
      };
    };

    config.lib.stylix.colors = {
      base00 = "282a36";
      base01 = "343746";
      base05 = "f8f8f2";
      base08 = "ff5555";
      base0A = "f1fa8c";
      base0B = "50fa7b";
      base0D = "6272a4";
      red = "ff5555";
      yellow = "f1fa8c";
      green = "50fa7b";
    };
  };

  evalHm =
    modules:
    (lib.evalModules {
      specialArgs = { inherit pkgs; };
      modules = [ hmStub ] ++ modules;
    }).config;

  # Helper: verify that `nowayprompt` parses an ini file cleanly without
  # emitting any `config.ini` parse error (it exits nonzero only because no
  # Wayland display is set in the Nix build sandbox).
  assertParsesWithNowayprompt =
    name: iniPath:
    ''
      cfg_dir="$(mktemp -d)"
      mkdir -p "$cfg_dir/nowayprompt"
      cp ${iniPath} "$cfg_dir/nowayprompt/config.ini"
      set +e
      err="$(XDG_CONFIG_HOME="$cfg_dir" WAYLAND_DISPLAY= ${targetPkg}/bin/nowayprompt --title test </dev/null 2>&1)"
      rc=$?
      set -e
      if [ "$rc" -eq 0 ]; then
        echo "${name}: unexpected zero exit without Wayland display" >&2
        exit 1
      fi
      if printf '%s' "$err" | grep -q 'config\.ini'; then
        echo "${name}: nowayprompt rejected generated config.ini:" >&2
        echo "$err" >&2
        cat "$cfg_dir/nowayprompt/config.ini" >&2
        exit 1
      fi
      rm -rf "$cfg_dir"
    '';

  # --- Home Manager module cases ---

  hmEmpty = evalHm [
    self.homeModules.default
    { programs.nowayprompt.enable = true; }
  ];

  hmDisabled = evalHm [
    self.homeModules.default
    {
      programs.nowayprompt = {
        enable = false;
        settings.general.border = 4;
      };
    }
  ];

  hmNullPackage = evalHm [
    self.homeModules.default
    {
      programs.nowayprompt = {
        enable = true;
        package = null;
        settings.general.border = 3;
      };
    }
  ];

  hmFull = evalHm [
    self.homeModules.nowayprompt
    {
      programs.nowayprompt = {
        enable = true;
        settings = {
          general = {
            vertical-padding = 12;
            pin-square-amount = 32;
            font-regular = "sans:size=14";
            font-large = "DejaVu Sans:size=20";
          };
          colours = {
            background = "ffffffaa";
            border = "F7CD5D";
            text = "fad6a5";
            error-text = "#e0002b";
            ok-button = "0x282a36";
            cancel-button = "0Xff464780";
          };
        };
      };
    }
  ];

  hmFullExpected = pkgs.writeText "hm-full-expected.ini" ''
    [colours]
    background = 0xFFFFFFAA;
    border = 0xF7CD5D;
    cancel-button = 0xFF464780;
    error-text = 0xE0002B;
    ok-button = 0x282A36;
    text = 0xFAD6A5;

    [general]
    font-large = "DejaVu Sans:size=20";
    font-regular = "sans:size=14";
    pin-square-amount = 32;
    vertical-padding = 12;
  '';

  # --- Stylix module cases ---

  # Both `default` and `stylix` imported together verifies module deduplication.
  stylixFull = evalHm [
    stylixStub
    self.homeModules.default
    self.homeModules.stylix
    {
      stylix = {
        enable = true;
        opacity.popups = 0.8;
      };
      programs.nowayprompt.enable = true;
    }
  ];

  stylixFullExpected = pkgs.writeText "stylix-full-expected.ini" ''
    [colours]
    background = 0x282A36CC;
    border = 0x6272A4;
    cancel-button = 0xFF5555;
    cancel-button-border = 0xFF5555;
    cancel-button-text = 0x282A36;
    error-text = 0xFF5555;
    not-ok-button = 0xF1FA8C;
    not-ok-button-border = 0xF1FA8C;
    not-ok-button-text = 0x282A36;
    ok-button = 0x50FA7B;
    ok-button-border = 0x50FA7B;
    ok-button-text = 0x282A36;
    pin-background = 0x343746;
    pin-border = 0xF8F8F2;
    pin-square = 0xF8F8F2;
    text = 0xF8F8F2;

    [general]
    font-large = "DejaVu Sans:size=20";
    font-regular = "DejaVu Sans:size=14";
  '';

  # Low opacity (< 16/255) must zero-pad to two hex digits.
  stylixLowOpacity = evalHm [
    stylixStub
    self.homeModules.stylix
    {
      stylix = {
        enable = true;
        opacity.popups = 0.05;
      };
      programs.nowayprompt.enable = true;
    }
  ];

  # User settings override individual Stylix keys without needing `mkForce`.
  stylixUserOverride = evalHm [
    stylixStub
    self.homeModules.stylix
    {
      stylix.enable = true;
      programs.nowayprompt = {
        enable = true;
        settings = {
          general.corner-radius = 4;
          colours.border = "0x123456";
        };
      };
    }
  ];

  # Stylix target override hooks (`colors.override`, `opacity.override`, `fonts.override`).
  stylixTargetOverrides = evalHm [
    stylixStub
    self.homeModules.stylix
    {
      stylix = {
        enable = true;
        targets.nowayprompt = {
          colors.override = {
            base0D = "abcdef";
          };
          opacity.override = {
            popups = 0.5;
          };
          fonts.override = {
            sansSerif.name = "Inter";
            sizes.popups = 12;
          };
        };
      };
      programs.nowayprompt.enable = true;
    }
  ];

  # Sub-toggles: disabling opacity keeps opaque colours; disabling fonts omits [general].
  stylixNoOpacityNoFonts = evalHm [
    stylixStub
    self.homeModules.stylix
    {
      stylix = {
        enable = true;
        opacity.popups = 0.5;
        targets.nowayprompt = {
          opacity.enable = false;
          fonts.enable = false;
        };
      };
      programs.nowayprompt.enable = true;
    }
  ];

  # Disabling colours keeps fonts only.
  stylixFontsOnly = evalHm [
    stylixStub
    self.homeModules.stylix
    {
      stylix = {
        enable = true;
        targets.nowayprompt.colors.enable = false;
      };
      programs.nowayprompt.enable = true;
    }
  ];

  # autoEnable = false leaves the target disabled by default.
  stylixAutoEnableFalse = evalHm [
    stylixStub
    self.homeModules.stylix
    {
      stylix = {
        enable = true;
        autoEnable = false;
      };
      programs.nowayprompt.enable = true;
    }
  ];

  # Stylix enabled while programs.nowayprompt.enable = false installs nothing.
  stylixProgramDisabled = evalHm [
    stylixStub
    self.homeModules.stylix
    { stylix.enable = true; }
  ];

  # Importing the Stylix module without Stylix present degrades safely.
  stylixAbsent = evalHm [
    self.homeModules.stylix
    { programs.nowayprompt.enable = true; }
  ];

  overlaidPkg = (pkgs.extend self.overlays.default).nowayprompt;
in
{
  nowayprompt-home-manager-module =
    assert lib.all (a: a.assertion) hmEmpty.assertions;
    assert hmEmpty.home.packages == [ targetPkg ];
    assert !(hmEmpty.xdg.configFile ? "nowayprompt/config.ini");
    assert hmDisabled.home.packages == [ ];
    assert !(hmDisabled.xdg.configFile ? "nowayprompt/config.ini");
    assert hmNullPackage.home.packages == [ ];
    assert hmNullPackage.xdg.configFile ? "nowayprompt/config.ini";
    assert hmFull.home.packages == [ targetPkg ];
    assert overlaidPkg == targetPkg;
    pkgs.runCommand "nowayprompt-home-manager-module" { } ''
      diff -u ${hmFullExpected} ${hmFull.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "hmFull" hmFull.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "hmNullPackage" hmNullPackage.xdg.configFile."nowayprompt/config.ini".source}
      touch "$out"
    '';

  nowayprompt-stylix-module =
    assert lib.all (a: a.assertion) stylixFull.assertions;
    assert stylixFull.home.packages == [ targetPkg ];
    assert !(stylixAutoEnableFalse.xdg.configFile ? "nowayprompt/config.ini");
    assert stylixProgramDisabled.home.packages == [ ];
    assert !(stylixProgramDisabled.xdg.configFile ? "nowayprompt/config.ini");
    assert stylixAbsent.home.packages == [ targetPkg ];
    assert !(stylixAbsent.xdg.configFile ? "nowayprompt/config.ini");
    pkgs.runCommand "nowayprompt-stylix-module" { } ''
      diff -u ${stylixFullExpected} ${stylixFull.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "stylixFull" stylixFull.xdg.configFile."nowayprompt/config.ini".source}

      grep -qx 'background = 0x282A360D;' ${stylixLowOpacity.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "stylixLowOpacity" stylixLowOpacity.xdg.configFile."nowayprompt/config.ini".source}

      grep -qx 'border = 0x123456;' ${stylixUserOverride.xdg.configFile."nowayprompt/config.ini".source}
      grep -qx 'corner-radius = 4;' ${stylixUserOverride.xdg.configFile."nowayprompt/config.ini".source}
      grep -qx 'text = 0xF8F8F2;' ${stylixUserOverride.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "stylixUserOverride" stylixUserOverride.xdg.configFile."nowayprompt/config.ini".source}

      grep -qx 'background = 0x282A3680;' ${stylixTargetOverrides.xdg.configFile."nowayprompt/config.ini".source}
      grep -qx 'border = 0xABCDEF;' ${stylixTargetOverrides.xdg.configFile."nowayprompt/config.ini".source}
      grep -qx 'font-regular = "Inter:size=16";' ${stylixTargetOverrides.xdg.configFile."nowayprompt/config.ini".source}
      grep -qx 'font-large = "Inter:size=23";' ${stylixTargetOverrides.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "stylixTargetOverrides" stylixTargetOverrides.xdg.configFile."nowayprompt/config.ini".source}

      grep -qx 'background = 0x282A36;' ${stylixNoOpacityNoFonts.xdg.configFile."nowayprompt/config.ini".source}
      ! grep -q '^\[general\]' ${stylixNoOpacityNoFonts.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "stylixNoOpacityNoFonts" stylixNoOpacityNoFonts.xdg.configFile."nowayprompt/config.ini".source}

      ! grep -q '^\[colours\]' ${stylixFontsOnly.xdg.configFile."nowayprompt/config.ini".source}
      grep -qx 'font-regular = "DejaVu Sans:size=14";' ${stylixFontsOnly.xdg.configFile."nowayprompt/config.ini".source}
      ${assertParsesWithNowayprompt "stylixFontsOnly" stylixFontsOnly.xdg.configFile."nowayprompt/config.ini".source}

      touch "$out"
    '';
}
