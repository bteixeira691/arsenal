//! Arsenal — a terminal UI for agent skills, agents, commands, and rules.

mod app;
mod asset;
mod config;
mod editor;
mod install;
mod integrations;
mod keys;
mod search;
mod ui;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event};
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::DefaultTerminal;
use serde::Serialize;

use crate::app::{App, Effect};
use crate::asset::{Asset, Kind};
use crate::config::{Config, Level, Source};
use crate::search::SortMode;

#[derive(Parser)]
#[command(
    name = "arsenal",
    version,
    about = "Terminal UI for cataloging, searching, and authoring agent skills, agents, commands, and rules",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Configuration file [default: ~/.config/arsenal/config.toml]
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Replace configured sources with KIND=PATH (repeatable)
    #[arg(long = "root", global = true, value_name = "KIND=PATH")]
    roots: Vec<String>,

    /// Initial fuzzy search query (`#tag` filters tags)
    #[arg(long, global = true, value_name = "QUERY")]
    query: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Print assets without launching the TUI
    List {
        /// Emit JSON (one object per asset) for scripting
        #[arg(long)]
        json: bool,
        /// Restrict to one kind: skill, agent, command, rule, snippet
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// Sort order: kind, name, modified, path
        #[arg(long, value_name = "MODE", default_value = "kind")]
        sort: String,
        /// Search document bodies in addition to names, tags, and descriptions
        #[arg(long)]
        content: bool,
    },
    /// Clone a skill or other asset from GitHub/git into a source root
    Install {
        /// GitHub repository, tree, or blob URL, or owner/repo
        source: String,
        /// Asset kind: skill, agent, command, rule, snippet
        #[arg(long, value_name = "KIND", default_value = "skill")]
        kind: String,
        /// Destination root (default: first configured source for the kind)
        #[arg(long, value_name = "PATH")]
        dest: Option<PathBuf>,
        /// Override the installed name
        #[arg(long)]
        name: Option<String>,
        /// Git branch, tag, or commit
        #[arg(long = "ref", value_name = "REF")]
        reference: Option<String>,
        /// Path inside the repository
        #[arg(long, value_name = "PATH")]
        subdir: Option<String>,
        /// Replace an existing destination (moves it to trash)
        #[arg(long)]
        force: bool,
    },
    /// Remove an installed asset (moves it to trash by default)
    Remove {
        /// Asset name or path; omit to choose interactively
        asset: Option<String>,
        /// Restrict removal to one kind
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// Permanently delete instead of moving to trash
        #[arg(long)]
        purge: bool,
    },
    /// Uninstall Arsenal itself
    Uninstall,
    /// List supported agent-tool integration roots
    Integrations {
        /// Emit JSON (one object per root) for scripting
        #[arg(long)]
        json: bool,
    },
    /// Inspect or initialize the configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage extra source folders (.sources.toml overlay)
    Sources {
        #[command(subcommand)]
        action: SourcesAction,
    },
}

