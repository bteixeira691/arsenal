//! Key chords, sequences, actions, and the configurable keymap.
//!
//! A binding is a whitespace-separated sequence of chords (vim style): `"g g"`,
//! `"ctrl-d"`, `"?"`. Sequences resolve online: a prefix waits for the next key,
//! an exact match fires immediately, and an unmatched key clears the prefix.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Terminal key, independent of the backend's enum so it can be ordered/hashed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl Key {
    fn name(self) -> String {
        match self {
            Key::Char(' ') => "space".into(),
            Key::Char(c) => c.to_string(),
            Key::Enter => "enter".into(),
            Key::Esc => "esc".into(),
            Key::Tab => "tab".into(),
            Key::BackTab => "backtab".into(),
            Key::Backspace => "backspace".into(),
            Key::Delete => "delete".into(),
            Key::Insert => "insert".into(),
            Key::Left => "left".into(),
            Key::Right => "right".into(),
            Key::Up => "up".into(),
            Key::Down => "down".into(),
            Key::Home => "home".into(),
            Key::End => "end".into(),
            Key::PageUp => "pageup".into(),
            Key::PageDown => "pagedown".into(),
            Key::F(n) => format!("f{n}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Chord {
    pub key: Key,
    pub mods: Mods,
}

impl Chord {
    pub fn parse(token: &str) -> Result<Self, String> {
        let token = token.trim();
        if token.is_empty() {
            return Err("empty key".into());
        }
        let mut mods = Mods::default();
        let key_token: String = if token.contains('+') {
            let parts: Vec<&str> = token
                .split('+')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect();
            let (key, mods_part) = parts
                .split_last()
                .ok_or_else(|| format!("invalid key `{token}`"))?;
            for name in mods_part {
                apply_modifier(&mut mods, name).map_err(|error| format!("{error} in `{token}`"))?;
            }
            (*key).to_string()
        } else if parse_key(token).is_some() {
            token.to_string()
        } else {
            // vim-style `ctrl-d` / `ctrl-shift-k`
            let mut rest = token;
            loop {
                match rest.split_once('-') {
                    Some((head, tail)) if is_modifier_word(head) => {
                        apply_modifier(&mut mods, head)
                            .map_err(|error| format!("{error} in `{token}`"))?;
                        rest = tail;
                    }
                    _ => break,
                }
            }
            if rest.is_empty() {
                return Err(format!("invalid key `{token}`"));
            }
            rest.to_string()
        };

        let key = parse_key(&key_token).ok_or_else(|| format!("unknown key `{token}`"))?;
        let mut chord = Chord { key, mods };
        chord.normalize();
        Ok(chord)
    }

    pub fn from_event(ev: KeyEvent) -> Option<Self> {
        let key = match ev.code {
            KeyCode::Char(c) => Key::Char(c),
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Insert => Key::Insert,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::F(n) => Key::F(n),
            _ => return None,
        };
        let mut chord = Chord {
            key,
            mods: Mods {
                ctrl: ev.modifiers.contains(KeyModifiers::CONTROL),
                alt: ev.modifiers.contains(KeyModifiers::ALT),
                shift: ev.modifiers.contains(KeyModifiers::SHIFT),
            },
        };
        chord.normalize();
        Some(chord)
    }

    /// Shift is meaningful for letters; for punctuation and named keys it is
    /// either implied by the symbol or irrelevant, so it is dropped. This keeps
    /// `?`, `:`, and `G` bindable regardless of the reported modifier bits.
    fn normalize(&mut self) {
        match self.key {
            Key::Char(c) if c.is_ascii_uppercase() => self.mods.shift = true,
            Key::Char(c) if !c.is_ascii_alphabetic() => self.mods.shift = false,
            Key::BackTab => {
                self.mods.shift = true;
            }
            Key::Tab => self.mods.shift = false,
            _ => {}
        }
    }
}

fn is_modifier_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "ctrl" | "control" | "alt" | "meta" | "option" | "shift"
    )
}

fn apply_modifier(mods: &mut Mods, name: &str) -> Result<(), String> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "c" => mods.ctrl = true,
        "alt" | "meta" | "option" | "a" => mods.alt = true,
        "shift" | "s" => mods.shift = true,
        other => return Err(format!("unknown modifier `{other}`")),
    }
    Ok(())
}

