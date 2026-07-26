//! `wayprompt.5` INI configuration parser.
//!
//! Streaming `std::io::BufRead` line parser with section dispatch
//! (`[general]`, `[colours]`), trailing-semicolon stripping, inline `#`
//! comment stripping, hyphen-to-underscore field normalization, and hex
//! `0xRRGGBB` / `0xRRGGBBAA` to premultiplied-alpha `Colour` conversion.
//!
//! The file format is the `wayprompt.5` INI dialect. Config file resolution
//! is nowayprompt-primary with a silent wayprompt fallback: within the
//! chosen config base, an existing `nowayprompt/config.ini` wins; otherwise
//! `wayprompt/config.ini` is used.

use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// File name shared by both configuration candidates within a base.
const CONFIG_FILE_NAME: &str = "config.ini";

/// Error returned by [`Config::parse`] and the colour/integer parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// I/O error reading the config file.
    Io(String),
    /// Syntax error (malformed line, unknown section, assignment outside a
    /// section, unknown variable, invalid integer, invalid colour).
    BadConfig {
        path: String,
        line: usize,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "config I/O error: {msg}"),
            Self::BadConfig {
                path,
                line,
                message,
            } => {
                write!(f, "{path}:{line}: {message}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Premultiplied-alpha 16-bit RGBA colour in the `pixman` colour layout
/// used for Wayland rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Colour {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub alpha: u16,
}

/// Parse a `0xRRGGBB` (6 hex digits) or `0xRRGGBBAA` (8 hex digits) string into
/// a premultiplied-alpha [`Colour`].
///
/// Conversion invariants:
/// - 6-digit form implies alpha `0xff`.
/// - `channel_16 = (channel_8 as f32 / 255.0 * 65535.0) as u16` (truncation).
/// - `premul_16 = (channel_16 as f32 * alpha_16 as f32 / 0xffff as f32) as u16`.
pub fn parse_colour(hex: &str) -> Result<Colour, ConfigError> {
    // Length validation: "0xRRGGBB" = 8 chars, "0xRRGGBBAA" = 10 chars.
    if hex.len() != 8 && hex.len() != 10 {
        return Err(ConfigError::BadConfig {
            path: String::new(),
            line: 0,
            message: format!("bad colour: '{hex}' (expected 0xRRGGBB or 0xRRGGBBAA)"),
        });
    }
    let bytes = hex.as_bytes();
    if bytes[0] != b'0' || bytes[1] != b'x' {
        return Err(ConfigError::BadConfig {
            path: String::new(),
            line: 0,
            message: format!("bad colour: '{hex}' (missing 0x prefix)"),
        });
    }
    let digits = &hex[2..];
    let parsed = u32::from_str_radix(digits, 16).map_err(|_| ConfigError::BadConfig {
        path: String::new(),
        line: 0,
        message: format!("bad colour: '{hex}' (invalid hex digits)"),
    })?;

    // Pack into a u32. 6-digit: shift left 8 and OR 0xff so alpha defaults
    // to opaque; 8-digit: use as-is. Channels are then extracted by direct
    // shifting (endianness independent).
    let (r, g, b, a) = if hex.len() == 8 {
        // 0xRRGGBB -> r=high byte, g=mid, b=low, a=0xff.
        let r8 = ((parsed >> 16) & 0xff) as u8;
        let g8 = ((parsed >> 8) & 0xff) as u8;
        let b8 = (parsed & 0xff) as u8;
        (r8, g8, b8, 0xff)
    } else {
        // 0xRRGGBBAA -> r=byte[3], g=byte[2], b=byte[1], a=byte[0].
        let r8 = ((parsed >> 24) & 0xff) as u8;
        let g8 = ((parsed >> 16) & 0xff) as u8;
        let b8 = ((parsed >> 8) & 0xff) as u8;
        let a8 = (parsed & 0xff) as u8;
        (r8, g8, b8, a8)
    };

    // Premultiplied alpha math: float-to-int conversion truncates toward
    // zero (Rust `as` cast semantics), rather than rounding.
    let alpha_f = (a as f32 / 255.0) * 65535.0;
    let alpha = alpha_f as u16;
    let red = ((r as f32 / 255.0) * 65535.0 * alpha as f32 / 0xffff as f32) as u16;
    let green = ((g as f32 / 255.0) * 65535.0 * alpha as f32 / 0xffff as f32) as u16;
    let blue = ((b as f32 / 255.0) * 65535.0 * alpha as f32 / 0xffff as f32) as u16;

    Ok(Colour {
        red,
        green,
        blue,
        alpha,
    })
}

/// Compare a Rust field name (using `_`) with a config variable (using
/// `-`): positions where the field has `_` accept `-` in the variable; all
/// other positions must match exactly. `_` in the variable does NOT satisfy
/// `_` in the field.
pub fn field_eq(field: &str, variable: &str) -> bool {
    if field.len() != variable.len() {
        return false;
    }
    let fb = field.as_bytes();
    let vb = variable.as_bytes();
    for (f, v) in fb.iter().zip(vb.iter()) {
        if *f == b'_' {
            if *v != b'-' {
                return false;
            }
        } else if f != v {
            return false;
        }
    }
    true
}

/// Runtime-populated label strings. Not parsed from the config file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Labels {
    pub title: Option<String>,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub err_message: Option<String>,
    pub not_ok: Option<String>,
    pub ok: Option<String>,
    pub cancel: Option<String>,
}