#[derive(Subcommand)]
enum SourcesAction {
    /// List effective sources
    List {
        /// Emit JSON for scripting
        #[arg(long)]
        json: bool,
    },
    /// Add an existing folder as a source
    Add {
        /// Directory to scan
        path: String,
        /// Asset kind: skill, agent, command, rule, snippet
        #[arg(long, value_name = "KIND", default_value = "skill")]
        kind: String,
    },
    /// Remove a source added with `sources add`
    Remove {
        /// Directory path used when adding
        path: String,
        /// Restrict removal to one kind
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the configuration file path
    Path,
    /// Write a commented default configuration
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Parse the configuration and report warnings
    Validate,
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli) {
        eprintln!("arsenal: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let config_path = config::config_path(cli.config.as_deref());
    match cli.command {
        Some(Command::Integrations { json }) => integrations_command(json),
        Some(Command::Config { action }) => config_command(&action, &config_path),
        Some(Command::Sources { action }) => sources_command(&action, &config_path),
        Some(Command::List {
            json,
            kind,
            sort,
            content,
        }) => {
            let (cfg, warnings) = load_config(&config_path, &cli.roots)?;
            for warning in warnings {
                eprintln!("warning: {warning}");
            }
            let kind = match kind {
                Some(value) => Some(Kind::parse_loose(&value).ok_or_else(|| {
                    anyhow::anyhow!("unknown kind `{value}` (skill, agent, command, rule, snippet)")
                })?),
                None => None,
            };
            let sort = SortMode::parse(&sort).ok_or_else(|| {
                anyhow::anyhow!("unknown sort `{sort}` (kind, name, modified, path)")
            })?;
            list_assets(
                &cfg,
                cli.query.as_deref().unwrap_or(""),
                kind,
                sort,
                content || cfg.search.content,
                json,
            )
        }
        Some(Command::Install {
            source,
            kind,
            dest,
            name,
            reference,
            subdir,
            force,
        }) => {
            let (cfg, warnings) = load_config(&config_path, &cli.roots)?;
            for warning in warnings {
                eprintln!("warning: {warning}");
            }
            let kind = Kind::parse_loose(&kind).ok_or_else(|| {
                anyhow::anyhow!("unknown kind `{kind}` (skill, agent, command, rule, snippet)")
            })?;
            let dest = match dest {
                Some(path) => config::expand_path(&path.to_string_lossy()),
                None => cfg
                    .install_source(kind)
                    .map(Source::resolved)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no source configured for {}s; add [[sources]] or pass --dest",
                            kind.plural()
                        )
                    })?,
            };
            let request = install::Request {
                url: source,
                kind,
                dest,
                name,
                reference,
                subdir,
                force,
                trash_dir: None,
            };
            let installed = install::install(&request)?;
            println!(
                "installed {} `{}` ({} file(s)) -> {}",
                installed.kind.as_str(),
                installed.name,
                installed.files,
                installed.path.display()
            );
            Ok(())
        }
        Some(Command::Remove { asset, kind, purge }) => {
            let (cfg, warnings) = load_config(&config_path, &cli.roots)?;
            for warning in warnings {
                eprintln!("warning: {warning}");
            }
            let kind = match kind {
                Some(value) => Some(Kind::parse_loose(&value).ok_or_else(|| {
                    anyhow::anyhow!("unknown kind `{value}` (skill, agent, command, rule, snippet)")
                })?),
                None => None,
            };
            remove_asset(&cfg, asset.as_deref(), kind, purge)
        }
        Some(Command::Uninstall) => uninstall_self(),
        None => {
            let (cfg, warnings) = load_config(&config_path, &cli.roots)?;
            let app = App::new(cfg, config_path, warnings, cli.query.unwrap_or_default());
            run_tui(app)
        }
    }
}

#[derive(Serialize)]
struct SourceOut {
    kind: &'static str,
    path: String,
    resolved: String,
    exists: bool,
}

fn integrations_command(json: bool) -> Result<()> {
    #[derive(Serialize)]
    struct Root {
        id: &'static str,
        integration: &'static str,
        kind: &'static str,
        path: String,
        exists: bool,
    }

    let roots = integrations::roots();
    if json {
        for root in roots {
            let row = Root {
                id: root.integration.id,
                integration: root.integration.name,
                kind: root.kind.as_str(),
                path: compact_home(&root.path),
                exists: root.exists,
            };
            println!("{}", serde_json::to_string(&row)?);
        }
        return Ok(());
    }

    println!("{:<12} {:<8} {:<8} PATH", "INTEGRATION", "KIND", "STATUS");
    for root in roots {
        println!(
            "{:<12} {:<8} {:<8} {}",
            root.integration.name,
            root.kind.as_str(),
            if root.exists { "ok" } else { "missing" },
            compact_home(&root.path)
        );
    }
    Ok(())
}

fn compact_home(path: &Path) -> String {
    let home = config::home_dir();
    match path.strip_prefix(&home) {
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => path.display().to_string(),
    }
}

