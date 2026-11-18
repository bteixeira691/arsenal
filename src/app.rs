//! Application state, modal editors, and the event-to-action layer.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui_textarea::TextArea;

use crate::asset::{self, Asset, CreateError, Kind};
use crate::config::{self, Config, Level, Source};
use crate::install;
use crate::keys::{Action, Chord, Keymap, Resolution};
use crate::search::{self, SortMode};

const STATUS_TTL: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Assets,
    Sources,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Search,
    Command,
    Form,
    Install,
    InstallProgress,
    Edit,
    Source,
    Keymap,
    Confirm,
    Help,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Search => "SEARCH",
            Mode::Command => "COMMAND",
            Mode::Form => "FORM",
            Mode::Install => "INSTALL",
            Mode::InstallProgress => "CLONE",
            Mode::Edit => "EDIT",
            Mode::Source => "SOURCE",
            Mode::Keymap => "KEYS",
            Mode::Confirm => "CONFIRM",
            Mode::Help => "HELP",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn set(&mut self, value: &str) {
        self.value = value.to_string();
        self.cursor = self.value.chars().count();
    }

    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    fn insert(&mut self, ch: char) {
        let byte = char_to_byte(&self.value, self.cursor);
        self.value.insert(byte, ch);
        self.cursor += 1;
    }

    fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert(ch);
        }
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = char_to_byte(&self.value, self.cursor);
        let start = char_to_byte(&self.value, self.cursor - 1);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        let start = char_to_byte(&self.value, self.cursor);
        let end = char_to_byte(&self.value, self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.len();
    }
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(contents.as_bytes())?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    if let Ok(metadata) = fs::metadata(path) {
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

/// Apply editing keys to a text input. Returns whether the value changed.
fn edit_input(input: &mut TextInput, key: &KeyEvent) -> bool {
    let before = input.value.clone();
    match key.code {
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Left => input.left(),
        KeyCode::Right => input.right(),
        KeyCode::Home => input.home(),
        KeyCode::End => input.end(),
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
                && !ch.is_control() =>
        {
            input.insert(ch);
        }
        _ => {}
    }
    input.value != before
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormField {
    Kind,
    Name,
    Description,
    Source,
    Submit,
}

impl FormField {
    fn next(self) -> Self {
        match self {
            FormField::Kind => FormField::Name,
            FormField::Name => FormField::Description,
            FormField::Description => FormField::Source,
            FormField::Source => FormField::Submit,
            FormField::Submit => FormField::Kind,
        }
    }

    fn prev(self) -> Self {
        match self {
            FormField::Kind => FormField::Submit,
            FormField::Name => FormField::Kind,
            FormField::Description => FormField::Name,
            FormField::Source => FormField::Description,
            FormField::Submit => FormField::Source,
        }
    }

    fn is_choice(self) -> bool {
        matches!(self, FormField::Kind | FormField::Source)
    }
}

#[derive(Clone, Debug)]
pub struct Form {
    pub kind: Kind,
    pub name: TextInput,
    pub description: TextInput,
    pub source_options: Vec<usize>,
    pub source_pos: usize,
    pub focus: FormField,
    pub error: Option<String>,
}

impl Form {
    pub fn new(cfg: &Config, kind: Kind) -> Self {
        let mut form = Form {
            kind,
            name: TextInput::default(),
            description: TextInput::default(),
            source_options: Vec::new(),
            source_pos: 0,
            focus: FormField::Name,
            error: None,
        };
        form.rebuild_sources(cfg, None);
        form
    }

    fn rebuild_sources(&mut self, cfg: &Config, preferred: Option<&str>) {
        self.source_options = cfg
            .sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.kind == self.kind)
            .map(|(index, _)| index)
            .collect();
        self.source_pos = preferred
            .and_then(|path| {
                self.source_options
                    .iter()
                    .position(|&index| cfg.sources[index].path == path)
            })
            .unwrap_or(0);
    }

    pub fn source<'a>(&self, cfg: &'a Config) -> Option<&'a Source> {
        self.source_options
            .get(self.source_pos)
            .map(|&index| &cfg.sources[index])
    }

    fn cycle(&mut self, cfg: &Config, delta: i32) {
        match self.focus {
            FormField::Kind => {
                let preferred = self.source(cfg).map(|source| source.path.clone());
                let next = (self.kind.index() as i32 + delta).rem_euclid(Kind::ALL.len() as i32);
                self.kind = Kind::ALL[next as usize];
                self.rebuild_sources(cfg, preferred.as_deref());
            }
            FormField::Source => {
                let len = self.source_options.len();
                if len > 0 {
                    let next = (self.source_pos as i32 + delta).rem_euclid(len as i32);
                    self.source_pos = next as usize;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallField {
    Url,
    Kind,
    Source,
    Name,
    Submit,
}

impl InstallField {
    fn next(self) -> Self {
        match self {
            InstallField::Url => InstallField::Kind,
            InstallField::Kind => InstallField::Source,
            InstallField::Source => InstallField::Name,
            InstallField::Name => InstallField::Submit,
            InstallField::Submit => InstallField::Url,
        }
    }

    fn prev(self) -> Self {
        match self {
            InstallField::Url => InstallField::Submit,
            InstallField::Kind => InstallField::Url,
            InstallField::Source => InstallField::Kind,
            InstallField::Name => InstallField::Source,
            InstallField::Submit => InstallField::Name,
        }
    }

    fn is_choice(self) -> bool {
        matches!(self, InstallField::Kind | InstallField::Source)
    }
}

#[derive(Clone, Debug)]
pub struct InstallForm {
    pub url: TextInput,
    pub kind: Kind,
    pub source_options: Vec<usize>,
    pub source_pos: usize,
    pub name: TextInput,
    pub focus: InstallField,
    pub error: Option<String>,
}

impl InstallForm {
    pub fn new(cfg: &Config, kind: Kind) -> Self {
        let mut form = InstallForm {
            url: TextInput::default(),
            kind,
            source_options: Vec::new(),
            source_pos: 0,
            name: TextInput::default(),
            focus: InstallField::Url,
            error: None,
        };
        form.rebuild_sources(cfg, None);
        form
    }

    fn rebuild_sources(&mut self, cfg: &Config, preferred: Option<&str>) {
        self.source_options = cfg
            .sources
            .iter()
            .enumerate()
            .filter(|(_, source)| source.kind == self.kind)
            .map(|(index, _)| index)
            .collect();
        self.source_pos = preferred
            .and_then(|path| {
                self.source_options
                    .iter()
                    .position(|&index| cfg.sources[index].path == path)
            })
            .unwrap_or(0);
    }

    pub fn source<'a>(&self, cfg: &'a Config) -> Option<&'a Source> {
        self.source_options
            .get(self.source_pos)
            .map(|&index| &cfg.sources[index])
    }

    fn cycle(&mut self, cfg: &Config, delta: i32) {
        match self.focus {
            InstallField::Kind => {
                let preferred = self.source(cfg).map(|source| source.path.clone());
                let next = (self.kind.index() as i32 + delta).rem_euclid(Kind::ALL.len() as i32);
                self.kind = Kind::ALL[next as usize];
                self.rebuild_sources(cfg, preferred.as_deref());
            }
            InstallField::Source => {
                let len = self.source_options.len();
                if len > 0 {
                    let next = (self.source_pos as i32 + delta).rem_euclid(len as i32);
                    self.source_pos = next as usize;
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceField {
    Path,
    Kind,
    Submit,
}

impl SourceField {
    fn next(self) -> Self {
        match self {
            SourceField::Path => SourceField::Kind,
            SourceField::Kind => SourceField::Submit,
            SourceField::Submit => SourceField::Path,
        }
    }

    fn prev(self) -> Self {
        match self {
            SourceField::Path => SourceField::Submit,
            SourceField::Kind => SourceField::Path,
            SourceField::Submit => SourceField::Kind,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceForm {
    pub path: TextInput,
    pub kind: Kind,
    pub focus: SourceField,
    pub error: Option<String>,
}

impl SourceForm {
    pub fn new(kind: Kind) -> Self {
        SourceForm {
            path: TextInput::default(),
            kind,
            focus: SourceField::Path,
            error: None,
        }
    }

    fn cycle_kind(&mut self, delta: i32) {
        let next = (self.kind.index() as i32 + delta).rem_euclid(Kind::ALL.len() as i32);
        self.kind = Kind::ALL[next as usize];
    }
}

#[derive(Clone, Debug, Default)]
pub struct KeymapEditor {
    pub selected: usize,
    pub capturing: bool,
    pub error: Option<String>,
}

enum InstallMessage {
    Progress(install::Progress),
    Done(Result<install::Installed, String>),
}

pub struct InstallJob {
    receiver: Receiver<InstallMessage>,
    pub progress: install::Progress,
    pub started: Instant,
    pub spinner: usize,
    pub target: String,
}

impl InstallJob {
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

#[derive(Clone, Debug)]
pub struct DocumentEditor {
    pub path: PathBuf,
    pub label: String,
    pub textarea: TextArea<'static>,
    pub dirty: bool,
}

#[derive(Clone, Debug)]
pub struct SourceRow {
    pub index: usize,
    pub kind: Kind,
    pub path: String,
    pub resolved: PathBuf,
    pub exists: bool,
    pub assets: usize,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Confirm {
    pub path: PathBuf,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub text: String,
    pub level: Level,
    pub at: Instant,
}

#[derive(Clone, Debug)]
pub enum Effect {
    EditAsset { path: PathBuf, line: usize },
    EditConfig,
    Reveal(PathBuf),
    Install(install::Request),
}

pub struct App {
    pub cfg: Config,
    pub config_path: PathBuf,
    pub assets: Vec<Asset>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub view: View,
    pub source_selected: usize,
    pub query: String,
    pub kind_filter: Option<Kind>,
    pub content_search: bool,
    pub sort: SortMode,
    pub mode: Mode,
    pub input: TextInput,
    pub form: Option<Form>,
    pub install: Option<InstallForm>,
    last_install: Option<InstallForm>,
    pub install_job: Option<InstallJob>,
    pub source_form: Option<SourceForm>,
    pub keymap_editor: Option<KeymapEditor>,
    pub editor: Option<DocumentEditor>,
    pub discard_prompt: bool,
    pub confirm: Option<Confirm>,
    pub help_scroll: u16,
    pub preview_scroll: u16,
    pub preview_max_scroll: u16,
    pub show_preview: bool,
    pub theme_preset_override: Option<String>,
    pub keys: Keymap,
    pub pending: Vec<Chord>,
    pending_at: Instant,
    pub status: Option<Status>,
    pub warnings: Vec<String>,
    static_warnings: Vec<String>,
    pub effects: Vec<Effect>,
    pub copy: Option<String>,
    pub quit: bool,
    pub list_height: u16,
    pub list_top: usize,
    pub source_top: usize,
    pub active_sources: usize,
    pub counts: [usize; 5],
    fingerprints: BTreeMap<u64, usize>,
    pub data_dir: PathBuf,
    search_backup: String,
    matcher: SkimMatcherV2,
}

impl App {
    pub fn new(
        cfg: Config,
        config_path: PathBuf,
        mut warnings: Vec<String>,
        query: String,
    ) -> Self {
        let (keys, key_warnings) = Keymap::from_map(&cfg.keys);
        warnings.extend(key_warnings);
        let static_warnings = warnings.clone();
        let show_preview = cfg.layout.show_preview;
        let content_search = cfg.search.content;
        let data_dir = config::data_dir();
        let mut app = App {
            cfg,
            config_path,
            assets: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            view: View::Assets,
            source_selected: 0,
            query: String::new(),
            kind_filter: None,
            content_search,
            sort: SortMode::Kind,
            mode: Mode::Normal,
            input: TextInput::default(),
            form: None,
            install: None,
            last_install: None,
            install_job: None,
            source_form: None,
            keymap_editor: None,
            editor: None,
            discard_prompt: false,
            confirm: None,
            help_scroll: 0,
            preview_scroll: 0,
            preview_max_scroll: 0,
            show_preview,
            theme_preset_override: None,
            keys,
            pending: Vec::new(),
            pending_at: Instant::now(),
            status: None,
            warnings,
            static_warnings,
            effects: Vec::new(),
            copy: None,
            quit: false,
            list_height: 10,
            list_top: 0,
            source_top: 0,
            active_sources: 0,
            counts: [0; 5],
            fingerprints: BTreeMap::new(),
            data_dir,
            search_backup: String::new(),
            matcher: SkimMatcherV2::default(),
        };
        app.reload(false);
        app.query = query;
        app.recompute();
        if !app.warnings.is_empty() {
            app.set_status(
                format!(
                    "{} configuration/scan warning(s) — press ? for details",
                    app.warnings.len()
                ),
                Level::Warning,
            );
        }
        app
    }

    pub fn theme(&self) -> config::ResolvedTheme {
        let mut spec = self.cfg.theme.clone();
        if let Some(preset) = &self.theme_preset_override {
            spec.preset = preset.clone();
        }
        config::ResolvedTheme::from_spec(&spec)
    }

    pub fn current(&self) -> Option<&Asset> {
        self.filtered
            .get(self.selected)
            .and_then(|&index| self.assets.get(index))
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.current().map(|asset| asset.path.clone())
    }

    pub fn duplicate_paths(&self) -> Vec<PathBuf> {
        let Some(current) = self.current() else {
            return Vec::new();
        };
        self.assets
            .iter()
            .filter(|asset| {
                asset.fingerprint == current.fingerprint && !paths_equal(&asset.path, &current.path)
            })
            .map(|asset| asset.path.clone())
            .collect()
    }

    pub fn duplicate_count(&self, asset: &Asset) -> usize {
        self.fingerprints
            .get(&asset.fingerprint)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1)
    }

    pub fn set_status(&mut self, text: impl Into<String>, level: Level) {
        self.status = Some(Status {
            text: text.into(),
            level,
            at: Instant::now(),
        });
    }

    pub fn tick(&mut self) {
        self.poll_install();
        if let Some(status) = &self.status
            && status.at.elapsed() > STATUS_TTL
        {
            self.status = None;
        }
    }

    fn reload(&mut self, remember_selection: bool) {
        let previous = if remember_selection {
            self.current_path()
        } else {
            None
        };
        let report = asset::scan(&self.cfg.sources, &self.cfg.behavior);
        self.assets = report.assets;
        self.fingerprints.clear();
        self.counts = [0; 5];
        for asset in &self.assets {
            *self.fingerprints.entry(asset.fingerprint).or_default() += 1;
            self.counts[asset.kind.index()] += 1;
        }
        self.active_sources = report.active_sources;
        self.warnings = self.static_warnings.clone();
        self.warnings.extend(report.warnings);
        self.recompute();
        if let Some(path) = previous {
            self.select_path(&path);
        }
    }

    fn recompute(&mut self) {
        let previous = self.current_path();
        self.filtered = search::select(
            &self.assets,
            self.kind_filter,
            &self.query,
            self.sort,
            self.content_search,
            &self.matcher,
        );
        if self.filtered.len() <= self.selected {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        if self.current_path() != previous {
            self.preview_scroll = 0;
        }
        self.clamp_source_selection();
        self.ensure_visible();
    }

    pub fn source_rows(&self) -> Vec<SourceRow> {
        let terms: Vec<String> = self
            .query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect();
        let mut rows = Vec::new();
        for (index, source) in self.cfg.sources.iter().enumerate() {
            let display = source.display();
            let resolved = source.resolved();
            let haystack = format!(
                "{} {} {} {}",
                display,
                source.kind.as_str(),
                source.path,
                resolved.display()
            )
            .to_lowercase();
            if !terms.iter().all(|term| haystack.contains(term)) {
                continue;
            }
            rows.push(SourceRow {
                index,
                kind: source.kind,
                path: source.path.clone(),
                resolved,
                exists: source.resolved().is_dir(),
                assets: self
                    .assets
                    .iter()
                    .filter(|asset| asset.source == display)
                    .count(),
                label: source.label.clone(),
            });
        }
        rows
    }

    fn clamp_source_selection(&mut self) {
        let len = self.source_rows().len();
        if len == 0 {
            self.source_selected = 0;
            self.source_top = 0;
        } else if self.source_selected >= len {
            self.source_selected = len - 1;
        }
    }

    pub fn source_count(&self) -> usize {
        self.source_rows().len()
    }

    pub fn selected_source(&self) -> Option<SourceRow> {
        self.source_rows().get(self.source_selected).cloned()
    }

    /// Keep the selected row inside the visible viewport.
    pub fn ensure_visible(&mut self) {
        if self.view == View::Sources {
            let len = self.source_rows().len();
            if len == 0 {
                self.source_top = 0;
                return;
            }
            let height = self.list_height.max(1) as usize;
            if self.source_selected < self.source_top {
                self.source_top = self.source_selected;
            }
            if self.source_selected >= self.source_top + height {
                self.source_top = self.source_selected + 1 - height;
            }
            let max_top = len.saturating_sub(height);
            self.source_top = self.source_top.min(max_top).min(self.source_selected);
            return;
        }
        if self.filtered.is_empty() {
            self.list_top = 0;
            return;
        }
        let height = self.list_height.max(1) as usize;
        if self.selected < self.list_top {
            self.list_top = self.selected;
        }
        if self.selected >= self.list_top + height {
            self.list_top = self.selected + 1 - height;
        }
        let max_top = self.filtered.len().saturating_sub(height);
        self.list_top = self.list_top.min(max_top).min(self.selected);
    }

    fn select_path(&mut self, path: &Path) {
        if let Some(position) = self
            .filtered
            .iter()
            .position(|&index| paths_equal(&self.assets[index].path, path))
        {
            self.selected = position;
            self.preview_scroll = 0;
            self.ensure_visible();
        }
    }

    fn move_selection(&mut self, delta: i32, wrap: bool) {
        if self.view == View::Sources {
            let len = self.source_rows().len();
            if len == 0 {
                self.source_selected = 0;
                return;
            }
            let current = self.source_selected.min(len - 1) as i32;
            let mut next = current + delta;
            next = if wrap {
                next.rem_euclid(len as i32)
            } else {
                next.clamp(0, len as i32 - 1)
            };
            if next as usize != self.source_selected {
                self.source_selected = next as usize;
                self.preview_scroll = 0;
                self.ensure_visible();
            }
            return;
        }
        let len = self.filtered.len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(len - 1) as i32;
        let mut next = current + delta;
        next = if wrap {
            next.rem_euclid(len as i32)
        } else {
            next.clamp(0, len as i32 - 1)
        };
        if next as usize != self.selected {
            self.selected = next as usize;
            self.preview_scroll = 0;
            self.ensure_visible();
        }
    }

    fn half_page(&mut self, delta: i32) {
        let step = (self.list_height / 2).max(1) as i32;
        self.move_selection(delta * step, false);
    }

    fn page(&mut self, delta: i32) {
        let step = self.list_height.saturating_sub(1).max(1) as i32;
        self.move_selection(delta * step, false);
    }

    fn set_content_search(&mut self, enabled: bool) {
        self.content_search = enabled;
        self.recompute();
        self.set_status(
            format!("Content search {}", if enabled { "on" } else { "off" }),
            Level::Info,
        );
    }

    fn set_kind_filter(&mut self, kind: Option<Kind>) {
        self.view = View::Assets;
        self.kind_filter = kind;
        self.selected = 0;
        self.list_top = 0;
        self.recompute();
    }

    fn open_form(&mut self, kind: Kind) {
        self.form = Some(Form::new(&self.cfg, kind));
        self.mode = Mode::Form;
        self.input.clear();
    }

    fn open_install_form(&mut self, kind: Kind) {
        self.install = Some(InstallForm::new(&self.cfg, kind));
        self.mode = Mode::Install;
    }

    fn open_source_form(&mut self, kind: Kind) {
        self.source_form = Some(SourceForm::new(kind));
        self.mode = Mode::Source;
    }

    fn reveal_path(&mut self, path: &Path) {
        self.select_path(path);
        let visible = self
            .current()
            .map(|asset| paths_equal(&asset.path, path))
            .unwrap_or(false);
        if !visible {
            self.query.clear();
            self.kind_filter = None;
            self.recompute();
            self.select_path(path);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c'))
            && self.mode != Mode::Edit
        {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.on_normal(key),
            Mode::Search => self.on_search(key),
            Mode::Command => self.on_command(key),
            Mode::Form => self.on_form(key),
            Mode::Install => self.on_install(key),
            Mode::InstallProgress => {}
            Mode::Edit => self.on_edit(key),
            Mode::Source => self.on_source(key),
            Mode::Keymap => self.on_keymap(key),
            Mode::Confirm => self.on_confirm(key),
            Mode::Help => self.on_help(key),
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        let clean: String = text.chars().filter(|ch| !ch.is_control()).collect();
        match self.mode {
            Mode::Search => {
                self.input.insert_str(&clean);
                self.query = self.input.value.clone();
                self.recompute();
            }
            Mode::Command => self.input.insert_str(&clean),
            Mode::Form => {
                let Some(mut form) = self.form.take() else {
                    return;
                };
                match form.focus {
                    FormField::Name => form.name.insert_str(&clean),
                    FormField::Description => form.description.insert_str(&clean),
                    _ => {}
                }
                self.form = Some(form);
            }
            Mode::Install => {
                let Some(mut form) = self.install.take() else {
                    return;
                };
                match form.focus {
                    InstallField::Url => form.url.insert_str(&clean),
                    InstallField::Name => form.name.insert_str(&clean),
                    _ => {}
                }
                self.install = Some(form);
            }
            Mode::Edit => {
                if let Some(editor) = self.editor.as_mut()
                    && editor.textarea.insert_str(&clean)
                {
                    editor.dirty = true;
                }
            }
            Mode::Source => {
                if let Some(form) = self.source_form.as_mut()
                    && form.focus == SourceField::Path
                {
                    form.path.insert_str(&clean);
                }
            }
            _ => {}
        }
    }

    fn on_normal(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.pending.clear();
            if !self.query.is_empty() {
                self.query.clear();
                self.recompute();
            }
            return;
        }
        // Standard terminal navigation stays available alongside the vim
        // bindings, independent of how the keymap is configured.
        let arrow_action = match key.code {
            KeyCode::Up => Some(Action::Up),
            KeyCode::Down => Some(Action::Down),
            KeyCode::Home => Some(Action::Top),
            KeyCode::End => Some(Action::Bottom),
            KeyCode::PageUp => Some(Action::PageUp),
            KeyCode::PageDown => Some(Action::PageDown),
            _ => None,
        };
        if let Some(action) = arrow_action {
            self.pending.clear();
            self.run_action(action);
            return;
        }
        let Some(chord) = Chord::from_event(key) else {
            return;
        };
        if !self.pending.is_empty() && self.pending_at.elapsed() > Duration::from_secs(1) {
            self.pending.clear();
        }
        self.pending.push(chord);
        self.pending_at = Instant::now();
        match self.keys.resolve(&self.pending) {
            Resolution::Matched(action) => {
                self.pending.clear();
                self.run_action(action);
            }
            Resolution::Prefix => {
                if self.pending.len() > 3 {
                    self.pending.clear();
                }
            }
            Resolution::Unmatched => self.pending.clear(),
        }
    }

    fn on_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.query = self.search_backup.clone();
                self.input.set(&self.query);
                self.recompute();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                self.query = self.input.value.clone();
                self.recompute();
                self.mode = Mode::Normal;
            }
            KeyCode::Up => self.move_selection(-1, false),
            KeyCode::Down => self.move_selection(1, false),
            _ => {
                if edit_input(&mut self.input, &key) {
                    self.query = self.input.value.clone();
                    self.recompute();
                }
            }
        }
    }

    fn on_command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
            }
            KeyCode::Enter => {
                let line = self.input.value.clone();
                self.input.clear();
                self.mode = Mode::Normal;
                self.run_command(&line);
            }
            _ => {
                edit_input(&mut self.input, &key);
            }
        }
    }

    fn on_form(&mut self, key: KeyEvent) {
        let Some(mut form) = self.form.take() else {
            self.mode = Mode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.form = Some(form);
                self.submit_form();
                return;
            }
            KeyCode::Tab => form.focus = form.focus.next(),
            KeyCode::BackTab => form.focus = form.focus.prev(),
            KeyCode::Left | KeyCode::Up if form.focus.is_choice() => form.cycle(&self.cfg, -1),
            KeyCode::Right | KeyCode::Down if form.focus.is_choice() => form.cycle(&self.cfg, 1),
            _ => match form.focus {
                FormField::Name => {
                    edit_input(&mut form.name, &key);
                }
                FormField::Description => {
                    edit_input(&mut form.description, &key);
                }
                _ => {}
            },
        }
        self.form = Some(form);
    }

    fn on_install(&mut self, key: KeyEvent) {
        let Some(mut form) = self.install.take() else {
            self.mode = Mode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.install = Some(form);
                self.submit_install();
                return;
            }
            KeyCode::Tab => form.focus = form.focus.next(),
            KeyCode::BackTab => form.focus = form.focus.prev(),
            KeyCode::Left | KeyCode::Up if form.focus.is_choice() => form.cycle(&self.cfg, -1),
            KeyCode::Right | KeyCode::Down if form.focus.is_choice() => form.cycle(&self.cfg, 1),
            _ => match form.focus {
                InstallField::Url => {
                    edit_input(&mut form.url, &key);
                }
                InstallField::Name => {
                    edit_input(&mut form.name, &key);
                }
                _ => {}
            },
        }
        self.install = Some(form);
    }

    fn submit_install(&mut self) {
        let Some(mut form) = self.install.take() else {
            return;
        };
        form.error = None;
        let url = form.url.value.trim().to_string();
        if url.is_empty() {
            form.error = Some("repository URL is required".into());
            form.focus = InstallField::Url;
            self.install = Some(form);
            return;
        }
        let Some(source) = form.source(&self.cfg).cloned() else {
            form.error = Some(format!(
                "No source configured for {}s. Add a [[sources]] entry with kind = \"{}\".",
                form.kind.plural(),
                form.kind.as_str()
            ));
            form.focus = InstallField::Source;
            self.install = Some(form);
            return;
        };
        let name = form.name.value.trim();
        let request = install::Request {
            url,
            kind: form.kind,
            dest: source.resolved(),
            name: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
            reference: None,
            subdir: None,
            force: false,
            trash_dir: None,
        };
        self.mode = Mode::Normal;
        self.last_install = Some(form);
        self.set_status("Installing from git…", Level::Info);
        self.effects.push(Effect::Install(request));
    }

    pub fn start_install(&mut self, request: install::Request) {
        let target = request.url.clone();
        let (sender, receiver) = mpsc::channel();
        let progress_sender = sender.clone();
        thread::spawn(move || {
            let mut report = move |progress| {
                let _ = progress_sender.send(InstallMessage::Progress(progress));
            };
            let result = install::install_with_progress(&request, &mut report)
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(InstallMessage::Done(result));
        });
        self.install_job = Some(InstallJob {
            receiver,
            progress: install::Progress::start(format!("cloning {target}")),
            started: Instant::now(),
            spinner: 0,
            target,
        });
        self.mode = Mode::InstallProgress;
    }

    fn poll_install(&mut self) {
        let mut finished = None;
        if let Some(job) = self.install_job.as_mut() {
            while let Ok(message) = job.receiver.try_recv() {
                match message {
                    InstallMessage::Progress(progress) => job.progress = progress,
                    InstallMessage::Done(result) => finished = Some(result),
                }
            }
            job.spinner = job.spinner.wrapping_add(1);
        }
        if let Some(result) = finished {
            self.install_job = None;
            self.after_install(result);
        }
    }

    pub fn after_install(&mut self, outcome: Result<install::Installed, String>) {
        match outcome {
            Ok(done) => {
                self.last_install = None;
                self.reload(false);
                self.reveal_path(&done.path);
                self.set_status(
                    format!(
                        "Installed {} `{}` ({} file(s)) → {}",
                        done.kind.as_str(),
                        done.name,
                        done.files,
                        done.path.display()
                    ),
                    Level::Success,
                );
            }
            Err(error) => {
                self.set_status(format!("Install failed: {error}"), Level::Error);
                if let Some(mut form) = self.last_install.take() {
                    form.error = Some(error);
                    form.focus = InstallField::Url;
                    self.install = Some(form);
                    self.mode = Mode::Install;
                }
            }
        }
    }

    fn on_edit(&mut self, key: KeyEvent) {
        if self.discard_prompt {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.close_editor(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.discard_prompt = false;
                }
                _ => {}
            }
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('s') {
            self.save_editor();
            return;
        }
        if key.code == KeyCode::Esc {
            if self.editor.as_ref().is_some_and(|editor| editor.dirty) {
                self.discard_prompt = true;
            } else {
                self.close_editor();
            }
            return;
        }
        let Some(editor) = self.editor.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let modified = if ctrl && key.code == KeyCode::Char('z') {
            editor.textarea.undo()
        } else if ctrl && key.code == KeyCode::Char('r') {
            editor.textarea.redo()
        } else {
            editor.textarea.input(key)
        };
        if modified {
            editor.dirty = true;
        }
    }

    fn open_editor(&mut self) {
        let Some(asset) = self.current() else {
            return;
        };
        let path = asset.path.clone();
        let label = asset.name.clone();
        match fs::read_to_string(&path) {
            Ok(text) => {
                let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
                let mut textarea = TextArea::new(lines);
                textarea.set_tab_length(4);
                textarea.set_hard_tab_indent(false);
                self.editor = Some(DocumentEditor {
                    path,
                    label,
                    textarea,
                    dirty: false,
                });
                self.discard_prompt = false;
                self.mode = Mode::Edit;
            }
            Err(error) => self.set_status(
                format!("Cannot edit {}: {error}", path.display()),
                Level::Error,
            ),
        }
    }

    fn save_editor(&mut self) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let path = editor.path.clone();
        let text = editor.textarea.lines().join("\n");
        match write_atomic(&path, &text) {
            Ok(()) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.dirty = false;
                }
                self.reload(true);
                self.set_status(format!("Saved {}", path.display()), Level::Success);
            }
            Err(error) => self.set_status(format!("Save failed: {error}"), Level::Error),
        }
    }

    fn close_editor(&mut self) {
        self.editor = None;
        self.discard_prompt = false;
        self.mode = Mode::Normal;
    }

    fn on_source(&mut self, key: KeyEvent) {
        let Some(mut form) = self.source_form.take() else {
            self.mode = Mode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.source_form = Some(form);
                self.submit_source();
                return;
            }
            KeyCode::Tab => form.focus = form.focus.next(),
            KeyCode::BackTab => form.focus = form.focus.prev(),
            KeyCode::Left | KeyCode::Up if form.focus == SourceField::Kind => {
                form.cycle_kind(-1);
            }
            KeyCode::Right | KeyCode::Down if form.focus == SourceField::Kind => {
                form.cycle_kind(1);
            }
            _ => {
                if form.focus == SourceField::Path {
                    edit_input(&mut form.path, &key);
                }
            }
        }
        self.source_form = Some(form);
    }

    fn submit_source(&mut self) {
        let Some(mut form) = self.source_form.take() else {
            return;
        };
        form.error = None;
        let path = form.path.value.trim().to_string();
        if path.is_empty() {
            form.error = Some("folder path is required".into());
            form.focus = SourceField::Path;
            self.source_form = Some(form);
            return;
        }
        let source = Source::new(form.kind, &path);
        if !source.resolved().is_dir() {
            form.error = Some(format!("not a directory: {}", source.resolved().display()));
            form.focus = SourceField::Path;
            self.source_form = Some(form);
            return;
        }
        match config::add_source(&self.config_path, &source) {
            Ok(true) => {
                let new_index = self.cfg.sources.len();
                self.cfg.sources.push(source);
                self.reload(false);
                self.view = View::Sources;
                self.source_selected = self
                    .source_rows()
                    .iter()
                    .position(|row| row.index == new_index)
                    .unwrap_or(0);
                self.ensure_visible();
                self.mode = Mode::Normal;
                self.set_status(
                    format!(
                        "Added {} source {} · {} asset(s) total",
                        form.kind.as_str(),
                        path,
                        self.assets.len()
                    ),
                    Level::Success,
                );
            }
            Ok(false) => {
                self.mode = Mode::Normal;
                self.set_status(format!("Source already configured: {path}"), Level::Warning);
            }
            Err(error) => {
                form.error = Some(error);
                form.focus = SourceField::Path;
                self.source_form = Some(form);
            }
        }
    }

    fn on_keymap(&mut self, key: KeyEvent) {
        let Some(mut editor) = self.keymap_editor.take() else {
            self.mode = Mode::Normal;
            return;
        };

        if editor.capturing {
            if key.code == KeyCode::Esc {
                editor.capturing = false;
                editor.error = None;
                self.keymap_editor = Some(editor);
                return;
            }
            let Some(chord) = Chord::from_event(key) else {
                self.keymap_editor = Some(editor);
                return;
            };
            let action = Action::ALL[editor.selected.min(Action::ALL.len() - 1)];
            match self.apply_key_change(action, Some(&chord.to_string())) {
                Ok(()) => {
                    editor.capturing = false;
                    editor.error = None;
                    self.set_status(
                        format!("Bound {} to {}", action.as_str(), chord),
                        Level::Success,
                    );
                }
                Err(error) => {
                    editor.capturing = false;
                    editor.error = Some(error);
                }
            }
            self.keymap_editor = Some(editor);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                editor.selected = editor.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                editor.selected = (editor.selected + 1).min(Action::ALL.len() - 1);
            }
            KeyCode::Home | KeyCode::Char('g') => editor.selected = 0,
            KeyCode::End | KeyCode::Char('G') => editor.selected = Action::ALL.len() - 1,
            KeyCode::Enter => {
                editor.capturing = true;
                editor.error = None;
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let action = Action::ALL[editor.selected.min(Action::ALL.len() - 1)];
                match self.apply_key_change(action, Some("")) {
                    Ok(()) => {
                        editor.error = None;
                        self.set_status(format!("Unbound {}", action.as_str()), Level::Warning);
                    }
                    Err(error) => editor.error = Some(error),
                }
            }
            KeyCode::Char('r') => {
                let action = Action::ALL[editor.selected.min(Action::ALL.len() - 1)];
                match self.apply_key_change(action, None) {
                    Ok(()) => {
                        editor.error = None;
                        self.set_status(
                            format!("Reset {} to its default/config binding", action.as_str()),
                            Level::Info,
                        );
                    }
                    Err(error) => editor.error = Some(error),
                }
            }
            _ => {}
        }
        self.keymap_editor = Some(editor);
    }

    fn apply_key_change(&mut self, action: Action, sequence: Option<&str>) -> Result<(), String> {
        config::set_key(&self.config_path, action.as_str(), sequence)?;
        let (cfg, warnings) =
            config::load(&self.config_path).map_err(|error| format!("{error:#}"))?;
        let (keys, key_warnings) = Keymap::from_map(&cfg.keys);
        self.cfg.keys = cfg.keys;
        self.keys = keys;
        self.pending.clear();
        for warning in warnings.into_iter().chain(key_warnings) {
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning.clone());
                self.static_warnings.push(warning);
            }
        }
        Ok(())
    }

    fn on_confirm(&mut self, key: KeyEvent) {
        let action = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => true,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => false,
            _ => return,
        };
        let Some(confirm) = self.confirm.take() else {
            self.mode = Mode::Normal;
            return;
        };
        self.mode = Mode::Normal;
        if action {
            self.perform_delete(&confirm.path, &confirm.label);
        }
    }

    fn on_help(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.mode = Mode::Normal;
                self.help_scroll = 0;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_scroll = self.help_scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_scroll = self.help_scroll.saturating_sub(1);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help_scroll = self.help_scroll.saturating_add(5);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help_scroll = self.help_scroll.saturating_sub(5);
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.help_scroll = self.help_scroll.saturating_add(10);
            }
            KeyCode::PageUp => {
                self.help_scroll = self.help_scroll.saturating_sub(10);
            }
            KeyCode::Char('g') | KeyCode::Home => self.help_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.help_scroll = u16::MAX,
            _ => {}
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Quit => self.quit = true,
            Action::Help => {
                self.mode = Mode::Help;
                self.help_scroll = 0;
            }
            Action::Command => {
                self.mode = Mode::Command;
                self.input.clear();
            }
            Action::Search => {
                self.mode = Mode::Search;
                self.search_backup = self.query.clone();
                self.input.set(&self.query);
            }
            Action::NextMatch => self.move_selection(1, true),
            Action::PrevMatch => self.move_selection(-1, true),
            Action::Create => {
                let kind = self.kind_filter.unwrap_or(Kind::Skill);
                self.open_form(kind);
            }
            Action::Install => {
                let kind = self.kind_filter.unwrap_or(Kind::Skill);
                self.open_install_form(kind);
            }
            Action::EditInline => {
                if self.view == View::Assets {
                    self.open_editor();
                }
            }
            Action::ShowSources => {
                self.view = View::Sources;
                self.mode = Mode::Normal;
                self.clamp_source_selection();
                self.ensure_visible();
            }
            Action::EditKeymap => {
                self.keymap_editor = Some(KeymapEditor::default());
                self.mode = Mode::Keymap;
            }
            Action::AddSource => {
                let kind = self.kind_filter.unwrap_or(Kind::Skill);
                self.open_source_form(kind);
            }
            Action::Edit => {
                if self.view == View::Assets
                    && let Some(asset) = self.current()
                {
                    self.effects.push(Effect::EditAsset {
                        path: asset.path.clone(),
                        line: 1,
                    });
                }
            }
            Action::Delete => {
                if self.view == View::Assets
                    && let Some(asset) = self.current()
                {
                    let path = asset.path.clone();
                    let label = asset.name.clone();
                    if self.cfg.behavior.confirm_delete {
                        self.confirm = Some(Confirm { path, label });
                        self.mode = Mode::Confirm;
                    } else {
                        self.perform_delete(&path, &label);
                    }
                }
            }
            Action::YankPath => {
                let path = if self.view == View::Sources {
                    self.selected_source().map(|source| source.path)
                } else {
                    self.current_path().map(|path| path.display().to_string())
                };
                if let Some(path) = path {
                    self.copy = Some(path);
                }
            }
            Action::Reveal => {
                let path = if self.view == View::Sources {
                    self.selected_source().map(|source| source.resolved)
                } else {
                    self.current_path()
                };
                if let Some(path) = path {
                    self.effects.push(Effect::Reveal(path));
                }
            }
            Action::Rescan => {
                self.reload(true);
                let level = if self.warnings.is_empty() {
                    Level::Success
                } else {
                    Level::Warning
                };
                self.set_status(
                    format!(
                        "Scanned {} asset(s) from {} source(s){}",
                        self.assets.len(),
                        self.active_sources,
                        if self.warnings.is_empty() {
                            String::new()
                        } else {
                            format!(" · {} warning(s)", self.warnings.len())
                        }
                    ),
                    level,
                );
            }
            Action::Config => self.effects.push(Effect::EditConfig),
            Action::Down => self.move_selection(1, false),
            Action::Up => self.move_selection(-1, false),
            Action::Top => {
                if self.view == View::Sources {
                    self.source_selected = 0;
                } else {
                    self.selected = 0;
                }
                self.preview_scroll = 0;
                self.ensure_visible();
            }
            Action::Bottom => {
                if self.view == View::Sources {
                    self.source_selected = self.source_rows().len().saturating_sub(1);
                } else {
                    self.selected = self.filtered.len().saturating_sub(1);
                }
                self.preview_scroll = 0;
                self.ensure_visible();
            }
            Action::HalfPageDown => self.half_page(1),
            Action::HalfPageUp => self.half_page(-1),
            Action::PageDown => self.page(1),
            Action::PageUp => self.page(-1),
            Action::PreviewDown => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_add(1)
                    .min(self.preview_max_scroll);
            }
            Action::PreviewUp => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            }
            Action::TogglePreview => {
                self.show_preview = !self.show_preview;
                self.set_status(
                    format!(
                        "Preview {}",
                        if self.show_preview { "shown" } else { "hidden" }
                    ),
                    Level::Info,
                );
            }
            Action::CycleTheme => {
                const PRESETS: [&str; 3] = ["dark", "light", "mono"];
                let current = self
                    .theme_preset_override
                    .clone()
                    .unwrap_or_else(|| self.cfg.theme.preset.clone());
                let next = PRESETS
                    .iter()
                    .position(|preset| *preset == current)
                    .map(|index| (index + 1) % PRESETS.len())
                    .unwrap_or(0);
                self.theme_preset_override = Some(PRESETS[next].to_string());
                self.set_status(format!("Theme: {}", PRESETS[next]), Level::Info);
            }
            Action::CycleSort => {
                self.sort = self.sort.next();
                self.recompute();
                self.set_status(format!("Sort: {}", self.sort.label()), Level::Info);
            }
            Action::ToggleContentSearch => self.set_content_search(!self.content_search),
            Action::FilterAll => self.set_kind_filter(None),
            Action::FilterSkill => self.set_kind_filter(Some(Kind::Skill)),
            Action::FilterAgent => self.set_kind_filter(Some(Kind::Agent)),
            Action::FilterCommand => self.set_kind_filter(Some(Kind::Command)),
            Action::FilterRule => self.set_kind_filter(Some(Kind::Rule)),
            Action::FilterSnippet => self.set_kind_filter(Some(Kind::Snippet)),
        }
    }

    fn submit_form(&mut self) {
        let Some(mut form) = self.form.take() else {
            return;
        };
        form.error = None;
        if let Err(message) = asset::validate_name(&form.name.value) {
            form.error = Some(message);
            form.focus = FormField::Name;
            self.form = Some(form);
            return;
        }
        if form.description.value.trim().is_empty() {
            form.error = Some("description is required".into());
            form.focus = FormField::Description;
            self.form = Some(form);
            return;
        }
        let Some(source) = form.source(&self.cfg).cloned() else {
            form.error = Some(format!(
                "No source configured for {}s. Add a [[sources]] entry with kind = \"{}\".",
                form.kind.plural(),
                form.kind.as_str()
            ));
            form.focus = FormField::Source;
            self.form = Some(form);
            return;
        };
        let name = form.name.value.trim().to_string();
        let description = form.description.value.trim().to_string();
        match asset::create_asset(&source, form.kind, &name, &description) {
            Ok(path) => {
                let kind = form.kind;
                self.mode = Mode::Normal;
                self.reload(false);
                self.reveal_path(&path);
                self.set_status(
                    format!("Created {} {}", kind.as_str(), path.display()),
                    Level::Success,
                );
            }
            Err(CreateError::Exists(path)) => {
                form.error = Some(format!("{} already exists", path.display()));
                form.focus = FormField::Name;
                self.form = Some(form);
            }
            Err(error) => {
                form.error = Some(error.to_string());
                self.form = Some(form);
            }
        }
    }

    fn perform_delete(&mut self, path: &Path, label: &str) {
        let result = if self.cfg.behavior.trash {
            asset::trash_path(path, &self.data_dir).map(Some)
        } else {
            fs::remove_file(path)
                .map(|()| None)
                .map_err(|error| error.to_string())
        };
        match result {
            Ok(trashed) => {
                let previous = self.selected;
                self.reload(false);
                self.selected = previous.min(self.filtered.len().saturating_sub(1));
                self.preview_scroll = 0;
                let suffix = trashed
                    .map(|target| format!(" (trash: {})", target.display()))
                    .unwrap_or_default();
                self.set_status(format!("Deleted {label}{suffix}"), Level::Success);
            }
            Err(error) => self.set_status(format!("Delete failed: {error}"), Level::Error),
        }
    }

    pub fn run_command(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let mut parts = line.split_whitespace();
        let command = parts.next().unwrap_or_default();
        let argument = parts.next();
        match command {
            "q" | "q!" | "quit" | "quit!" | "qa" | "qa!" | "exit" => self.quit = true,
            "w" | "write" | "rescan" | "reload" => self.run_action(Action::Rescan),
            "new" | "create" | "add" => {
                let kind = match argument {
                    Some(value) => {
                        match Kind::parse_loose(value) {
                            Some(kind) => kind,
                            None => {
                                self.set_status(
                                format!("Unknown kind `{value}` (skill, agent, command, rule, snippet)"),
                                Level::Error,
                            );
                                return;
                            }
                        }
                    }
                    None => self.kind_filter.unwrap_or(Kind::Skill),
                };
                self.open_form(kind);
            }
            "install" | "clone" => {
                let mut tokens = line.split_whitespace();
                tokens.next();
                let url = tokens.next();
                let kind_arg = tokens.next();
                let kind = match kind_arg {
                    Some(value) => match Kind::parse_loose(value) {
                        Some(kind) => kind,
                        None => {
                            self.set_status(
                                format!(
                                    "Unknown kind `{value}` (skill, agent, command, rule, snippet)"
                                ),
                                Level::Error,
                            );
                            return;
                        }
                    },
                    None => self.kind_filter.unwrap_or(Kind::Skill),
                };
                self.open_install_form(kind);
                if let Some(url) = url
                    && let Some(form) = self.install.as_mut()
                {
                    form.url.set(url);
                    form.focus = InstallField::Kind;
                }
            }
            "edit" | "e" => self.run_action(Action::Edit),
            "inline" | "edit-inline" => self.open_editor(),
            "sources" => self.run_action(Action::ShowSources),
            "keys" | "keymap" | "bindings" => {
                self.keymap_editor = Some(KeymapEditor::default());
                self.mode = Mode::Keymap;
            }
            "source" => {
                let mut tokens = line.split_whitespace();
                tokens.next();
                let path = tokens.next();
                let kind_arg = tokens.next();
                let kind = match kind_arg {
                    Some(value) => match Kind::parse_loose(value) {
                        Some(kind) => kind,
                        None => {
                            self.set_status(
                                format!(
                                    "Unknown kind `{value}` (skill, agent, command, rule, snippet)"
                                ),
                                Level::Error,
                            );
                            return;
                        }
                    },
                    None => self.kind_filter.unwrap_or(Kind::Skill),
                };
                self.open_source_form(kind);
                if let Some(path) = path
                    && let Some(form) = self.source_form.as_mut()
                {
                    form.path.set(path);
                    form.focus = SourceField::Kind;
                }
            }
            "delete" | "del" | "rm" => self.run_action(Action::Delete),
            "yank" | "copy" => self.run_action(Action::YankPath),
            "reveal" | "open" => self.run_action(Action::Reveal),
            "config" | "settings" => self.effects.push(Effect::EditConfig),
            "help" | "h" => self.run_action(Action::Help),
            "sort" | "order" => match argument.and_then(SortMode::parse) {
                Some(mode) => {
                    self.sort = mode;
                    self.recompute();
                    self.set_status(format!("Sort: {}", mode.label()), Level::Info);
                }
                None => self.set_status("Usage: :sort kind|name|modified|path", Level::Error),
            },
            "filter" | "kind" => match argument {
                None => self.set_status(
                    "Usage: :filter all|skill|agent|command|rule|snippet",
                    Level::Error,
                ),
                Some(value) if value.eq_ignore_ascii_case("all") => {
                    self.set_kind_filter(None);
                }
                Some(value) => match Kind::parse_loose(value) {
                    Some(kind) => self.set_kind_filter(Some(kind)),
                    None => self.set_status(format!("Unknown kind `{value}`"), Level::Error),
                },
            },
            "theme" | "colors" => {
                let value = argument.unwrap_or_default().to_ascii_lowercase();
                if ["dark", "light", "mono"].contains(&value.as_str()) {
                    self.theme_preset_override = Some(value.clone());
                    self.set_status(format!("Theme: {value}"), Level::Info);
                } else {
                    self.set_status("Usage: :theme dark|light|mono", Level::Error);
                }
            }
            "content" | "contents" => match argument.unwrap_or("toggle") {
                "on" => self.set_content_search(true),
                "off" => self.set_content_search(false),
                "toggle" => self.set_content_search(!self.content_search),
                _ => self.set_status("Usage: :content on|off|toggle", Level::Error),
            },
            "warnings" => {
                if self.warnings.is_empty() {
                    self.set_status("No warnings", Level::Success);
                } else {
                    let first = self.warnings.first().cloned().unwrap_or_default();
                    self.set_status(
                        format!("{} warning(s): {first}", self.warnings.len()),
                        Level::Warning,
                    );
                }
            }
            other => self.set_status(
                format!("Unknown command `:{other}` — press ? for help"),
                Level::Error,
            ),
        }
    }

    pub fn after_edit(&mut self, config_edited: bool) {
        if config_edited {
            match config::load(&self.config_path) {
                Ok((cfg, warnings)) => {
                    let (keys, key_warnings) = Keymap::from_map(&cfg.keys);
                    self.cfg = cfg;
                    self.keys = keys;
                    self.theme_preset_override = None;
                    self.show_preview = self.cfg.layout.show_preview;
                    self.content_search = self.cfg.search.content;
                    self.static_warnings = warnings;
                    self.static_warnings.extend(key_warnings);
                    self.reload(false);
                    self.set_status("Configuration reloaded", Level::Success);
                }
                Err(error) => {
                    self.set_status(format!("Configuration error: {error:#}"), Level::Error);
                }
            }
        } else {
            self.reload(true);
            self.set_status("Reloaded", Level::Success);
        }
    }

    pub fn take_effect(&mut self) -> Option<Effect> {
        if self.effects.is_empty() {
            None
        } else {
            Some(self.effects.remove(0))
        }
    }

    pub fn take_copy(&mut self) -> Option<String> {
        self.copy.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app_with_sources(root: &Path) -> App {
        let cfg = Config {
            sources: vec![
                Source::new(Kind::Skill, root.join("skills").to_string_lossy()),
                Source::new(Kind::Agent, root.join("agents").to_string_lossy()),
            ],
            ..Config::default()
        };
        App::new(cfg, root.join("config.toml"), Vec::new(), String::new())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn create_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name}\n---\n"),
        )
        .unwrap();
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.on_key(key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn text_input_handles_utf8() {
        let mut input = TextInput::default();
        input.insert('a');
        input.insert('é');
        input.insert('b');
        assert_eq!(input.value, "aéb");
        assert_eq!(input.cursor, 3);
        input.left();
        input.backspace();
        assert_eq!(input.value, "ab");
        assert_eq!(input.cursor, 1);
        input.end();
        input.right();
        assert_eq!(input.cursor, 2);
        input.home();
        input.right();
        assert_eq!(input.cursor, 1);
        input.delete();
        assert_eq!(input.value, "a");
        input.clear();
        assert_eq!(input.value, "");
        assert_eq!(input.cursor, 0);
    }

    #[test]
    fn search_filters_and_escape_restores() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills").join("deploy");
        fs::create_dir_all(&skills).unwrap();
        fs::write(
            skills.join("SKILL.md"),
            "---\nname: deploy\ndescription: Deploy services\n---\n",
        )
        .unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews code\n---\n",
        )
        .unwrap();

        let mut app = app_with_sources(dir.path());
        assert_eq!(app.filtered.len(), 2);

        app.on_key(key(KeyCode::Char('/')));
        assert_eq!(app.mode, Mode::Search);
        type_text(&mut app, "deploy");
        assert_eq!(app.filtered.len(), 1);
        assert_eq!(app.current().unwrap().name, "deploy");
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.filtered.len(), 2);
        assert!(app.query.is_empty());

        app.on_key(key(KeyCode::Char('/')));
        type_text(&mut app, "review");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.query, "review");
        assert_eq!(app.filtered.len(), 1);
        assert_eq!(app.current().unwrap().name, "reviewer");
    }

    #[test]
    fn keymap_sequences_drive_navigation() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        for name in ["alpha", "beta", "gamma"] {
            fs::write(
                agents.join(format!("{name}.md")),
                format!("---\nname: {name}\ndescription: {name}\n---\n"),
            )
            .unwrap();
        }
        let mut app = app_with_sources(dir.path());
        assert_eq!(app.filtered.len(), 3);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, 1);
        app.on_key(key(KeyCode::Char('G')));
        assert_eq!(app.selected, 2);
        app.on_key(key(KeyCode::Char('g')));
        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::Char('2')));
        assert_eq!(app.kind_filter, Some(Kind::Agent));
        assert_eq!(app.filtered.len(), 3);
        app.on_key(key(KeyCode::Char('0')));
        assert_eq!(app.kind_filter, None);
    }

    #[test]
    fn arrow_keys_navigate_alongside_vim_keys() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        for name in ["alpha", "bravo", "charlie", "delta", "echo"] {
            fs::write(
                agents.join(format!("{name}.md")),
                format!("---\nname: {name}\ndescription: {name}\n---\n"),
            )
            .unwrap();
        }
        let mut app = app_with_sources(dir.path());
        app.list_height = 2;
        assert_eq!(app.filtered.len(), 5);

        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected, 2);
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.selected, 1);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.selected, 4);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.selected, 0);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.selected, 1);
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.selected, 0);

        // Vim keys still work on the same state.
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn create_form_writes_asset_and_selects_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        assert!(app.assets.is_empty());

        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.mode, Mode::Form);
        type_text(&mut app, "new-skill");
        app.on_key(key(KeyCode::Tab));
        type_text(&mut app, "Does new things");
        app.on_key(key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.assets.len(), 1);
        assert_eq!(app.current().unwrap().name, "new-skill");
        assert_eq!(app.current().unwrap().kind, Kind::Skill);
        assert!(app.current().unwrap().path.ends_with("new-skill/SKILL.md"));
    }

    #[test]
    fn create_form_requires_description_and_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Char('a')));
        type_text(&mut app, "valid-name");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Form);
        assert_eq!(
            app.form.as_ref().unwrap().error.as_deref(),
            Some("description is required")
        );
    }

    #[test]
    fn commands_change_filters_and_sort() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.run_command("filter agents");
        assert_eq!(app.kind_filter, Some(Kind::Agent));
        app.run_command("filter all");
        assert_eq!(app.kind_filter, None);
        app.run_command("sort name");
        assert_eq!(app.sort, SortMode::Name);
        app.run_command("theme mono");
        assert_eq!(app.theme_preset_override.as_deref(), Some("mono"));
        app.run_command("nope");
        assert!(
            app.status
                .as_ref()
                .unwrap()
                .text
                .contains("Unknown command")
        );
    }

    #[test]
    fn creating_while_filtered_reveals_the_new_asset() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.query = "no-such-asset".into();
        app.recompute();
        assert!(app.filtered.is_empty());

        app.on_key(key(KeyCode::Char('a')));
        type_text(&mut app, "fresh-skill");
        app.on_key(key(KeyCode::Tab));
        type_text(&mut app, "Freshly created");
        app.on_key(key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.query.is_empty());
        assert_eq!(app.current().unwrap().name, "fresh-skill");
    }

    #[test]
    fn install_form_queues_a_git_request() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Char('i')));
        assert_eq!(app.mode, Mode::Install);
        type_text(&mut app, "owner/repo");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        match app.take_effect() {
            Some(Effect::Install(request)) => {
                assert_eq!(request.url, "owner/repo");
                assert_eq!(request.kind, Kind::Skill);
                assert_eq!(request.dest, dir.path().join("skills"));
            }
            other => panic!("expected install effect, got {other:?}"),
        }
    }

    #[test]
    fn install_command_prefills_url_and_kind() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.run_command("install https://github.com/owner/repo/tree/main/skills/x agent");
        assert_eq!(app.mode, Mode::Install);
        let form = app.install.as_ref().unwrap();
        assert_eq!(
            form.url.value,
            "https://github.com/owner/repo/tree/main/skills/x"
        );
        assert_eq!(form.kind, Kind::Agent);
        assert_eq!(form.source(&app.cfg).unwrap().kind, Kind::Agent);
    }

    #[test]
    fn failed_install_reopens_the_form_with_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Char('i')));
        type_text(&mut app, "owner/repo");
        app.on_key(key(KeyCode::Enter));
        assert!(matches!(app.take_effect(), Some(Effect::Install(_))));
        app.after_install(Err("clone failed".into()));
        assert_eq!(app.mode, Mode::Install);
        let form = app.install.as_ref().unwrap();
        assert_eq!(form.url.value, "owner/repo");
        assert_eq!(form.error.as_deref(), Some("clone failed"));
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn inline_editor_saves_changes() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("skills").join("demo");
        fs::create_dir_all(&skill).unwrap();
        let path = skill.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: demo\ndescription: Demo skill\n---\n\n# Demo\n",
        )
        .unwrap();
        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Edit);
        assert!(app.editor.is_some());

        app.on_key(key(KeyCode::Char('X')));
        assert!(app.editor.as_ref().unwrap().dirty);
        app.on_key(ctrl(KeyCode::Char('s')));
        assert!(!app.editor.as_ref().unwrap().dirty);
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with('X'), "{saved:?}");
        assert!(saved.ends_with('\n'));

        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.editor.is_none());
        assert_eq!(app.current().unwrap().name, "demo");
    }

    #[test]
    fn inline_editor_discard_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("skills").join("demo");
        fs::create_dir_all(&skill).unwrap();
        let path = skill.join("SKILL.md");
        let original = "---\nname: demo\ndescription: Demo\n---\n";
        fs::write(&path, original).unwrap();

        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Char('Y')));
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Edit);
        assert!(app.discard_prompt);

        app.on_key(key(KeyCode::Char('n')));
        assert!(!app.discard_prompt);
        assert_eq!(app.mode, Mode::Edit);

        app.on_key(key(KeyCode::Esc));
        assert!(app.discard_prompt);
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.editor.is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn inline_editor_undo_and_redo() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("skills").join("demo");
        fs::create_dir_all(&skill).unwrap();
        let path = skill.join("SKILL.md");
        let original = "---\nname: demo\ndescription: Demo\n---\n";
        fs::write(&path, original).unwrap();

        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Char('X')));
        app.on_key(ctrl(KeyCode::Char('z')));
        app.on_key(ctrl(KeyCode::Char('s')));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        app.on_key(ctrl(KeyCode::Char('r')));
        app.on_key(ctrl(KeyCode::Char('s')));
        assert!(fs::read_to_string(&path).unwrap().starts_with('X'));
    }

    #[test]
    fn content_search_and_duplicate_detection() {
        let dir = tempfile::tempdir().unwrap();
        let skill = dir.path().join("skills").join("demo");
        fs::create_dir_all(&skill).unwrap();
        let content =
            "---\nname: demo\ndescription: Demo asset\n---\n\nRun the migration checklist.\n";
        fs::write(skill.join("SKILL.md"), content).unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("demo.md"), content).unwrap();

        let mut app = app_with_sources(dir.path());
        assert_eq!(app.assets.len(), 2);
        assert_eq!(app.duplicate_paths().len(), 1);
        let current = app.current().unwrap().clone();
        assert_eq!(app.duplicate_count(&current), 1);

        app.query = "migration".into();
        app.recompute();
        assert!(app.filtered.is_empty());

        app.on_key(key(KeyCode::Char('F')));
        assert!(app.content_search);
        assert_eq!(app.filtered.len(), 2);
        app.on_key(key(KeyCode::Char('F')));
        assert!(!app.content_search);
        assert!(app.filtered.is_empty());
    }

    #[test]
    fn add_source_modal_persists_and_rescans() {
        let dir = tempfile::tempdir().unwrap();
        let extra = dir.path().join("extra-skills");
        create_skill(&extra.join("fresh"), "fresh");
        let mut app = app_with_sources(dir.path());

        app.on_key(key(KeyCode::Char('A')));
        assert_eq!(app.mode, Mode::Source);
        type_text(&mut app, &extra.to_string_lossy());
        app.on_key(key(KeyCode::Enter));

        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.cfg
                .sources
                .iter()
                .any(|source| source.resolved() == extra && source.kind == Kind::Skill)
        );
        assert!(app.assets.iter().any(|asset| asset.name == "fresh"));
        assert!(config::sources_path(&app.config_path).exists());
    }

    #[test]
    fn add_source_modal_rejects_missing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Char('A')));
        type_text(&mut app, "/definitely/not/here");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Source);
        assert!(
            app.source_form
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("not a directory")
        );
    }

    #[test]
    fn sources_tab_lists_configured_sources() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_sources(dir.path());
        app.on_key(key(KeyCode::Char('6')));
        assert_eq!(app.view, View::Sources);
        assert_eq!(app.mode, Mode::Normal);
        let rows = app.source_rows();
        assert!(
            rows.iter()
                .any(|row| row.kind == Kind::Skill && row.path.ends_with("skills"))
        );
        assert_eq!(app.source_count(), rows.len());

        app.query = "agent".into();
        app.recompute();
        assert!(app.source_rows().iter().all(|row| row.kind == Kind::Agent));

        app.on_key(key(KeyCode::Char('0')));
        assert_eq!(app.view, View::Assets);
    }

    #[test]
    fn keymap_editor_rebinds_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "[behavior]\nscan_depth = 2\n",
        )
        .unwrap();
        let mut app = app_with_sources(dir.path());

        app.on_key(key(KeyCode::Char('g')));
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.mode, Mode::Keymap);
        assert!(app.keymap_editor.is_some());

        app.on_key(key(KeyCode::Enter));
        assert!(app.keymap_editor.as_ref().unwrap().capturing);
        app.on_key(key(KeyCode::Char('X')));
        assert_eq!(app.keys.keys_for(Action::Quit).as_deref(), Some("X"));
        assert!(config::keys_path(&app.config_path).exists());

        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        let (cfg, _) = config::load(&app.config_path).unwrap();
        assert_eq!(cfg.keys.get("quit").map(String::as_str), Some("X"));
    }

    #[test]
    fn delete_confirmation_moves_to_trash() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        fs::create_dir_all(&agents).unwrap();
        let path = agents.join("victim.md");
        fs::write(&path, "---\nname: victim\ndescription: x\n---\n").unwrap();
        let mut app = app_with_sources(dir.path());
        app.data_dir = dir.path().join("data");
        app.on_key(key(KeyCode::Char('d')));
        assert_eq!(app.mode, Mode::Confirm);
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!path.exists());
        assert!(app.assets.is_empty());
    }
}