fn parse_key(token: &str) -> Option<Key> {
    let token = token.trim();
    let lower = token.to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" | "return" | "cr" => Some(Key::Enter),
        "esc" | "escape" => Some(Key::Esc),
        "tab" => Some(Key::Tab),
        "backtab" | "s-tab" | "shift-tab" => Some(Key::BackTab),
        "backspace" | "bs" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "insert" | "ins" => Some(Key::Insert),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "space" => Some(Key::Char(' ')),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Some(Key::F(n));
    }
    let mut chars = token.chars();
    let c = chars.next()?;
    if chars.next().is_none() {
        Some(Key::Char(c))
    } else {
        None
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.ctrl {
            parts.push("ctrl".into());
        }
        if self.mods.alt {
            parts.push("alt".into());
        }
        let implied_shift =
            matches!(self.key, Key::Char(c) if c.is_ascii_uppercase()) || self.key == Key::BackTab;
        if self.mods.shift && !implied_shift {
            parts.push("shift".into());
        }
        parts.push(self.key.name());
        write!(f, "{}", parts.join("-"))
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct KeySeq(pub Vec<Chord>);

impl KeySeq {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let tokens: Vec<&str> = spec.split_whitespace().collect();
        let chords = tokens
            .iter()
            .map(|t| Chord::parse(t))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(KeySeq(chords))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn starts_with(&self, prefix: &[Chord]) -> bool {
        self.0.starts_with(prefix)
    }

    pub fn display(&self) -> String {
        self.0
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Action {
    Quit,
    Help,
    Command,
    Search,
    NextMatch,
    PrevMatch,
    Create,
    Install,
    AddSource,
    Edit,
    EditInline,
    Delete,
    YankPath,
    Reveal,
    Rescan,
    Config,
    Down,
    Up,
    Top,
    Bottom,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    PreviewDown,
    PreviewUp,
    TogglePreview,
    CycleTheme,
    CycleSort,
    ToggleContentSearch,
    FilterAll,
    FilterSkill,
    FilterAgent,
    FilterCommand,
    FilterRule,
    FilterSnippet,
}

impl Action {
    pub const ALL: [Action; 36] = [
        Action::Quit,
        Action::Help,
        Action::Command,
        Action::Search,
        Action::NextMatch,
        Action::PrevMatch,
        Action::Create,
        Action::Install,
        Action::AddSource,
        Action::Edit,
        Action::EditInline,
        Action::Delete,
        Action::YankPath,
        Action::Reveal,
        Action::Rescan,
        Action::Config,
        Action::Down,
        Action::Up,
        Action::Top,
        Action::Bottom,
        Action::HalfPageDown,
        Action::HalfPageUp,
        Action::PageDown,
        Action::PageUp,
        Action::PreviewDown,
        Action::PreviewUp,
        Action::TogglePreview,
        Action::CycleTheme,
        Action::CycleSort,
        Action::ToggleContentSearch,
        Action::FilterAll,
        Action::FilterSkill,
        Action::FilterAgent,
        Action::FilterCommand,
        Action::FilterRule,
        Action::FilterSnippet,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Help => "help",
            Action::Command => "command",
            Action::Search => "search",
            Action::NextMatch => "next_match",
            Action::PrevMatch => "prev_match",
            Action::Create => "create",
            Action::Install => "install",
            Action::AddSource => "add_source",
            Action::Edit => "edit",
            Action::EditInline => "edit_inline",
            Action::Delete => "delete",
            Action::YankPath => "yank_path",
            Action::Reveal => "reveal",
            Action::Rescan => "rescan",
            Action::Config => "config",
            Action::Down => "down",
            Action::Up => "up",
            Action::Top => "top",
            Action::Bottom => "bottom",
            Action::HalfPageDown => "half_page_down",
            Action::HalfPageUp => "half_page_up",
            Action::PageDown => "page_down",
            Action::PageUp => "page_up",
            Action::PreviewDown => "preview_down",
            Action::PreviewUp => "preview_up",
            Action::TogglePreview => "toggle_preview",
            Action::CycleTheme => "cycle_theme",
            Action::CycleSort => "cycle_sort",
            Action::ToggleContentSearch => "toggle_content_search",
            Action::FilterAll => "filter_all",
            Action::FilterSkill => "filter_skill",
            Action::FilterAgent => "filter_agent",
            Action::FilterCommand => "filter_command",
            Action::FilterRule => "filter_rule",
            Action::FilterSnippet => "filter_snippet",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Quit => "Quit",
            Action::Help => "Keymap help",
            Action::Command => "Command palette",
            Action::Search => "Fuzzy search",
            Action::NextMatch => "Next match",
            Action::PrevMatch => "Previous match",
            Action::Create => "Create asset",
            Action::Install => "Install from git",
            Action::AddSource => "Add source folder",
            Action::Edit => "Edit in $EDITOR",
            Action::EditInline => "Edit document in-app",
            Action::Delete => "Delete (to trash)",
            Action::YankPath => "Copy path (OSC 52)",
            Action::Reveal => "Reveal in file manager",
            Action::Rescan => "Rescan sources",
            Action::Config => "Edit configuration",
            Action::Down => "Move down",
            Action::Up => "Move up",
            Action::Top => "Jump to top",
            Action::Bottom => "Jump to bottom",
            Action::HalfPageDown => "Half page down",
            Action::HalfPageUp => "Half page up",
            Action::PageDown => "Page down",
            Action::PageUp => "Page up",
            Action::PreviewDown => "Scroll preview down",
            Action::PreviewUp => "Scroll preview up",
            Action::TogglePreview => "Toggle preview pane",
            Action::CycleTheme => "Cycle theme preset",
            Action::CycleSort => "Cycle sort order",
            Action::ToggleContentSearch => "Search document contents",
            Action::FilterAll => "Show all kinds",
            Action::FilterSkill => "Show skills",
            Action::FilterAgent => "Show agents",
            Action::FilterCommand => "Show commands",
            Action::FilterRule => "Show rules",
            Action::FilterSnippet => "Show snippets",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Action::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == name.trim().to_ascii_lowercase())
    }
}

pub const DEFAULT_BINDINGS: &[(Action, &str)] = &[
    (Action::Quit, "q"),
    (Action::Help, "?"),
    (Action::Command, ":"),
    (Action::Search, "/"),
    (Action::NextMatch, "n"),
    (Action::PrevMatch, "N"),
    (Action::Create, "a"),
    (Action::Install, "i"),
    (Action::AddSource, "A"),
    (Action::Edit, "e"),
    (Action::EditInline, "enter"),
    (Action::Delete, "d"),
    (Action::YankPath, "y"),
    (Action::Reveal, "o"),
    (Action::Rescan, "R"),
    (Action::Config, "g c"),
    (Action::Down, "j"),
    (Action::Up, "k"),
    (Action::Top, "g g"),
    (Action::Bottom, "G"),
    (Action::HalfPageDown, "ctrl-d"),
    (Action::HalfPageUp, "ctrl-u"),
    (Action::PageDown, "ctrl-f"),
    (Action::PageUp, "ctrl-b"),
    (Action::PreviewDown, "J"),
    (Action::PreviewUp, "K"),
    (Action::TogglePreview, "t"),
    (Action::CycleTheme, "T"),
    (Action::CycleSort, "s"),
    (Action::ToggleContentSearch, "F"),
    (Action::FilterAll, "0"),
    (Action::FilterSkill, "1"),
    (Action::FilterAgent, "2"),
    (Action::FilterCommand, "3"),
    (Action::FilterRule, "4"),
    (Action::FilterSnippet, "5"),
];

/// Default action -> key-sequence map, used to seed the config file and to fill
/// in bindings the user did not override.
pub fn default_map() -> BTreeMap<String, String> {
    DEFAULT_BINDINGS
        .iter()
        .map(|(a, seq)| (a.as_str().to_string(), seq.to_string()))
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<(Action, KeySeq)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    Matched(Action),
    Prefix,
    Unmatched,
}

impl Keymap {
    /// Build a keymap from `action = "sequence"` entries. Unknown actions and
    /// malformed sequences produce warnings and are ignored, keeping the
    /// built-in binding.
    pub fn from_map(map: &BTreeMap<String, String>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut seqs: BTreeMap<Action, KeySeq> = BTreeMap::new();
        for (action, seq) in DEFAULT_BINDINGS {
            match KeySeq::parse(seq) {
                Ok(s) => {
                    seqs.insert(*action, s);
                }
                Err(e) => warnings.push(format!("built-in binding `{action:?}` is invalid: {e}")),
            }
        }
        for (name, seq) in map {
            let Some(action) = Action::parse(name) else {
                warnings.push(format!("unknown key binding `{name}`"));
                continue;
            };
            match KeySeq::parse(seq) {
                Ok(parsed) => {
                    seqs.insert(action, parsed);
                }
                Err(e) => warnings.push(format!("invalid binding for `{name}`: {e}")),
            }
        }

        let mut bindings: Vec<(Action, KeySeq)> = Vec::new();
        let mut seen: BTreeSet<KeySeq> = BTreeSet::new();
        for action in Action::ALL {
            let Some(seq) = seqs.remove(&action) else {
                continue;
            };
            if seq.is_empty() {
                continue;
            }
            if seen.contains(&seq) {
                if let Some((other, _)) = bindings.iter().find(|(_, s)| *s == seq) {
                    warnings.push(format!(
                        "`{}` is bound to both `{}` and `{}`; keeping `{}`",
                        seq.display(),
                        other.as_str(),
                        action.as_str(),
                        other.as_str()
                    ));
                }
                continue;
            }
            seen.insert(seq.clone());
            bindings.push((action, seq));
        }
        (Keymap { bindings }, warnings)
    }

    pub fn resolve(&self, pending: &[Chord]) -> Resolution {
        if pending.is_empty() {
            return Resolution::Unmatched;
        }
        let mut longer = false;
        for (action, seq) in &self.bindings {
            if seq.0 == pending {
                return Resolution::Matched(*action);
            }
            if seq.0.len() > pending.len() && seq.starts_with(pending) {
                longer = true;
            }
        }
        if longer {
            Resolution::Prefix
        } else {
            Resolution::Unmatched
        }
    }

    pub fn keys_for(&self, action: Action) -> Option<String> {
        self.bindings
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, s)| s.display())
    }

    pub fn display_or_dash(&self, action: Action) -> String {
        self.keys_for(action).unwrap_or_else(|| "—".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parses_modifiers_and_named_keys() {
        let c = Chord::parse("ctrl-d").unwrap();
        assert_eq!(c.key, Key::Char('d'));
        assert!(c.mods.ctrl);

        let c = Chord::parse("G").unwrap();
        assert_eq!(c.key, Key::Char('G'));
        assert!(c.mods.shift);

        let c = Chord::parse("?").unwrap();
        assert_eq!(c.key, Key::Char('?'));
        assert!(!c.mods.shift, "shift is implied for punctuation");

        let c = Chord::parse("shift-tab").unwrap();
        assert_eq!(c.key, Key::BackTab);
        assert!(c.mods.shift);
    }

    #[test]
    fn event_normalization_matches_config_chords() {
        let chord = Chord::from_event(ev(KeyCode::Char('?'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(chord, Chord::parse("?").unwrap());
        let chord = Chord::from_event(ev(KeyCode::Char('G'), KeyModifiers::SHIFT)).unwrap();
        assert_eq!(chord, Chord::parse("G").unwrap());
        let chord = Chord::from_event(ev(KeyCode::Char('d'), KeyModifiers::CONTROL)).unwrap();
        assert_eq!(chord, Chord::parse("ctrl-d").unwrap());
    }

    #[test]
    fn sequence_resolution() {
        let map = default_map();
        let (km, warnings) = Keymap::from_map(&map);
        assert!(warnings.is_empty(), "{warnings:?}");

        let g = Chord::parse("g").unwrap();
        let c = Chord::parse("c").unwrap();
        let d = Chord::parse("d").unwrap();
        assert_eq!(km.resolve(&[g]), Resolution::Prefix);
        assert_eq!(km.resolve(&[g, g]), Resolution::Matched(Action::Top));
        assert_eq!(km.resolve(&[g, c]), Resolution::Matched(Action::Config));
        assert_eq!(km.resolve(&[d]), Resolution::Matched(Action::Delete));
        assert_eq!(
            km.resolve(&[Chord::parse("z").unwrap()]),
            Resolution::Unmatched
        );
    }

    #[test]
    fn user_overrides_and_unknown_actions_warn() {
        let mut map = default_map();
        map.insert("quit".into(), "ctrl-q".into());
        map.insert("search".into(), "g s".into());
        map.insert("bogus".into(), "x".into());
        map.insert("down".into(), "nonsense+key".into());
        let (km, warnings) = Keymap::from_map(&map);
        assert_eq!(km.keys_for(Action::Quit).as_deref(), Some("ctrl-q"));
        assert_eq!(
            km.resolve(&[Chord::parse("g").unwrap()]),
            Resolution::Prefix
        );
        assert!(warnings.iter().any(|w| w.contains("bogus")));
        assert!(warnings.iter().any(|w| w.contains("down")));
    }

    #[test]
    fn empty_sequence_unbinds() {
        let mut map = default_map();
        map.insert("yank_path".into(), String::new());
        let (km, _) = Keymap::from_map(&map);
        assert_eq!(km.keys_for(Action::YankPath), None);
    }
}