fn uninstall_self() -> Result<()> {
    let exe = std::env::current_exe().context("resolving the Arsenal executable")?;
    let normalized = exe.to_string_lossy().replace('\\', "/");
    if normalized.contains("/target/debug/") || normalized.contains("/target/release/") {
        anyhow::bail!(
            "refusing to remove development build {}; use cargo clean or delete it manually",
            exe.display()
        );
    }

    let from_launcher =
        std::env::var_os("ARSENAL_LAUNCHER").is_some() || normalized.contains("/arsenal/bin/");
    if from_launcher {
        let mut package_removed = false;
        for (program, args) in [
            ("npm", ["uninstall", "-g", "arsenal-tui"]),
            ("bun", ["remove", "-g", "arsenal-tui"]),
        ] {
            match std::process::Command::new(program).args(args).status() {
                Ok(status) if status.success() => {
                    package_removed = true;
                    break;
                }
                _ => {}
            }
        }
        if !package_removed {
            eprintln!(
                "warning: could not remove the arsenal-tui package; remove it with your package manager"
            );
        }
        if let Some(cache) = arsenal_cache_root()
            && cache.exists()
        {
            fs::remove_dir_all(&cache)
                .with_context(|| format!("removing cache {}", cache.display()))?;
        }
    }

    match fs::remove_file(&exe) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("removing {}", exe.display()));
        }
    }
    println!("uninstalled Arsenal from {}", exe.display());
    println!("configuration and managed assets were left in place.");
    Ok(())
}

fn arsenal_cache_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ARSENAL_CACHE_DIR") {
        return Some(PathBuf::from(path));
    }
    if cfg!(target_os = "macos") {
        return Some(config::home_dir().join("Library/Caches/arsenal/bin"));
    }
    if cfg!(target_os = "windows") {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("arsenal/bin"));
    }
    Some(
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| config::home_dir().join(".cache"))
            .join("arsenal/bin"),
    )
}

fn canonical_or(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn removal_target(asset: &Asset) -> PathBuf {
    if asset.kind == Kind::Skill
        && asset
            .path
            .file_name()
            .is_some_and(|name| name == "SKILL.md")
        && let Some(parent) = asset.path.parent()
        && canonical_or(parent) != canonical_or(&config::expand_path(&asset.source))
    {
        return parent.to_path_buf();
    }
    asset.path.clone()
}

fn remove_asset(cfg: &Config, query: Option<&str>, kind: Option<Kind>, purge: bool) -> Result<()> {
    let report = asset::scan(&cfg.sources, &cfg.behavior);
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    let mut candidates: Vec<&Asset> = report
        .assets
        .iter()
        .filter(|asset| kind.is_none_or(|kind| asset.kind == kind))
        .collect();
    if candidates.is_empty() {
        anyhow::bail!("no installed assets found");
    }
    candidates.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.path.cmp(&right.path))
    });

    let selected = if let Some(query) = query {
        let query = query.trim();
        if query.is_empty() {
            anyhow::bail!("asset name or path is empty");
        }
        let target = config::expand_path(query);
        let target_path = canonical_or(&target);
        let mut matches: Vec<&Asset> = candidates
            .iter()
            .copied()
            .filter(|asset| {
                asset.name.eq_ignore_ascii_case(query)
                    || canonical_or(&asset.path) == target_path
                    || (asset.kind == Kind::Skill
                        && asset
                            .path
                            .parent()
                            .is_some_and(|parent| canonical_or(parent) == target_path))
            })
            .collect();
        matches.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.name.cmp(&right.name))
        });
        match matches.as_slice() {
            [] => anyhow::bail!("no asset matches `{query}`"),
            [asset] => *asset,
            many => {
                let listing = many
                    .iter()
                    .map(|asset| {
                        format!(
                            "  {}  {}\n    {}",
                            asset.kind.as_str(),
                            asset.name,
                            asset.path.display()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow::bail!(
                    "multiple assets match `{query}`; narrow with --kind or use a path:\n{listing}"
                );
            }
        }
    } else {
        if !io::stdin().is_terminal() {
            anyhow::bail!("asset name or path is required when stdin is not a terminal");
        }
        println!("Installed assets:");
        for (index, asset) in candidates.iter().enumerate() {
            println!(
                "  {:>2}. {:<8} {:<32} {}",
                index + 1,
                asset.kind.as_str(),
                asset.name,
                asset.path.display()
            );
        }
        print!(
            "Remove which asset? [1-{}] (empty cancels): ",
            candidates.len()
        );
        io::stdout().flush().context("writing prompt")?;
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("reading selection")?;
        let line = line.trim();
        if line.is_empty() {
            println!("cancelled");
            return Ok(());
        }
        let index: usize = line
            .parse()
            .with_context(|| format!("invalid selection `{line}`"))?;
        *candidates
            .get(index.saturating_sub(1))
            .ok_or_else(|| anyhow::anyhow!("selection {index} is out of range"))?
    };

    remove_selected(cfg, selected, purge)
}

fn remove_selected(cfg: &Config, asset: &Asset, purge: bool) -> Result<()> {
    let target = removal_target(asset);
    let trashed = if purge || !cfg.behavior.trash {
        if target.is_dir() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("removing {}", target.display()))?;
        } else {
            fs::remove_file(&target).with_context(|| format!("removing {}", target.display()))?;
        }
        None
    } else {
        Some(asset::trash_path(&target, &config::data_dir()).map_err(anyhow::Error::msg)?)
    };
    println!(
        "uninstalled {} `{}` from {}{}",
        asset.kind.as_str(),
        asset.name,
        target.display(),
        trashed
            .map(|path| format!(" (trash: {})", path.display()))
            .unwrap_or_default()
    );
    Ok(())
}