/// Wayland UI dimensions populated from `[general]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandUi {
    pub vertical_padding: u16,
    pub horizontal_padding: u16,
    pub button_inner_padding: u16,
    pub pin_square_size: u16,
    pub pin_square_border: u16,
    pub button_border: u16,
    pub border: u16,
    pub corner_radius: u16,
    pub pin_square_amount: u16,
    pub font_regular: Option<String>,
    pub font_large: Option<String>,
}

impl Default for WaylandUi {
    fn default() -> Self {
        Self {
            vertical_padding: 10,
            horizontal_padding: 15,
            button_inner_padding: 5,
            pin_square_size: 18,
            pin_square_border: 1,
            button_border: 1,
            border: 2,
            corner_radius: 10,
            pin_square_amount: 16,
            font_regular: None,
            font_large: None,
        }
    }
}

impl WaylandUi {
    /// Assign a single `key = value` pair. Returns `Ok(true)` if the field
    /// was recognized and assigned, `Ok(false)` if unknown, `Err` on parse
    /// failure.
    fn assign(
        &mut self,
        path: &str,
        line: usize,
        variable: &str,
        value: &str,
    ) -> Result<bool, ConfigError> {
        let err = |msg: String| ConfigError::BadConfig {
            path: path.to_string(),
            line,
            message: msg,
        };

        // Integer fields: dispatch by name. Using a helper to avoid repetition.
        macro_rules! int_field {
            ($field:ident) => {
                if field_eq(stringify!($field), variable) {
                    self.$field = value
                        .parse::<u16>()
                        .map_err(|_| err(format!("invalid positive integer: '{value}'")))?;
                    return Ok(true);
                }
            };
        }

        int_field!(vertical_padding);
        int_field!(horizontal_padding);
        int_field!(button_inner_padding);
        int_field!(pin_square_size);
        int_field!(pin_square_border);
        int_field!(button_border);
        int_field!(border);
        int_field!(corner_radius);
        int_field!(pin_square_amount);

        // String fields (font paths/descriptions).
        if field_eq("font_regular", variable) {
            self.font_regular = Some(value.to_string());
            return Ok(true);
        }
        if field_eq("font_large", variable) {
            self.font_large = Some(value.to_string());
            return Ok(true);
        }

        Ok(false)
    }
}

