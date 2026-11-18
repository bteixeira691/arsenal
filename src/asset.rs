//! Asset discovery, frontmatter parsing, scaffolding, and trash handling.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::config::{Behavior, Source};

pub const KIND_LETTERS: [char; 5] = ['S', 'A', 'C', 'R', 'P'];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Skill,
    Agent,
    Command,
    Rule,
    Snippet,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Skill,
        Kind::Agent,
        Kind::Command,
        Kind::Rule,
        Kind::Snippet,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Skill => "skill",
            Kind::Agent => "agent",
            Kind::Command => "command",
            Kind::Rule => "rule",
            Kind::Snippet => "snippet",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            Kind::Skill => "skills",
            Kind::Agent => "agents",
            Kind::Command => "commands",
            Kind::Rule => "rules",
            Kind::Snippet => "snippets",
        }
    }

    pub fn badge(self) -> char {
        KIND_LETTERS[self.index()]
    }

    pub fn index(self) -> usize {
        match self {
            Kind::Skill => 0,
            Kind::Agent => 1,
            Kind::Command => 2,
            Kind::Rule => 3,
            Kind::Snippet => 4,
        }
    }

    /// Accepts singular and plural names, case-insensitively.
    pub fn parse_loose(input: &str) -> Option<Self> {
        let s = input.trim().to_ascii_lowercase();
        Kind::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s || k.plural() == s)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Frontmatter {
    pub fields: BTreeMap<String, String>,
    pub body: String,
}

impl Frontmatter {
    pub fn tags(&self) -> Vec<String> {
        let raw = self
            .fields
            .get("tags")
            .or_else(|| self.fields.get("keywords"));
        raw.map(|value| {
            value
                .split(',')
                .map(|tag| tag.trim().trim_start_matches('#').trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect()
        })
        .unwrap_or_default()
    }
}

/// Split YAML frontmatter from a Markdown document. Invalid YAML falls back to a
/// lenient `key: value` parser so hand-written assets keep loading.
pub fn split_frontmatter(raw: &str) -> Frontmatter {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let normalized = raw.replace("\r\n", "\n");
    let mut lines = normalized.split('\n');
    if lines.next().map(str::trim) != Some("---") {
        return Frontmatter {
            fields: BTreeMap::new(),
            body: normalized,
        };
    }

    let rest: Vec<&str> = lines.collect();
    let Some(end) = rest
        .iter()
        .position(|line| matches!(line.trim(), "---" | "..."))
    else {
        // Unterminated frontmatter: treat the document as plain body.
        return Frontmatter {
            fields: BTreeMap::new(),
            body: normalized,
        };
    };

    let yaml = rest[..end].join("\n");
    let body = rest[end + 1..].join("\n");
    let fields = parse_yaml_fields(&yaml).unwrap_or_else(|| parse_lenient_fields(&yaml));
    Frontmatter { fields, body }
}

fn parse_yaml_fields(text: &str) -> Option<BTreeMap<String, String>> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(text).ok()?;
    let mapping = value.as_mapping()?;
    let mut fields = BTreeMap::new();
    for (key, value) in mapping {
        let key = key.as_str()?.trim().to_string();
        if key.is_empty() {
            continue;
        }
        fields.insert(key, flatten_yaml(value));
    }
    Some(fields)
}

fn flatten_yaml(value: &serde_yaml_ng::Value) -> String {
    use serde_yaml_ng::Value;
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => collapse_ws(value),
        Value::Sequence(items) => items
            .iter()
            .map(flatten_yaml)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Mapping(_) | Value::Tagged(_) => {
            let text = serde_yaml_ng::to_string(value).unwrap_or_default();
            collapse_ws(&text)
        }
    }
}

fn parse_lenient_fields(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut last_key: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some(key) = &last_key {
                let entry = fields.entry(key.clone()).or_insert_with(String::new);
                if !entry.is_empty() {
                    entry.push_str(", ");
                }
                entry.push_str(item.trim());
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = &last_key {
                let entry = fields.entry(key.clone()).or_insert_with(String::new);
                if !entry.is_empty() {
                    entry.push(' ');
                }
                entry.push_str(trimmed);
            }
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let key = key.trim().to_string();
            if key.is_empty() {
                continue;
            }
            let value = value.trim();
            let value = if matches!(value, ">" | "|" | ">-" | "|-" | ">+" | "|+") {
                String::new()
            } else {
                value.trim_matches(['"', '\'']).to_string()
            };
            fields.insert(key.clone(), value);
            last_key = Some(key);
        }
    }
    fields
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub path: PathBuf,
    pub kind: Kind,
    pub source: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub fields: BTreeMap<String, String>,
    pub body: String,
    pub modified: Option<SystemTime>,
    pub size: Option<u64>,
    /// Stable content fingerprint used to spot identical copies across roots.
    pub fingerprint: u64,
}