fn sources_command(action: &SourcesAction, config_path: &Path) -> Result<()> {
    match action {
        SourcesAction::List { json } => {
            let (config, warnings) = load_config(config_path, &[])?;
            for warning in warnings {
                eprintln!("warning: {warning}");
            }
            let items: Vec<SourceOut> = config
                .sources
                .iter()
                .map(|source| SourceOut {
                    kind: source.kind.as_str(),
                    path: source.path.clone(),
                    resolved: source.resolved().display().to_string(),
                    exists: source.resolved().is_dir(),
                })
                .collect();
            if *json {
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else {
                println!("{:<8} {:<8} PATH", "KIND", "STATUS");
                for item in &items {
                    println!(
                        "{:<8} {:<8} {}",
                        item.kind,
                        if item.exists { "ok" } else { "missing" },
                        item.path
                    );
                }
            }
        }
        SourcesAction::Add { path, kind } => {
            let kind = Kind::parse_loose(kind).ok_or_else(|| {
                anyhow::anyhow!("unknown kind `{kind}` (skill, agent, command, rule, snippet)")
            })?;
            let path = path.trim();
            if path.is_empty() {
                anyhow::bail!("source path is empty");
            }
            let source = Source::new(kind, path);
            if !source.resolved().is_dir() {
                anyhow::bail!("not a directory: {}", source.resolved().display());
            }
            match config::add_source(config_path, &source).map_err(anyhow::Error::msg)? {
                true => println!(
                    "added {} source {} (managed in {})",
                    kind.as_str(),
                    path,
                    config::sources_path(config_path).display()
                ),
                false => println!("source already configured: {path}"),
            }
        }
        SourcesAction::Remove { path, kind } => {
            let kind = match kind {
                Some(value) => Some(Kind::parse_loose(value).ok_or_else(|| {
                    anyhow::anyhow!("unknown kind `{value}` (skill, agent, command, rule, snippet)")
                })?),
                None => None,
            };
            let removed =
                config::remove_source(config_path, path, kind).map_err(anyhow::Error::msg)?;
            println!(
                "removed {removed} source entr{} from {}",
                if removed == 1 { "y" } else { "ies" },
                config::sources_path(config_path).display()
            );
        }
    }
    Ok(())
}

fn config_command(action: &ConfigAction, path: &Path) -> Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", path.display());
            Ok(())
        }
        ConfigAction::Init { force } => {
            config::write_default(path, *force)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        ConfigAction::Validate => {
            let path = if path.exists() {
                path
            } else {
                anyhow::bail!("no configuration at {}", path.display());
            };
            let (cfg, warnings) = config::load(path)?;
            println!("ok {}", path.display());
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
            let (_, key_warnings) = keys::Keymap::from_map(&cfg.keys);
            for warning in &key_warnings {
                eprintln!("warning: {warning}");
            }
            Ok(())
        }
    }
}