/// Wayland colours populated from `[colours]`. Defaults are those
/// documented in `wayprompt.5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaylandColours {
    pub background: Colour,
    pub border: Colour,
    pub text: Colour,
    pub error_text: Colour,
    pub pin_background: Colour,
    pub pin_border: Colour,
    pub pin_square: Colour,
    pub ok_button: Colour,
    pub ok_button_border: Colour,
    pub ok_button_text: Colour,
    pub not_ok_button: Colour,
    pub not_ok_button_border: Colour,
    pub not_ok_button_text: Colour,
    pub cancel_button: Colour,
    pub cancel_button_border: Colour,
    pub cancel_button_text: Colour,
}

impl Default for WaylandColours {
    fn default() -> Self {
        // Defaults documented in `wayprompt.5`.
        Self {
            background: parse_colour("0xffffff").unwrap(),
            border: parse_colour("0x000000").unwrap(),
            text: parse_colour("0x000000").unwrap(),
            error_text: parse_colour("0xe0002b").unwrap(),
            pin_background: parse_colour("0xd0d0d0").unwrap(),
            pin_border: parse_colour("0x000000").unwrap(),
            pin_square: parse_colour("0x808080").unwrap(),
            ok_button: parse_colour("0xd5f200").unwrap(),
            ok_button_border: parse_colour("0x000000").unwrap(),
            ok_button_text: parse_colour("0x000000").unwrap(),
            not_ok_button: parse_colour("0xffe53e").unwrap(),
            not_ok_button_border: parse_colour("0x000000").unwrap(),
            not_ok_button_text: parse_colour("0x000000").unwrap(),
            cancel_button: parse_colour("0xff4647").unwrap(),
            cancel_button_border: parse_colour("0x000000").unwrap(),
            cancel_button_text: parse_colour("0x000000").unwrap(),
        }
    }
}

impl WaylandColours {
    /// Assign a single `key = value` pair. Returns `Ok(true)` if recognised,
    /// `Ok(false)` if unknown, `Err` on bad colour.
    fn assign(
        &mut self,
        path: &str,
        line: usize,
        variable: &str,
        value: &str,
    ) -> Result<bool, ConfigError> {
        let err = |msg: String| ConfigError::BadConfig {
            path: path.to_string(),
            line,
            message: msg,
        };

        macro_rules! colour_field {
            ($field:ident) => {
                if field_eq(stringify!($field), variable) {
                    self.$field = parse_colour(value)
                        .map_err(|e| err(format!("bad colour: '{value}' ({})", e)))?;
                    return Ok(true);
                }
            };
        }

        colour_field!(background);
        colour_field!(border);
        colour_field!(text);
        colour_field!(error_text);
        colour_field!(pin_background);
        colour_field!(pin_border);
        colour_field!(pin_square);
        colour_field!(ok_button);
        colour_field!(ok_button_border);
        colour_field!(ok_button_text);
        colour_field!(not_ok_button);
        colour_field!(not_ok_button_border);
        colour_field!(not_ok_button_text);
        colour_field!(cancel_button);
        colour_field!(cancel_button_border);
        colour_field!(cancel_button_text);

        Ok(false)
    }
}

/// Top-level configuration aggregating all sections plus runtime state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    pub labels: Labels,
    pub wayland_colours: WaylandColours,
    pub wayland_ui: WaylandUi,
    /// General process configuration. Populated at runtime.
    pub allow_tty_fallback: bool,
    /// Explicit TTY name (e.g. provided by gpg-agent). Populated at runtime.
    pub tty_name: Option<String>,
    /// Explicit Wayland display socket. Populated at runtime.
    pub wayland_display: Option<String>,
}

/// Section tracking during parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    General,
    Colours,
}

impl Config {
    /// Resolve the config file path.
    ///
    /// The configuration base is chosen exactly once: `$XDG_CONFIG_HOME` when
    /// set and nonempty, else `$HOME/.config` when `$HOME` is set and
    /// nonempty, else `/etc`. Within that single base the first existing
    /// candidate wins: `nowayprompt/config.ini`, then the compatibility
    /// fallback `wayprompt/config.ini`. Non-selected bases are never
    /// consulted. If neither candidate exists, [`Self::parse`] silently keeps
    /// defaults.
    ///
    /// The `nowayprompt`-first order is an intentional, documented
    /// target-only divergence from `pkgs.wayprompt`, which reads only
    /// `wayprompt/config.ini`.
    pub fn config_path() -> PathBuf {
        Self::config_path_from(
            std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        )
    }

