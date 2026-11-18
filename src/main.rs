//! Arsenal — a terminal UI for agent skills, agents, commands, and rules.

mod app;
mod asset;
mod config;
mod editor;
mod install;
mod keys;
mod search;
mod ui;

use std::collections::BTreeMap;
use std::io::{self, Write};
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
use crate::asset::Kind;
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
    /// Inspect or initialize the configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
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
        Some(Command::Config { action }) => config_command(&action, &config_path),
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
                    .sources
                    .iter()
                    .find(|source| source.kind == kind)
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
        None => {
            let (cfg, warnings) = load_config(&config_path, &cli.roots)?;
            let app = App::new(cfg, config_path, warnings, cli.query.unwrap_or_default());
            run_tui(app)
        }
    }
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
            Effect::Install(request) => {
                let _ = ratatui::try_restore();
                let outcome = install::install(&request).map_err(|error| format!("{error:#}"));
                *terminal = ratatui::try_init().context("re-initializing terminal")?;
                app.after_install(outcome);
            }
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