/// Load configuration, creating a documented default on first run. Scan and
/// parse problems that are non-fatal come back as warnings.
fn load_config(path: &Path, roots: &[String]) -> Result<(Config, Vec<String>)> {
    let mut extra_warnings = Vec::new();
    let (mut cfg, warnings) = if path.exists() {
        config::load(path)?
    } else {
        let cfg = Config::default();
        let mut warnings = Vec::new();
        match config::write_default(path, false) {
            Ok(()) => warnings.push(format!(
                "created default configuration at {}",
                path.display()
            )),
            Err(error) => warnings.push(format!(
                "configuration {} is unavailable ({error:#}); using built-in defaults",
                path.display()
            )),
        }
        (cfg, warnings)
    };
    extra_warnings.extend(warnings);

    if !roots.is_empty() {
        let mut sources = Vec::with_capacity(roots.len());
        for spec in roots {
            sources.push(Source::parse_root(spec)?);
        }
        cfg.sources = sources;
    }

    Ok((cfg, extra_warnings))
}

#[derive(Serialize)]
struct ListedAsset {
    kind: &'static str,
    name: String,
    description: String,
    tags: Vec<String>,
    path: String,
    source: String,
    modified: Option<u64>,
}

fn list_assets(
    cfg: &Config,
    query: &str,
    kind: Option<Kind>,
    sort: SortMode,
    content: bool,
    json: bool,
) -> Result<()> {
    let report = asset::scan(&cfg.sources, &cfg.behavior);
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    let matcher = SkimMatcherV2::default();
    let indices = search::select(&report.assets, kind, query, sort, content, &matcher);

    if json {
        let items: Vec<ListedAsset> = indices
            .iter()
            .map(|&index| {
                let asset = &report.assets[index];
                ListedAsset {
                    kind: asset.kind.as_str(),
                    name: asset.name.clone(),
                    description: asset.description.clone(),
                    tags: asset.tags.clone(),
                    path: asset.path.display().to_string(),
                    source: asset.source.clone(),
                    modified: asset.modified.and_then(|time| {
                        time.duration_since(UNIX_EPOCH)
                            .ok()
                            .map(|duration| duration.as_secs())
                    }),
                }
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        for index in indices {
            let asset = &report.assets[index];
            println!(
                "{:<8}  {:<34}  {}",
                asset.kind.as_str(),
                truncate(&asset.name, 34),
                asset.path.display()
            );
        }
    }

    if !report.warnings.is_empty() {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for asset in &report.assets {
            *counts.entry(asset.kind.as_str()).or_default() += 1;
        }
        eprintln!(
            "scanned {} asset(s): {}",
            report.assets.len(),
            counts
                .iter()
                .map(|(kind, count)| format!("{count} {kind}(s)"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn run_tui(app: App) -> Result<()> {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original_hook(info);
    }));

    let mut terminal = ratatui::try_init().context("initializing terminal")?;
    let result = event_loop(&mut terminal, app);
    let _ = ratatui::try_restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, mut app: App) -> Result<()> {
    loop {
        app.tick();
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        if app.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Paste(text) => app.on_paste(&text),
                _ => {}
            }
            handle_effects(terminal, &mut app)?;
            if let Some(text) = app.take_copy() {
                match write_osc52(&text) {
                    Ok(()) => app.set_status("OSC 52 copy sent", Level::Success),
                    Err(error) => {
                        app.set_status(format!("Clipboard failed: {error}"), Level::Error)
                    }
                }
            }
        }
    }
}

fn handle_effects(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while let Some(effect) = app.take_effect() {
        match effect {
            Effect::EditAsset { path, line } => {
                let _ = ratatui::try_restore();
                let outcome = editor::open(&path, line, &app.cfg.editor);
                *terminal = ratatui::try_init().context("re-initializing terminal")?;
                app.after_edit(false);
                if let Err(error) = outcome {
                    app.set_status(format!("Editor: {error:#}"), Level::Error);
                }
            }
            Effect::EditConfig => {
                let _ = ratatui::try_restore();
                let outcome = editor::open(&app.config_path, 1, &app.cfg.editor);
                *terminal = ratatui::try_init().context("re-initializing terminal")?;
                app.after_edit(true);
                if let Err(error) = outcome {
                    app.set_status(format!("Editor: {error:#}"), Level::Error);
                }
            }
            Effect::Reveal(path) => {
                if let Err(error) = editor::reveal(&path) {
                    app.set_status(format!("Reveal failed: {error:#}"), Level::Error);
                }
            }
            Effect::Install(request) => app.start_install(request),
        }
    }
    Ok(())
}

fn write_osc52(text: &str) -> io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}