    /// Pure form of [`Self::config_path`] over explicit environment inputs.
    fn config_path_from(xdg_config_home: Option<&str>, home: Option<&str>) -> PathBuf {
        Self::select_config_in(&Self::config_base(xdg_config_home, home))
    }

    /// Choose the single configuration base: nonempty `xdg_config_home`,
    /// else nonempty `home` + `/.config`, else `/etc`.
    fn config_base(xdg_config_home: Option<&str>, home: Option<&str>) -> PathBuf {
        match xdg_config_home.filter(|v| !v.is_empty()) {
            Some(xdg) => PathBuf::from(xdg),
            None => match home.filter(|v| !v.is_empty()) {
                Some(home) => Path::new(home).join(".config"),
                None => PathBuf::from("/etc"),
            },
        }
    }

    /// Within one `base`, select `nowayprompt/config.ini` when that path
    /// exists — regardless of content: empty, malformed, unreadable, or a
    /// directory all win — otherwise silently fall back to
    /// `wayprompt/config.ini`. Existence alone decides; errors parsing the
    /// selected file never fall through to the fallback.
    fn select_config_in(base: &Path) -> PathBuf {
        let primary = base.join("nowayprompt").join(CONFIG_FILE_NAME);
        if primary.exists() {
            primary
        } else {
            base.join("wayprompt").join(CONFIG_FILE_NAME)
        }
    }

    /// Parse the config file chosen by [`Self::config_path`]. If the selected
    /// candidate does not exist, succeeds silently (defaults remain).
    /// Otherwise reads line-by-line and dispatches assignments by section.
    /// Parse and I/O errors from the selected file propagate; the fallback
    /// candidate is never consulted after selection.
    pub fn parse(&mut self) -> Result<(), ConfigError> {
        self.parse_at(&Self::config_path())
    }

    /// Parse the config file at `path`; a missing file silently keeps
    /// defaults. Diagnostics report this exact selected path.
    fn parse_at(&mut self, path: &Path) -> Result<(), ConfigError> {
        if !path.exists() {
            return Ok(());
        }
        let path_str = path.to_string_lossy().into_owned();
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        self.parse_from(reader, &path_str)
    }

    /// Parse from any `BufRead` source. Exposed for testing.
    fn parse_from<R: BufRead>(&mut self, reader: R, path: &str) -> Result<(), ConfigError> {
        let mut section = Section::None;
        let mut line_no: usize = 0;

        for line in reader.lines() {
            let line = line?;
            line_no += 1;
            self.parse_line(&line, path, line_no, &mut section)?;
        }
        Ok(())
    }

    /// Parse a single line, mutating `section` as section headers are seen.
    fn parse_line(
        &mut self,
        raw: &str,
        path: &str,
        line_no: usize,
        section: &mut Section,
    ) -> Result<(), ConfigError> {
        let err = |msg: String| ConfigError::BadConfig {
            path: path.to_string(),
            line: line_no,
            message: msg,
        };

        // Trim leading/trailing whitespace.
        let trimmed = raw.trim();
        // Skip blank lines and full-line comments.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return Ok(());
        }

        // Strip inline `#` comment: anything after an unquoted `#` is a
        // comment. `#` starts a comment anywhere in the line, per the
        // `wayprompt.5` INI dialect.
        let content = match trimmed.find('#') {
            Some(idx) => trimmed[..idx].trim_end(),
            None => trimmed,
        };
        if content.is_empty() {
            return Ok(());
        }

        // Section header: `[name]`.
        if let Some(rest) = content.strip_prefix('[') {
            if let Some(name) = rest.strip_suffix(']') {
                let name = name.trim();
                *section = match name {
                    "general" => Section::General,
                    "colours" => Section::Colours,
                    _ => {
                        return Err(err(format!("unknown section '{name}'")));
                    }
                };
                return Ok(());
            }
            return Err(err("syntax error: malformed section header".to_string()));
        }

