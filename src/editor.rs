//! External editor and file-manager integration.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::Editor;

/// Open `path` in the configured editor at `line`. The TUI must be restored
/// before calling this and re-initialized afterwards.
pub fn open(path: &Path, line: usize, cfg: &Editor) -> Result<()> {
    let command = if !cfg.command.trim().is_empty() {
        cfg.command.trim().to_string()
    } else {
        first_non_empty([std::env::var("VISUAL").ok(), std::env::var("EDITOR").ok()])
            .unwrap_or_else(|| "vi".into())
    };

    let mut words = command.split_whitespace();
    let program = words.next().context("editor command is empty")?;
    let mut process = Command::new(program);
    process.args(words);

    let args = if cfg.args.is_empty() {
        vec!["+{line}".to_string(), "{file}".to_string()]
    } else {
        cfg.args.clone()
    };
    let file = path.display().to_string();
    let mut has_file = false;
    for arg in &args {
        if arg.contains("{file}") {
            has_file = true;
        }
        process.arg(
            arg.replace("{file}", &file)
                .replace("{line}", &line.to_string()),
        );
    }
    if !has_file {
        process.arg(path);
    }

    let status = process
        .status()
        .with_context(|| format!("running editor `{command}`"))?;
    if !status.success() {
        bail!("editor `{command}` exited with {status}");
    }
    Ok(())
}

fn first_non_empty(values: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

/// Reveal the asset in the platform file manager. Failures are reported in the
/// statusline; a missing file manager never breaks the session.
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .context("running `open -R`")?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .context("running explorer")?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = path.parent().unwrap_or(path);
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .context("running xdg-open")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_first_non_empty_editor() {
        assert_eq!(
            first_non_empty([Some("".into()), Some("nvim".into())]),
            Some("nvim".into())
        );
        assert_eq!(first_non_empty([None, None]), None);
    }

    #[test]
    fn editor_argument_substitution_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file with space.md");
        std::fs::write(&path, "x").unwrap();
        let cfg = Editor {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "printf %s \"$1\" > \"$2\"".into(),
                "sh".into(),
                "{line}".into(),
                "{file}".into(),
            ],
        };
        open(&path, 7, &cfg).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "7");
    }
}
