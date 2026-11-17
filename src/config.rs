//! Configuration model, path resolution, and theme resolution.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

use crate::asset::Kind;

pub const APP_NAME: &str = "arsenal";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub editor: Editor,
    pub layout: LayoutCfg,
    pub behavior: Behavior,
    pub theme: ThemeSpec,
    pub keys: BTreeMap<String, String>,
    pub sources: Vec<Source>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            editor: Editor::default(),
            layout: LayoutCfg::default(),
            behavior: Behavior::default(),
            theme: ThemeSpec::default(),
            keys: crate::keys::default_map(),
            sources: default_sources(),
        }
    }
}

impl Config {
    /// Merge defaults for partial maps and clamp nonsensical values. Returns
    /// human-readable warnings (also used by `arsenal config validate`).
    pub fn normalize(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();

        let mut merged = crate::keys::default_map();
        for (key, value) in std::mem::take(&mut self.keys) {
            merged.insert(key, value);
        }
        self.keys = merged;

        self.layout.list_width = self.layout.list_width.clamp(20, 70);
        self.behavior.scan_depth = self.behavior.scan_depth.clamp(1, 12);

        self.theme.preset = self.theme.preset.trim().to_ascii_lowercase();
        if !["dark", "light", "mono"].contains(&self.theme.preset.as_str()) {
            warnings.push(format!(
                "unknown theme preset `{}`; using `dark`",
                self.theme.preset
            ));
            self.theme.preset = "dark".into();
        }
        for (name, value) in self.theme.color_overrides() {
            if let Some(value) = value
                && parse_color(&value).is_none()
            {
                warnings.push(format!("theme.{name}: cannot parse color `{value}`"));
            }
        }

        let mut sources = Vec::new();
        for source in std::mem::take(&mut self.sources) {
            if source.path.trim().is_empty() {
                warnings.push("ignoring source with an empty path".into());
                continue;
            }
            if let Some(depth) = source.depth
                && !(1..=12).contains(&depth)
            {
                warnings.push(format!(
                    "source `{}`: depth {depth} out of range 1..=12; clamping",
                    source.path
                ));
            }
            sources.push(source);
        }
        self.sources = sources;

        warnings
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).context("serializing configuration")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// Command line, e.g. `"nvim"` or `"code --wait"`. Empty means
    /// `$VISUAL`, then `$EDITOR`, then `vi`.
    pub command: String,
    /// Arguments. `{file}` and `{line}` are substituted. When no `{file}` is
    /// present the path is appended.
    pub args: Vec<String>,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            command: String::new(),
            args: vec!["+{line}".into(), "{file}".into()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutCfg {
    /// Percentage of the main area occupied by the asset list.
    pub list_width: u16,
    pub show_preview: bool,
}

impl Default for LayoutCfg {
    fn default() -> Self {
        LayoutCfg {
            list_width: 38,
            show_preview: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    /// Recursion depth for non-skill sources.
    pub scan_depth: usize,
    /// Ask before deleting.
    pub confirm_delete: bool,
    /// Move deleted files to the trash directory instead of unlinking.
    pub trash: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Behavior {
            scan_depth: 4,
            confirm_delete: true,
            trash: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSpec {
    /// `dark`, `light`, or `mono`.
    pub preset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

impl Default for ThemeSpec {
    fn default() -> Self {
        ThemeSpec {
            preset: "dark".into(),
            accent: None,
            border: None,
            muted: None,
            success: None,
            warning: None,
            error: None,
            skill: None,
            agent: None,
            command: None,
            rule: None,
            snippet: None,
        }
    }
}

impl ThemeSpec {
    fn color_overrides(&self) -> [(&'static str, Option<String>); 11] {
        [
            ("accent", self.accent.clone()),
            ("border", self.border.clone()),
            ("muted", self.muted.clone()),
            ("success", self.success.clone()),
            ("warning", self.warning.clone()),
            ("error", self.error.clone()),
            ("skill", self.skill.clone()),
            ("agent", self.agent.clone()),
            ("command", self.command.clone()),
            ("rule", self.rule.clone()),
            ("snippet", self.snippet.clone()),
        ]
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTheme {
    pub accent: Color,
    pub border: Color,
    pub muted: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub kinds: [Color; 5],
    pub selection: Style,
    pub chip_fg: Color,
    pub chip_bg: Color,
}

impl ResolvedTheme {
    pub fn from_spec(spec: &ThemeSpec) -> Self {
        let mono = spec.preset.eq_ignore_ascii_case("mono");
        let light = spec.preset.eq_ignore_ascii_case("light");
        let mut theme = if mono {
            ResolvedTheme {
                accent: Color::Reset,
                border: Color::Reset,
                muted: Color::Reset,
                success: Color::Reset,
                warning: Color::Reset,
                error: Color::Reset,
                kinds: [Color::Reset; 5],
                selection: Style::new()
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD),
                chip_fg: Color::Reset,
                chip_bg: Color::Reset,
            }
        } else if light {
            ResolvedTheme {
                accent: Color::Blue,
                border: Color::DarkGray,
                muted: Color::DarkGray,
                success: Color::Green,
                warning: Color::Yellow,
                error: Color::Red,
                kinds: [
                    Color::Blue,
                    Color::Magenta,
                    Color::Green,
                    Color::Red,
                    Color::Cyan,
                ],
                selection: Style::new()
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD),
                chip_fg: Color::Black,
                chip_bg: Color::Blue,
            }
        } else {
            ResolvedTheme {
                accent: Color::Cyan,
                border: Color::DarkGray,
                muted: Color::Gray,
                success: Color::Green,
                warning: Color::Yellow,
                error: Color::Red,
                kinds: [
                    Color::Cyan,
                    Color::Magenta,
                    Color::Green,
                    Color::Yellow,
                    Color::Blue,
                ],
                selection: Style::new()
                    .add_modifier(Modifier::REVERSED)
                    .add_modifier(Modifier::BOLD),
                chip_fg: Color::Black,
                chip_bg: Color::Cyan,
            }
        };

        if let Some(c) = spec.accent.as_deref().and_then(parse_color) {
            theme.accent = c;
            if !mono {
                theme.chip_bg = c;
            }
        }
        if let Some(c) = spec.border.as_deref().and_then(parse_color) {
            theme.border = c;
        }
        if let Some(c) = spec.muted.as_deref().and_then(parse_color) {
            theme.muted = c;
        }
        if let Some(c) = spec.success.as_deref().and_then(parse_color) {
            theme.success = c;
        }
        if let Some(c) = spec.warning.as_deref().and_then(parse_color) {
            theme.warning = c;
        }
        if let Some(c) = spec.error.as_deref().and_then(parse_color) {
            theme.error = c;
        }
        for (index, value) in [
            spec.skill.as_deref(),
            spec.agent.as_deref(),
            spec.command.as_deref(),
            spec.rule.as_deref(),
            spec.snippet.as_deref(),
        ]
        .iter()
        .enumerate()
        {
            if let Some(c) = value.and_then(parse_color) {
                theme.kinds[index] = c;
            }
        }
        theme
    }

    pub fn kind_color(&self, kind: Kind) -> Color {
        self.kinds[kind.index()]
    }

    pub fn status_color(&self, level: Level) -> Color {
        match level {
            Level::Info => self.accent,
            Level::Success => self.success,
            Level::Warning => self.warning,
            Level::Error => self.error,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

/// Parse terminal color names and `#rrggbb`.
pub fn parse_color(input: &str) -> Option<Color> {
    let s = input.trim().to_ascii_lowercase();
    let named = match s.as_str() {
        "reset" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "white" => Color::White,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => {
            let hex = s.strip_prefix('#')?;
            if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
    };
    Some(named)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Source {
    pub kind: Kind,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
}

impl Source {
    pub fn new(kind: Kind, path: impl Into<String>) -> Self {
        Source {
            kind,
            path: path.into(),
            label: None,
            depth: None,
        }
    }

    pub fn display(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.path.clone())
    }

    pub fn resolved(&self) -> PathBuf {
        expand_path(&self.path)
    }

    /// Parse a `--root kind=path` argument.
    pub fn parse_root(spec: &str) -> Result<Source> {
        let (kind, path) = spec
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("expected KIND=PATH, got `{spec}`"))?;
        let kind = Kind::parse_loose(kind).ok_or_else(|| {
            anyhow::anyhow!("unknown kind `{kind}` (skill, agent, command, rule, snippet)")
        })?;
        if path.trim().is_empty() {
            bail!("source path is empty in `{spec}`");
        }
        Ok(Source::new(kind, path.trim()))
    }
}

fn default_sources() -> Vec<Source> {
    let mut sources = Vec::new();
    for (kind, dir) in [
        (Kind::Skill, "skills"),
        (Kind::Agent, "agents"),
        (Kind::Command, "commands"),
        (Kind::Rule, "rules"),
    ] {
        sources.push(Source::new(kind, format!("~/.claude/{dir}")));
        sources.push(Source::new(kind, format!("~/.config/omp/{dir}")));
        sources.push(Source::new(kind, format!("./.claude/{dir}")));
        sources.push(Source::new(kind, format!("./.omp/{dir}")));
    }
    sources.push(Source::new(Kind::Snippet, "~/.config/omp/snippets"));
    sources.push(Source::new(Kind::Snippet, "./.omp/snippets"));
    sources
}

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    if let Some(value) = env::var_os(var)
        && !value.is_empty()
    {
        return PathBuf::from(value).join(APP_NAME);
    }
    home_dir().join(fallback).join(APP_NAME)
}

pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

pub fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

pub fn config_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(value) = env::var_os("ARSENAL_CONFIG")
        && !value.is_empty()
    {
        return PathBuf::from(value);
    }
    config_dir().join("config.toml")
}

pub fn load(path: &Path) -> Result<(Config, Vec<String>)> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading configuration {}", path.display()))?;
    let mut config: Config = toml::from_str(&text)
        .with_context(|| format!("parsing configuration {}", path.display()))?;
    let warnings = config.normalize();
    Ok((config, warnings))
}

pub const CONFIG_HEADER: &str = "\
# Arsenal configuration
#
# Colors: named terminal colors (cyan, blue, gray, darkgray, lightred, ...) or #rrggbb.
# Keys: whitespace-separated chord sequences, e.g. \"g g\", \"ctrl-d\", \"?\";
#       use an empty string to disable a binding. Unknown actions are reported on
#       startup and in `arsenal config validate`.
# Sources: each [[sources]] entry points a directory at one asset kind.
#       Skills are directories containing SKILL.md (or flat *.md files);
#       other kinds are *.md files, scanned recursively up to `behavior.scan_depth`.
#
# Built-in presets: dark, light, mono.
#
";

pub fn write_default(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "configuration already exists at {} (use --force to overwrite)",
            path.display()
        );
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = Config::default().to_toml()?;
    fs::write(path, format!("{CONFIG_HEADER}{body}"))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Expand a leading `~` and `$VAR` / `${VAR}` references. Unknown variables are
/// left untouched so users can see the typo in the UI.
pub fn expand_path(input: &str) -> PathBuf {
    let mut text = input.trim().to_string();
    if text == "~" {
        return home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        text = home_dir().join(rest).to_string_lossy().into_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            match env::var(&name) {
                Ok(value) => out.push_str(&value),
                Err(_) => {
                    out.push_str("${");
                    out.push_str(&name);
                    out.push('}');
                }
            }
        } else {
            let mut name = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_ascii_alphanumeric() || next == '_' {
                    name.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if name.is_empty() {
                out.push('$');
            } else {
                match env::var(&name) {
                    Ok(value) => out.push_str(&value),
                    Err(_) => {
                        out.push('$');
                        out.push_str(&name);
                    }
                }
            }
        }
    }
    PathBuf::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips_through_toml() {
        let config = Config::default();
        let text = config.to_toml().unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.sources.len(), config.sources.len());
        assert_eq!(parsed.keys, config.keys);
        assert_eq!(parsed.theme.preset, "dark");
    }

    #[test]
    fn partial_config_keeps_defaults_and_merges_keys() {
        let text = r#"
            [behavior]
            confirm_delete = false

            [keys]
            quit = "ctrl-q"
        "#;
        let mut config: Config = toml::from_str(text).unwrap();
        let warnings = config.normalize();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!config.behavior.confirm_delete);
        assert_eq!(config.keys.get("quit").map(String::as_str), Some("ctrl-q"));
        assert_eq!(config.keys.get("search").map(String::as_str), Some("/"));
        assert_eq!(config.sources.len(), Config::default().sources.len());
    }

    #[test]
    fn invalid_values_warn_and_fall_back() {
        let text = r#"
            [theme]
            preset = "neon"
            accent = "not-a-color"

            [[sources]]
            kind = "skill"
            path = "   "
        "#;
        let mut config: Config = toml::from_str(text).unwrap();
        let warnings = config.normalize();
        assert_eq!(config.theme.preset, "dark");
        assert!(warnings.iter().any(|w| w.contains("neon")));
        assert!(warnings.iter().any(|w| w.contains("not-a-color")));
        assert!(warnings.iter().any(|w| w.contains("empty path")));
        assert!(config.sources.is_empty());
    }

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("#FF8000"), Some(Color::Rgb(255, 128, 0)));
        assert_eq!(parse_color("nope"), None);
        assert_eq!(parse_color("#ff80"), None);
    }

    #[test]
    fn path_expansion() {
        unsafe {
            env::set_var("ARSENAL_TEST_VAR", "/tmp/arsenal-test");
        }
        let path = expand_path("$ARSENAL_TEST_VAR/rules");
        assert_eq!(path, PathBuf::from("/tmp/arsenal-test/rules"));
        let path = expand_path("${ARSENAL_TEST_VAR}/rules");
        assert_eq!(path, PathBuf::from("/tmp/arsenal-test/rules"));
        let path = expand_path("~/rules");
        assert_eq!(path, home_dir().join("rules"));
        assert_eq!(
            expand_path("$UNSET_VAR_X/rules"),
            PathBuf::from("$UNSET_VAR_X/rules")
        );
    }

    #[test]
    fn theme_kind_colors_and_overrides() {
        let spec = ThemeSpec {
            preset: "dark".into(),
            accent: Some("#336699".into()),
            skill: Some("magenta".into()),
            ..ThemeSpec::default()
        };
        let theme = ResolvedTheme::from_spec(&spec);
        assert_eq!(theme.accent, Color::Rgb(0x33, 0x66, 0x99));
        assert_eq!(theme.kind_color(Kind::Skill), Color::Magenta);
        assert_eq!(theme.kind_color(Kind::Agent), Color::Magenta);
    }

    #[test]
    fn parse_root_specs() {
        let source = Source::parse_root("agents=./agents").unwrap();
        assert_eq!(source.kind, Kind::Agent);
        assert_eq!(source.path, "./agents");
        assert!(Source::parse_root("nope=./x").is_err());
        assert!(Source::parse_root("./x").is_err());
    }
}