        // Assignment: `key = value;` (trailing semicolon optional, stripped).
        let eq_idx = match content.find('=') {
            Some(i) => i,
            None => return Err(err("syntax error: expected 'key = value'".to_string())),
        };
        let key = content[..eq_idx].trim();
        let mut value = content[eq_idx + 1..].trim();
        // Strip trailing semicolon (INI values may terminate with `;`).
        if value.ends_with(';') {
            value = value[..value.len() - 1].trim_end();
        }

        match section {
            Section::None => Err(err("assignments must be part of a section".to_string())),
            Section::General => {
                if self.wayland_ui.assign(path, line_no, key, value)? {
                    Ok(())
                } else {
                    Err(err(format!(
                        "unknown variable in section 'general': '{key}'"
                    )))
                }
            }
            Section::Colours => {
                if self.wayland_colours.assign(path, line_no, key, value)? {
                    Ok(())
                } else {
                    Err(err(format!(
                        "unknown variable in section 'colours': '{key}'"
                    )))
                }
            }
        }
    }

    /// Clear all Assuan-populated label fields to `None`.
    ///
    /// The Assuan `RESET` command clears only runtime label state; file
    /// config (`wayland_ui`, `wayland_colours`) is untouched.
    pub fn reset(&mut self) {
        self.labels.title = None;
        self.labels.description = None;
        self.labels.prompt = None;
        self.labels.err_message = None;
        self.labels.ok = None;
        self.labels.not_ok = None;
        self.labels.cancel = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn field_eq_hyphen_matches_underscore() {
        assert!(field_eq("test_test", "test-test"));
        assert!(!field_eq("test_testA", "test-testB"));
        // Boundary cases.
        assert!(field_eq("pin_square_size", "pin-square-size"));
        assert!(!field_eq("pin_square", "pin_squar"));
        assert!(!field_eq("border", "bordery"));
        // `_` in variable does NOT match `_` in field.
        assert!(!field_eq("a_b", "a_b"));
        assert!(field_eq("a_b", "a-b"));
    }

    #[test]
    fn reset_clears_all_labels() {
        let mut cfg = Config::default();
        cfg.labels.title = Some("T".into());
        cfg.labels.description = Some("D".into());
        cfg.labels.prompt = Some("P".into());
        cfg.labels.err_message = Some("E".into());
        cfg.labels.ok = Some("OK".into());
        cfg.labels.not_ok = Some("NO".into());
        cfg.labels.cancel = Some("X".into());
        cfg.reset();
        assert_eq!(cfg.labels.title, None);
        assert_eq!(cfg.labels.description, None);
        assert_eq!(cfg.labels.prompt, None);
        assert_eq!(cfg.labels.err_message, None);
        assert_eq!(cfg.labels.ok, None);
        assert_eq!(cfg.labels.not_ok, None);
        assert_eq!(cfg.labels.cancel, None);
    }

    #[test]
    fn parse_colour_opaque_white() {
        let c = parse_colour("0xffffff").unwrap();
        assert_eq!(
            c,
            Colour {
                red: 65535,
                green: 65535,
                blue: 65535,
                alpha: 65535
            }
        );
    }

    #[test]
    fn parse_colour_opaque_black() {
        let c = parse_colour("0x000000").unwrap();
        assert_eq!(
            c,
            Colour {
                red: 0,
                green: 0,
                blue: 0,
                alpha: 65535
            }
        );
    }

    #[test]
    fn parse_colour_with_alpha_premultiplied() {
        // 0xff000080: r=0xff, g=0, b=0, a=0x80 (128).
        let c = parse_colour("0xff000080").unwrap();
        // alpha = (128/255 * 65535) truncated = 32896 (0x8080).
        assert_eq!(c.alpha, (128.0_f32 / 255.0 * 65535.0) as u16);
        // red = (255/255 * 65535 * alpha / 65535) truncated = alpha.
        assert_eq!(c.red, c.alpha);
        assert_eq!(c.green, 0);
        assert_eq!(c.blue, 0);
    }

    #[test]
    fn parse_colour_default_error_text() {
        // 0xe0002b: r=0xe0, g=0, b=0x2b, a=0xff.
        let c = parse_colour("0xe0002b").unwrap();
        // alpha = 65535.
        assert_eq!(c.alpha, 65535);
        // red = (224/255 * 65535) truncated.
        let expected_red = (224.0_f32 / 255.0 * 65535.0) as u16;
        assert_eq!(c.red, expected_red);
        let expected_blue = (43.0_f32 / 255.0 * 65535.0) as u16;
        assert_eq!(c.blue, expected_blue);
        assert_eq!(c.green, 0);
    }

    #[test]
    fn parse_colour_invalid() {
        assert!(parse_colour("red").is_err());
        assert!(parse_colour("0xGGG").is_err());
        assert!(parse_colour("0xff").is_err());
        assert!(parse_colour("0xffffffXX").is_err());
        assert!(parse_colour("1xabcdef").is_err());
    }

    #[test]
    fn config_defaults_match_wayprompt5() {
        let cfg = Config::default();
        let default_colours = WaylandColours::default();
        assert_eq!(cfg.wayland_colours, default_colours);
        // Spot-check defaults from wayprompt.5.
        assert_eq!(cfg.wayland_ui.vertical_padding, 10);
        assert_eq!(cfg.wayland_ui.horizontal_padding, 15);
        assert_eq!(cfg.wayland_ui.pin_square_size, 18);
        assert_eq!(cfg.wayland_ui.pin_square_amount, 16);
        assert_eq!(cfg.wayland_ui.corner_radius, 10);
    }

    #[test]
    fn parse_full_config_sample() {
        let input = "\
[general]
# This is a comment
vertical-padding = 10;
horizontal-padding = 15;
button-inner-padding = 5;
pin-square-size = 18;
pin-square-amount = 16;
border = 2;
corner-radius = 10;
font-regular = sans:size=14;

[colours]
background = 0xffffff;
border = 0x000000;
text = 0x000000;
error-text = 0xe0002b;
pin-background = 0xd0d0d0;
ok-button = 0xd5f200;
cancel-button = 0xff4647;
";
        let mut cfg = Config::default();
        cfg.parse_from(Cursor::new(input), "test.ini").unwrap();

        assert_eq!(cfg.wayland_ui.vertical_padding, 10);
        assert_eq!(cfg.wayland_ui.horizontal_padding, 15);
        assert_eq!(cfg.wayland_ui.button_inner_padding, 5);
        assert_eq!(cfg.wayland_ui.pin_square_size, 18);
        assert_eq!(cfg.wayland_ui.pin_square_amount, 16);
        assert_eq!(cfg.wayland_ui.border, 2);
        assert_eq!(cfg.wayland_ui.corner_radius, 10);
        assert_eq!(cfg.wayland_ui.font_regular.as_deref(), Some("sans:size=14"));

        assert_eq!(
            cfg.wayland_colours.background,
            parse_colour("0xffffff").unwrap()
        );
        assert_eq!(
            cfg.wayland_colours.border,
            parse_colour("0x000000").unwrap()
        );
        assert_eq!(cfg.wayland_colours.text, parse_colour("0x000000").unwrap());
        assert_eq!(
            cfg.wayland_colours.error_text,
            parse_colour("0xe0002b").unwrap()
        );
        assert_eq!(
            cfg.wayland_colours.pin_background,
            parse_colour("0xd0d0d0").unwrap()
        );
        assert_eq!(
            cfg.wayland_colours.ok_button,
            parse_colour("0xd5f200").unwrap()
        );
        assert_eq!(
            cfg.wayland_colours.cancel_button,
            parse_colour("0xff4647").unwrap()
        );
    }

    #[test]
    fn parse_inline_comment_and_no_semicolon() {
        let input = "\
[general]
border = 2  # thickness
corner-radius = 8
";
        let mut cfg = Config::default();
        cfg.parse_from(Cursor::new(input), "test.ini").unwrap();
        assert_eq!(cfg.wayland_ui.border, 2);
        assert_eq!(cfg.wayland_ui.corner_radius, 8);
    }

    #[test]
    fn parse_unknown_section_errors() {
        let input = "[unknown]\nfoo = 1;\n";
        let mut cfg = Config::default();
        let err = cfg.parse_from(Cursor::new(input), "test.ini").unwrap_err();
        match err {
            ConfigError::BadConfig { line, message, .. } => {
                assert_eq!(line, 1);
                assert!(message.contains("unknown section"), "message: {message}");
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_assignment_outside_section_errors() {
        let input = "foo = 1;\n";
        let mut cfg = Config::default();
        let err = cfg.parse_from(Cursor::new(input), "test.ini").unwrap_err();
        match err {
            ConfigError::BadConfig { line, message, .. } => {
                assert_eq!(line, 1);
                assert!(message.contains("assignments must be part of a section"));
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_variable_in_general_errors() {
        let input = "[general]\nnonexistent = 1;\n";
        let mut cfg = Config::default();
        let err = cfg.parse_from(Cursor::new(input), "test.ini").unwrap_err();
        match err {
            ConfigError::BadConfig { line, message, .. } => {
                assert_eq!(line, 2);
                assert!(message.contains("unknown variable"));
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_invalid_integer_errors() {
        let input = "[general]\nborder = wide;\n";
        let mut cfg = Config::default();
        let err = cfg.parse_from(Cursor::new(input), "test.ini").unwrap_err();
        match err {
            ConfigError::BadConfig { line, message, .. } => {
                assert_eq!(line, 2);
                assert!(message.contains("invalid positive integer"));
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_invalid_colour_errors() {
        let input = "[colours]\nbackground = red;\n";
        let mut cfg = Config::default();
        let err = cfg.parse_from(Cursor::new(input), "test.ini").unwrap_err();
        match err {
            ConfigError::BadConfig { line, message, .. } => {
                assert_eq!(line, 2);
                assert!(message.contains("bad colour"));
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_unknown_variable_in_colours_errors() {
        let input = "[colours]\nunknown_colour = 0x000000;\n";
        let mut cfg = Config::default();
        let err = cfg.parse_from(Cursor::new(input), "test.ini").unwrap_err();
        match err {
            ConfigError::BadConfig { line, message, .. } => {
                assert_eq!(line, 2);
                assert!(message.contains("unknown variable"));
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_input_keeps_defaults() {
        let mut cfg = Config::default();
        cfg.parse_from(Cursor::new(""), "test.ini").unwrap();
        assert_eq!(cfg.wayland_ui, WaylandUi::default());
        assert_eq!(cfg.wayland_colours, WaylandColours::default());
    }

    #[test]
    fn parse_blank_and_comment_lines_skipped() {
        let input = "\n  \n# comment\n   # spaced comment\n";
        let mut cfg = Config::default();
        cfg.parse_from(Cursor::new(input), "test.ini").unwrap();
    }

    // --- Config path selection (nowayprompt-primary, wayprompt fallback) ---

    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// Unique scratch base directory; avoids process-global env mutation and
    /// is parallel-test safe.
    fn temp_base() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nowayprompt-config-test-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_candidate(base: &Path, app: &str, contents: &str) -> PathBuf {
        let dir = base.join(app);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.ini");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn base_selection_precedence() {
        assert_eq!(
            Config::config_base(Some("/xdg"), Some("/home")),
            PathBuf::from("/xdg")
        );
        assert_eq!(
            Config::config_base(Some(""), Some("/home")),
            PathBuf::from("/home/.config")
        );
        assert_eq!(
            Config::config_base(None, Some("/home")),
            PathBuf::from("/home/.config")
        );
        assert_eq!(
            Config::config_base(Some("/xdg"), None),
            PathBuf::from("/xdg")
        );
        assert_eq!(
            Config::config_base(Some(""), Some("")),
            PathBuf::from("/etc")
        );
        assert_eq!(Config::config_base(None, None), PathBuf::from("/etc"));
    }

    #[test]
    fn candidates_never_cross_bases() {
        let xdg = temp_base();
        let xdg_str = xdg.to_str().unwrap();
        // XDG base wins outright: nothing from HOME or /etc is constructed.
        assert_eq!(
            Config::config_path_from(Some(xdg_str), Some("/home")),
            xdg.join("wayprompt").join("config.ini")
        );
        // Empty XDG falls to HOME/.config, not /etc.
        assert_eq!(
            Config::config_path_from(Some(""), Some("/home")),
            PathBuf::from("/home/.config/wayprompt/config.ini")
        );
        // No usable base variable: /etc only.
        assert_eq!(
            Config::config_path_from(None, None),
            PathBuf::from("/etc/wayprompt/config.ini")
        );
    }

    #[test]
    fn primary_wins_over_wayprompt_fallback() {
        let base = temp_base();
        let primary = write_candidate(&base, "nowayprompt", "[general]\nborder = 3;\n");
        write_candidate(&base, "wayprompt", "[general]\nborder = 7;\n");
        assert_eq!(Config::select_config_in(&base), primary);

        let mut cfg = Config::default();
        cfg.parse_at(&Config::select_config_in(&base)).unwrap();
        assert_eq!(cfg.wayland_ui.border, 3);
    }

    #[test]
    fn wayprompt_used_when_primary_absent() {
        let base = temp_base();
        let legacy = write_candidate(&base, "wayprompt", "[general]\nborder = 7;\n");
        assert_eq!(Config::select_config_in(&base), legacy);

        let mut cfg = Config::default();
        cfg.parse_at(&Config::select_config_in(&base)).unwrap();
        assert_eq!(cfg.wayland_ui.border, 7);
    }

    #[test]
    fn no_candidate_silently_keeps_defaults() {
        let base = temp_base();
        let selected = Config::select_config_in(&base);
        assert_eq!(selected, base.join("wayprompt").join("config.ini"));
        assert!(!selected.exists());

        let mut cfg = Config::default();
        cfg.parse_at(&selected).unwrap();
        assert_eq!(cfg.wayland_ui, WaylandUi::default());
        assert_eq!(cfg.wayland_colours, WaylandColours::default());
    }

    #[test]
    fn existing_primary_wins_regardless_of_content() {
        // Malformed primary: selected, and its parse error propagates with
        // the selected full path; the valid fallback is never read.
        let base = temp_base();
        let primary = write_candidate(&base, "nowayprompt", "garbage\n");
        write_candidate(&base, "wayprompt", "[general]\nborder = 7;\n");
        assert_eq!(Config::select_config_in(&base), primary);
        let mut cfg = Config::default();
        let err = cfg.parse_at(&Config::select_config_in(&base)).unwrap_err();
        match err {
            ConfigError::BadConfig { path, line, .. } => {
                assert_eq!(path, primary.to_string_lossy().into_owned());
                assert_eq!(line, 1);
            }
            other => panic!("expected BadConfig, got {other:?}"),
        }
        assert_eq!(cfg.wayland_ui, WaylandUi::default());

        // Empty primary: an existing winner. The malformed fallback is never
        // read, so parsing succeeds with defaults.
        let base = temp_base();
        let primary = write_candidate(&base, "nowayprompt", "");
        write_candidate(&base, "wayprompt", "garbage\n");
        assert_eq!(Config::select_config_in(&base), primary);
        let mut cfg = Config::default();
        cfg.parse_at(&Config::select_config_in(&base)).unwrap();
        assert_eq!(cfg.wayland_ui, WaylandUi::default());

        // Directory primary: existence alone selects it; opening it is an
        // I/O error, not a fallback.
        let base = temp_base();
        let primary_dir = base.join("nowayprompt");
        std::fs::create_dir_all(primary_dir.join("config.ini")).unwrap();
        let primary = primary_dir.join("config.ini");
        assert_eq!(Config::select_config_in(&base), primary);
        let mut cfg = Config::default();
        assert!(cfg.parse_at(&Config::select_config_in(&base)).is_err());
    }
}