impl Asset {
    pub fn load(path: &Path, kind: Kind, source: &str) -> Result<Asset, String> {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let frontmatter = split_frontmatter(&text);

        let fallback_name = if kind == Kind::Skill {
            path.parent()
                .and_then(Path::file_name)
                .or_else(|| path.file_stem())
        } else {
            path.file_stem()
        }
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());

        let name = frontmatter
            .fields
            .get("name")
            .map(|name| collapse_ws(name))
            .filter(|name| !name.is_empty())
            .unwrap_or(fallback_name);

        let description = frontmatter
            .fields
            .get("description")
            .map(|description| collapse_ws(description))
            .filter(|description| !description.is_empty())
            .unwrap_or_else(|| summarize_body(&frontmatter.body));

        let metadata = fs::metadata(path).ok();
        let modified = metadata.as_ref().and_then(|m| m.modified().ok());
        let size = metadata.as_ref().map(|m| m.len());
        let tags = frontmatter.tags();
        let fingerprint = fingerprint(&bytes);

        Ok(Asset {
            path: path.to_path_buf(),
            kind,
            source: source.to_string(),
            name,
            description: truncate_chars(&description, 320),
            tags,
            fields: frontmatter.fields,
            body: frontmatter.body,
            modified,
            size,
            fingerprint,
        })
    }

    pub fn haystack(&self) -> String {
        format!(
            "{} {} {} {}",
            self.name,
            self.kind.as_str(),
            self.tags.join(" "),
            self.description
        )
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    // FNV-1a: stable across builds and platforms.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn summarize_body(body: &str) -> String {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with("---")
            || line.starts_with("```")
            || line.starts_with("<!--")
        {
            continue;
        }
        let line = line.trim_start_matches(['-', '*', '>', '+', ' ']);
        let line = line.replace("**", "").replace('`', "");
        if !line.is_empty() {
            return truncate_chars(&line, 240);
        }
    }
    String::new()
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub struct ScanReport {
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
    pub active_sources: usize,
}

const MAX_WARNINGS: usize = 25;

pub fn scan(sources: &[Source], behavior: &Behavior) -> ScanReport {
    let mut assets = Vec::new();
    let mut warnings = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut active_sources = 0;

    for source in sources {
        let base = source.resolved();
        if !base.is_dir() {
            continue;
        }
        active_sources += 1;
        for path in candidate_files(source, &base, behavior, &mut warnings) {
            let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical) {
                continue;
            }
            match Asset::load(&path, source.kind, &source.path) {
                Ok(asset) => assets.push(asset),
                Err(error) => warn(&mut warnings, format!("{}: {error}", path.display())),
            }
        }
    }

    assets.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    ScanReport {
        assets,
        warnings,
        active_sources,
    }
}

fn candidate_files(
    source: &Source,
    base: &Path,
    behavior: &Behavior,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if source.kind == Kind::Skill {
        let direct = base.join("SKILL.md");
        if direct.is_file() {
            files.push(direct);
        }
        match fs::read_dir(base) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let hidden = entry.file_name().to_string_lossy().starts_with('.');
                    if hidden {
                        continue;
                    }
                    if path.is_dir() {
                        let skill = path.join("SKILL.md");
                        if skill.is_file() {
                            files.push(skill);
                        }
                    } else if is_markdown(&path) {
                        files.push(path);
                    }
                }
            }
            Err(error) => warn(warnings, format!("{}: {error}", base.display())),
        }
        return files;
    }

    let depth = source.depth.unwrap_or(behavior.scan_depth).clamp(1, 12);
    let walker = WalkDir::new(base)
        .max_depth(depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(is_relevant_entry);
    for entry in walker {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_file() && is_markdown(entry.path()) {
                    files.push(entry.path().to_path_buf());
                }
            }
            Err(error) => warn(warnings, format!("{}: {error}", base.display())),
        }
    }
    files
}

