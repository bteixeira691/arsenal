//! Configuration model, path resolution, and theme resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::ErrorKind;
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
    pub search: SearchCfg,
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
            search: SearchCfg::default(),
            behavior: Behavior::default(),
            theme: ThemeSpec::default(),
            keys: crate::keys::default_map(),
            sources: default_sources(),
        }
    }
}

impl Config {
    /// Installation target for a kind. Prefer OMP's native root so installed
    /// assets are immediately visible to OMP; fall back to the first configured
    /// source.
    pub fn install_source(&self, kind: Kind) -> Option<&Source> {
        let omp = home_dir().join(".omp/agent").join(kind.plural());
        self.sources
            .iter()
            .find(|source| source.kind == kind && source.resolved() == omp)
            .or_else(|| self.sources.iter().find(|source| source.kind == kind))
    }

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
        for mut source in std::mem::take(&mut self.sources) {
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
            if source.label.is_none() {
                source.label =
                    crate::integrations::label_for_path(&source.path).map(str::to_string);
            }
            sources.push(source);
        }
        let known: BTreeSet<(Kind, PathBuf)> = sources
            .iter()
            .map(|source| (source.kind, source.resolved()))
            .collect();
        for source in crate::integrations::existing_sources() {
            if known.contains(&(source.kind, source.resolved())) {
                continue;
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchCfg {
    /// Search document bodies in addition to names, tags, and descriptions.
    pub content: bool,
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

    pub fn display_with_path(&self) -> String {
        match &self.label {
            Some(label) => format!("{label} · {}", self.path),
            None => self.path.clone(),
        }
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
    let mut sources = crate::integrations::sources();
    sources.push(Source::new(Kind::Snippet, "~/.config/arsenal/snippets"));
    sources.push(Source::new(Kind::Snippet, "./.arsenal/snippets"));
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
    let mut warnings = Vec::new();
    match load_overlay(path) {
        Ok(sources) => config.sources.extend(sources),
        Err(error) => warnings.push(format!("extra sources: {error}")),
    }
    match load_key_overlay(path) {
        Ok(keys) => config.keys.extend(keys),
        Err(error) => warnings.push(format!("extra key bindings: {error}")),
    }
    warnings.extend(config.normalize());
    Ok((config, warnings))
}

/// Additional sources managed by `arsenal sources` and the TUI live next to the
/// main configuration so user comments in `config.toml` are never rewritten.
pub fn sources_path(config_path: &Path) -> PathBuf {
    overlay_path(config_path, "sources")
}

pub fn keys_path(config_path: &Path) -> PathBuf {
    overlay_path(config_path, "keys")
}

fn overlay_path(config_path: &Path, suffix: &str) -> PathBuf {
    let stem = config_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "config".into());
    config_path.with_file_name(format!("{stem}.{suffix}.toml"))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct KeyOverlay {
    keys: BTreeMap<String, String>,
}

fn load_key_overlay(config_path: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = keys_path(config_path);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let overlay: KeyOverlay =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(overlay.keys)
}

/// Persist one binding in the managed key overlay. `None` removes the override
/// so the built-in or main-config binding applies again; `Some("")` unbinds.
pub fn set_key(config_path: &Path, action: &str, sequence: Option<&str>) -> Result<(), String> {
    let mut keys = load_key_overlay(config_path)?;
    match sequence {
        Some(sequence) => {
            keys.insert(action.to_string(), sequence.to_string());
        }
        None => {
            keys.remove(action);
        }
    }
    write_key_overlay(config_path, &keys)
}

fn write_key_overlay(config_path: &Path, keys: &BTreeMap<String, String>) -> Result<(), String> {
    let path = keys_path(config_path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let body = toml::to_string_pretty(&KeyOverlay { keys: keys.clone() })
        .map_err(|error| error.to_string())?;
    fs::write(
        &path,
        format!(
            "# Key bindings managed by `arsenal` (the g k editor).\n\
             # Values replace built-in bindings; an empty string disables an action.\n\
             # Delete an entry to fall back to the built-in binding.\n\n{body}"
        ),
    )
    .map_err(|error| format!("{}: {error}", path.display()))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SourceOverlay {
    sources: Vec<Source>,
}

fn load_overlay(config_path: &Path) -> Result<Vec<Source>, String> {
    let path = sources_path(config_path);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let overlay: SourceOverlay =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(overlay.sources)
}

/// Append a source to the managed overlay. Returns `false` when an equivalent
/// source is already present.
pub fn add_source(config_path: &Path, source: &Source) -> Result<bool, String> {
    let mut sources = load_overlay(config_path)?;
    if sources.iter().any(|existing| same_source(existing, source)) {
        return Ok(false);
    }
    if let Ok(text) = fs::read_to_string(config_path)
        && let Ok(main) = toml::from_str::<Config>(&text)
        && main
            .sources
            .iter()
            .any(|existing| same_source(existing, source))
    {
        return Ok(false);
    }
    sources.push(source.clone());
    write_overlay(config_path, &sources)?;
    Ok(true)
}

/// Remove matching sources from the managed overlay. The main configuration is
/// never modified.
pub fn remove_source(config_path: &Path, path: &str, kind: Option<Kind>) -> Result<usize, String> {
    let mut sources = load_overlay(config_path)?;
    let before = sources.len();
    sources.retain(|source| {
        let kind_matches = kind.is_none_or(|kind| source.kind == kind);
        let path_matches = source.path == path || source.resolved() == expand_path(path);
        !(kind_matches && path_matches)
    });
    let removed = before - sources.len();
    if removed == 0 {
        return Err(format!(
            "`{path}` is not managed in {}; edit the main config for sources listed there",
            sources_path(config_path).display()
        ));
    }
    write_overlay(config_path, &sources)?;
    Ok(removed)
}

fn same_source(left: &Source, right: &Source) -> bool {
    if left.kind != right.kind {
        return false;
    }
    if left.path == right.path {
        return true;
    }
    let left_path = fs::canonicalize(left.resolved()).unwrap_or_else(|_| left.resolved());
    let right_path = fs::canonicalize(right.resolved()).unwrap_or_else(|_| right.resolved());
    left_path == right_path
}

fn write_overlay(config_path: &Path, sources: &[Source]) -> Result<(), String> {
    let path = sources_path(config_path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let body = toml::to_string_pretty(&SourceOverlay {
        sources: sources.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    fs::write(
        &path,
        format!(
            "# Extra sources managed by `arsenal sources` and the TUI.\n\
             # The main config is never rewritten; remove entries here or with\n\
             # `arsenal sources remove <path>`.\n\n{body}"
        ),
    )
    .map_err(|error| format!("{}: {error}", path.display()))
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
#       Extra folders added with `arsenal sources add` or the TUI's A key live in
#       config.sources.toml next to this file; this file is never rewritten by them.
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
        assert!(
            config
                .sources
                .iter()
                .all(|source| !source.path.trim().is_empty())
        );
    }

    #[test]
    fn install_source_prefers_omp_native_root() {
        let config = Config {
            sources: vec![
                Source::new(Kind::Skill, "~/.claude/skills"),
                Source::new(Kind::Skill, "~/.omp/agent/skills"),
                Source::new(Kind::Skill, "~/.agents/skills"),
            ],
            ..Config::default()
        };
        let source = config.install_source(Kind::Skill).unwrap();
        assert_eq!(source.path, "~/.omp/agent/skills");
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

    #[test]
    fn extra_sources_are_managed_separately() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let main = "[[sources]]\nkind = \"agent\"\npath = \"/main/agents\"\n";
        fs::write(&config_path, main).unwrap();

        let extra = dir.path().join("extra-skills");
        fs::create_dir_all(&extra).unwrap();
        let source = Source::new(Kind::Skill, extra.to_string_lossy());
        assert!(add_source(&config_path, &source).unwrap());
        assert!(!add_source(&config_path, &source).unwrap());
        assert!(sources_path(&config_path).exists());
        assert_eq!(fs::read_to_string(&config_path).unwrap(), main);

        let (config, warnings) = load(&config_path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            config.sources.iter().any(|candidate| {
                candidate.kind == Kind::Skill && candidate.resolved() == extra
            })
        );

        assert_eq!(
            remove_source(&config_path, &source.path, Some(Kind::Skill)).unwrap(),
            1
        );
        let (config, _) = load(&config_path).unwrap();
        assert!(
            !config
                .sources
                .iter()
                .any(|candidate| candidate.resolved() == extra)
        );
    }

    #[test]
    fn malformed_extra_sources_warn_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[behavior]\nscan_depth = 2\n").unwrap();
        fs::write(sources_path(&config_path), "sources = [\n").unwrap();

        let (config, warnings) = load(&config_path).unwrap();
        assert_eq!(config.behavior.scan_depth, 2);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("extra sources")),
            "{warnings:?}"
        );
    }

    #[test]
    fn key_bindings_are_managed_separately() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let main = "[behavior]\nscan_depth = 2\n";
        fs::write(&config_path, main).unwrap();

        set_key(&config_path, "quit", Some("ctrl-q")).unwrap();
        assert_eq!(fs::read_to_string(&config_path).unwrap(), main);
        assert!(keys_path(&config_path).exists());
        let (config, warnings) = load(&config_path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.keys.get("quit").map(String::as_str), Some("ctrl-q"));

        set_key(&config_path, "quit", None).unwrap();
        let (config, _) = load(&config_path).unwrap();
        assert_eq!(config.keys.get("quit").map(String::as_str), Some("q"));

        set_key(&config_path, "quit", Some("")).unwrap();
        let (config, _) = load(&config_path).unwrap();
        assert_eq!(config.keys.get("quit").map(String::as_str), Some(""));
    }

    #[test]
    fn malformed_key_overlay_warns_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[behavior]\nscan_depth = 2\n").unwrap();
        fs::write(keys_path(&config_path), "[keys\n").unwrap();

        let (config, warnings) = load(&config_path).unwrap();
        assert_eq!(config.behavior.scan_depth, 2);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("extra key bindings")),
            "{warnings:?}"
        );
    }
}