fn is_relevant_entry(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.depth() > 0 && name.starts_with('.') {
        return false;
    }
    if entry.file_type().is_dir()
        && matches!(
            name.as_ref(),
            "node_modules" | "target" | "vendor" | "dist" | "build" | ".git"
        )
    {
        return false;
    }
    true
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown"))
        .unwrap_or(false)
}

fn warn(warnings: &mut Vec<String>, message: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(message);
    } else if warnings.len() == MAX_WARNINGS {
        warnings.push("further scan warnings suppressed".into());
    }
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_dash = !out.is_empty();
        }
    }
    while out.ends_with('.') || out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn validate_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("name is required".into());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("name cannot contain path separators".into());
    }
    if trimmed.chars().count() > 64 {
        return Err("name is limited to 64 characters".into());
    }
    let slug = slugify(trimmed);
    if slug.is_empty() {
        return Err("name must contain at least one letter or digit".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateError {
    InvalidName(String),
    Exists(PathBuf),
    Io(String),
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateError::InvalidName(message) => write!(f, "{message}"),
            CreateError::Exists(path) => write!(f, "{} already exists", path.display()),
            CreateError::Io(message) => write!(f, "{message}"),
        }
    }
}

pub fn create_asset(
    source: &Source,
    kind: Kind,
    name: &str,
    description: &str,
) -> Result<PathBuf, CreateError> {
    validate_name(name).map_err(CreateError::InvalidName)?;
    let slug = slugify(name);
    let base = source.resolved();
    let path = match kind {
        Kind::Skill => base.join(&slug).join("SKILL.md"),
        _ => base.join(format!("{slug}.md")),
    };
    if path.exists() {
        return Err(CreateError::Exists(path));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| CreateError::Io(format!("{}: {e}", parent.display())))?;
    }
    let content = template(kind, &slug, description);
    fs::write(&path, content).map_err(|e| CreateError::Io(format!("{}: {e}", path.display())))?;
    Ok(path)
}

fn yaml_scalar(value: &str) -> String {
    let value = collapse_ws(value);
    let unsafe_plain = value.is_empty()
        || value.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%',
            '@', '`',
        ])
        || value.contains(": ")
        || value.contains(" #");
    if unsafe_plain {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value
    }
}

fn title_from_slug(slug: &str) -> String {
    let mut out = String::new();
    for (index, part) in slug.split(['-', '_']).filter(|p| !p.is_empty()).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

pub fn template(kind: Kind, slug: &str, description: &str) -> String {
    let description = yaml_scalar(description);
    let title = title_from_slug(slug);
    match kind {
        Kind::Skill => format!(
            "---\nname: {slug}\ndescription: {description}\n---\n\n# {title}\n\n## When to use\n\n- \n\n## Procedure\n\n1. \n\n## Verification\n\n- \n"
        ),
        Kind::Agent => format!(
            "---\nname: {slug}\ndescription: {description}\ntools: [read, grep, glob, bash]\nmodel: inherit\n---\n\nYou are a focused specialist. State your role, the inputs you expect, and the result you return.\n\n## Responsibilities\n\n- \n\n## Output\n\n- \n"
        ),
        Kind::Command => format!("---\ndescription: {description}\n---\n\n"),
        Kind::Rule => format!(
            "---\ndescription: {description}\nglobs: [\"**/*\"]\nalwaysApply: false\n---\n\n- \n"
        ),
        Kind::Snippet => format!("---\nname: {slug}\ndescription: {description}\n---\n\n"),
    }
}

/// Recursively copy `src` into `dst`, preserving file permissions and skipping
/// `.git` entries. Returns the number of files copied.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<usize, String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut files = 0;
    let walker = WalkDir::new(src)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git");
    for entry in walker {
        let entry = entry.map_err(|e| e.to_string())?;
        let relative = entry.path().strip_prefix(src).map_err(|e| e.to_string())?;
        let target = dst.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &target).map_err(|e| format!("{}: {e}", target.display()))?;
            files += 1;
        } else if entry.file_type().is_symlink() {
            let metadata = fs::metadata(entry.path()).map_err(|e| e.to_string())?;
            if metadata.is_file() {
                fs::copy(entry.path(), &target)
                    .map_err(|e| format!("{}: {e}", target.display()))?;
                files += 1;
            }
        }
    }
    Ok(files)
}

/// Move a file or directory into `data_dir/trash/<timestamp>/<name>` and return
/// the new path.
pub fn trash_path(path: &Path, data_dir: &Path) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{}.{:09}", d.as_secs(), d.subsec_nanos()))
        .unwrap_or_else(|_| "unknown".into());
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "asset".into());
    let dir = data_dir.join("trash").join(stamp);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let target = dir.join(file_name);
    match fs::rename(path, &target) {
        Ok(()) => Ok(target),
        Err(_) => {
            if path.is_dir() {
                copy_tree(path, &target)?;
                fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
            } else {
                fs::copy(path, &target).map_err(|e| format!("{}: {e}", target.display()))?;
                fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            Ok(target)
        }
    }
}

pub fn format_age(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return "unknown".into();
    };
    let Ok(elapsed) = SystemTime::now().duration_since(modified) else {
        return "just now".into();
    };
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        86_400..=604_799 => format!("{}d ago", secs / 86_400),
        604_800..=2_419_199 => format!("{}w ago", secs / 604_800),
        2_419_200..=31_535_999 => format!("{}mo ago", secs / 2_419_200),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

pub fn format_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "—".into();
    };
    if size < 1024 {
        return format!("{size} B");
    }
    let kb = size as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} KB");
    }
    format!("{:.1} MB", kb / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_frontmatter_with_lists_and_block_scalars() {
        let raw = "---\nname: railway\ndescription: >\n  Operate Railway\n  infrastructure.\ntags: [ops, infra]\ntools: Read, Write, Bash\n---\n\n# Title\n\nBody\n";
        let fm = split_frontmatter(raw);
        assert_eq!(fm.fields.get("name").unwrap(), "railway");
        assert_eq!(
            fm.fields.get("description").unwrap(),
            "Operate Railway infrastructure."
        );
        assert_eq!(fm.tags(), vec!["ops", "infra"]);
        assert_eq!(fm.fields.get("tools").unwrap(), "Read, Write, Bash");
        assert!(fm.body.contains("# Title"));
        assert!(!fm.body.contains("name:"));
    }

    #[test]
    fn lenient_fallback_handles_unquoted_colons() {
        let raw = "---\ndescription: Use when: the build fails\ntags: a, b\n---\nbody\n";
        let fm = split_frontmatter(raw);
        assert_eq!(
            fm.fields.get("description").unwrap(),
            "Use when: the build fails"
        );
        assert_eq!(fm.tags(), vec!["a", "b"]);
    }

    #[test]
    fn documents_without_frontmatter_are_all_body() {
        let fm = split_frontmatter("# Heading\n\nText\n");
        assert!(fm.fields.is_empty());
        assert_eq!(fm.body, "# Heading\n\nText\n");
        let fm = split_frontmatter("---\nname: broken\n");
        assert!(fm.fields.is_empty());
        assert!(fm.body.contains("name: broken"));
    }

    #[test]
    fn windows_line_endings_and_bom() {
        let fm = split_frontmatter("\u{feff}---\r\nname: test\r\n---\r\nbody\r\n");
        assert_eq!(fm.fields.get("name").unwrap(), "test");
        assert_eq!(fm.body, "body\n");
    }

    #[test]
    fn slug_and_name_validation() {
        assert_eq!(slugify("My  Fancy Skill!"), "my-fancy-skill");
        assert_eq!(slugify("already-slugged"), "already-slugged");
        assert_eq!(slugify("Ação Rápida"), "ação-rápida");
        assert_eq!(slugify("--"), "");
        assert!(validate_name("My Skill").is_ok());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn templates_have_valid_frontmatter() {
        let text = template(Kind::Skill, "demo-skill", "Use when: things break");
        let fm = split_frontmatter(&text);
        assert_eq!(fm.fields.get("name").unwrap(), "demo-skill");
        assert_eq!(
            fm.fields.get("description").unwrap(),
            "Use when: things break"
        );
        assert!(fm.body.contains("# Demo Skill"));
        assert!(fm.body.contains("## Procedure"));

        let text = template(Kind::Agent, "reviewer", "Reviews code");
        let fm = split_frontmatter(&text);
        assert_eq!(fm.fields.get("name").unwrap(), "reviewer");
        assert!(fm.fields.contains_key("tools"));
    }

    #[test]
    fn scans_skills_agents_and_nested_commands() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let skill_dir = root.join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: deploy\ndescription: Deploy things\n---\n\n# Deploy\n",
        )
        .unwrap();

        let agents = root.join("agents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews code\n---\nBody\n",
        )
        .unwrap();

        let commands = root.join("commands").join("nested");
        fs::create_dir_all(&commands).unwrap();
        fs::write(
            commands.join("ship.md"),
            "Ship the release. Extra details here.\n",
        )
        .unwrap();
        fs::write(commands.join("ignored.txt"), "not markdown\n").unwrap();
        fs::create_dir_all(commands.join(".hidden")).unwrap();
        fs::write(commands.join(".hidden").join("nope.md"), "x\n").unwrap();

        let sources = vec![
            Source::new(Kind::Skill, root.join("skills").to_string_lossy()),
            Source::new(Kind::Agent, root.join("agents").to_string_lossy()),
            Source::new(Kind::Command, root.join("commands").to_string_lossy()),
            Source::new(Kind::Rule, root.join("missing").to_string_lossy()),
        ];
        let report = scan(&sources, &Behavior::default());
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.active_sources, 3);
        assert_eq!(report.assets.len(), 3);
        let names: Vec<(&str, Kind)> = report
            .assets
            .iter()
            .map(|a| (a.name.as_str(), a.kind))
            .collect();
        assert!(names.contains(&("deploy", Kind::Skill)));
        assert!(names.contains(&("reviewer", Kind::Agent)));
        assert!(names.contains(&("ship", Kind::Command)));
        let ship = report.assets.iter().find(|a| a.name == "ship").unwrap();
        assert_eq!(ship.description, "Ship the release. Extra details here.");
    }

    #[test]
    fn labeled_sources_keep_distinct_asset_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut first = Source::new(Kind::Skill, root.join("one").to_string_lossy());
        first.label = Some("Claude Code".into());
        let mut second = Source::new(Kind::Skill, root.join("two").to_string_lossy());
        second.label = Some("Claude Code".into());
        for (source, name) in [(&first, "one"), (&second, "two")] {
            let skill = source.resolved().join(name);
            fs::create_dir_all(&skill).unwrap();
            fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name}\n---\n"),
            )
            .unwrap();
        }

        let report = scan(&[first.clone(), second.clone()], &Behavior::default());
        assert_eq!(report.assets.len(), 2);
        assert!(report.assets.iter().any(|asset| asset.source == first.path));
        assert!(
            report
                .assets
                .iter()
                .any(|asset| asset.source == second.path)
        );
    }

    #[test]
    fn creates_asset_and_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let source = Source::new(Kind::Skill, dir.path().join("skills").to_string_lossy());
        let path = create_asset(&source, Kind::Skill, "Team Deploy", "Deploys services").unwrap();
        assert!(path.ends_with("team-deploy/SKILL.md"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("name: team-deploy"));
        assert!(text.contains("description: Deploys services"));
        let error = create_asset(&source, Kind::Skill, "Team Deploy", "again").unwrap_err();
        assert!(matches!(error, CreateError::Exists(_)));

        let agent_source = Source::new(Kind::Agent, dir.path().join("agents").to_string_lossy());
        let path = create_asset(&agent_source, Kind::Agent, "Reviewer", "Reviews").unwrap();
        assert!(path.ends_with("reviewer.md"));
    }

    #[test]
    fn copy_tree_skips_git_and_counts_files() {
        let source = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("scripts")).unwrap();
        fs::create_dir_all(source.path().join(".git")).unwrap();
        fs::write(source.path().join("SKILL.md"), "skill").unwrap();
        fs::write(source.path().join("scripts/run.sh"), "echo hi").unwrap();
        fs::write(source.path().join(".git/config"), "ignored").unwrap();

        let dest = tempfile::tempdir().unwrap();
        let target = dest.path().join("copy");
        let files = copy_tree(source.path(), &target).unwrap();
        assert_eq!(files, 2);
        assert!(target.join("SKILL.md").is_file());
        assert!(target.join("scripts/run.sh").is_file());
        assert!(!target.join(".git").exists());
    }

    #[test]
    fn trash_moves_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.md");
        fs::write(&path, "bye").unwrap();
        let trash = dir.path().join("data");
        let target = trash_path(&path, &trash).unwrap();
        assert!(!path.exists());
        assert!(target.exists());
        assert_eq!(fs::read_to_string(&target).unwrap(), "bye");
    }

    #[test]
    fn formats_sizes_and_ages() {
        assert_eq!(format_size(Some(512)), "512 B");
        assert_eq!(format_size(Some(2048)), "2.0 KB");
        assert_eq!(format_size(None), "—");
        assert_eq!(format_age(None), "unknown");
        assert_eq!(format_age(Some(SystemTime::now())), "just now");
    }
}
