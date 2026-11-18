//! Install skills and other assets from a git repository (GitHub tree/blob
//! URLs, shorthand `owner/repo`, or any git URL).
//!
//! The repository is cloned into a temporary directory, the requested asset is
//! located, and only that asset is copied into the destination root. The
//! temporary clone is always removed.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;

use crate::asset::{self, Kind};
use crate::config;

#[derive(Clone, Debug)]
pub struct Request {
    pub url: String,
    pub kind: Kind,
    /// Source root that receives the asset (e.g. `~/.claude/skills`).
    pub dest: PathBuf,
    pub name: Option<String>,
    pub reference: Option<String>,
    pub subdir: Option<String>,
    /// Replace an existing destination after moving it to the trash.
    pub force: bool,
    /// Override the trash base directory (tests).
    pub trash_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub path: PathBuf,
    pub kind: Kind,
    pub name: String,
    pub files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUrl {
    pub clone_url: String,
    pub reference: Option<String>,
    pub subdir: Option<String>,
}

/// Parse a GitHub repository, tree, or blob URL (or a generic git URL /
/// `owner/repo` shorthand) into a clone URL plus optional ref and subdirectory.
pub fn parse_url(input: &str) -> Result<ParsedUrl> {
    let input = input.trim();
    if input.is_empty() {
        bail!("repository URL is required");
    }
    let without_query = input.split(['?', '#']).next().unwrap_or(input);
    let trimmed = without_query.trim_end_matches('/');

    let web = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("github.com/"));

    if let Some(rest) = web {
        let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
        if segments.len() < 2 {
            bail!("expected a GitHub repository URL, got `{input}`");
        }
        let owner = segments[0];
        let repo = segments[1].trim_end_matches(".git");
        if repo.is_empty() {
            bail!("missing repository name in `{input}`");
        }
        let clone_url = format!("https://github.com/{owner}/{repo}.git");
        if segments.len() == 2 {
            return Ok(ParsedUrl {
                clone_url,
                reference: None,
                subdir: None,
            });
        }
        let marker = segments[2];
        if marker != "tree" && marker != "blob" {
            bail!(
                "unsupported GitHub URL `{input}`; expected a repository, tree/<ref>/..., or blob/<ref>/... URL"
            );
        }
        let reference = segments.get(3).map(|value| (*value).to_string());
        if reference.as_deref().unwrap_or_default().is_empty() {
            bail!("missing ref in `{input}`");
        }
        let subdir = if segments.len() > 4 {
            Some(segments[4..].join("/"))
        } else {
            None
        };
        if marker == "blob" && subdir.is_none() {
            bail!("blob URL `{input}` is missing a file path");
        }
        return Ok(ParsedUrl {
            clone_url,
            reference,
            subdir,
        });
    }

    let looks_like_git = trimmed.starts_with("git@")
        || trimmed.starts_with("ssh://")
        || trimmed.starts_with("git://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("file://");
    if looks_like_git {
        return Ok(ParsedUrl {
            clone_url: trimmed.to_string(),
            reference: None,
            subdir: None,
        });
    }

    let shorthand: Vec<&str> = trimmed.split('/').filter(|part| !part.is_empty()).collect();
    if shorthand.len() == 2 && !shorthand[0].contains('.') {
        return Ok(ParsedUrl {
            clone_url: format!(
                "https://github.com/{}/{}.git",
                shorthand[0],
                shorthand[1].trim_end_matches(".git")
            ),
            reference: None,
            subdir: None,
        });
    }

    bail!("`{input}` is not a GitHub or git repository URL")
}

pub fn repo_name(clone_url: &str) -> String {
    let trimmed = clone_url.trim_end_matches('/').trim_end_matches(".git");
    trimmed
        .rsplit(['/', ':'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("asset")
        .to_string()
}

pub fn install(request: &Request) -> Result<Installed> {
    let parsed = parse_url(&request.url)?;
    let reference = request
        .reference
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or(parsed.reference);
    let subdir = request
        .subdir
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or(parsed.subdir);
    if let Some(reference) = &reference {
        validate_ref(reference)?;
    }

    let workspace = tempfile::tempdir().context("creating a temporary directory")?;
    clone_repo(&parsed.clone_url, reference.as_deref(), workspace.path())?;
    let repo_root = workspace.path();
    let base = match subdir.as_deref() {
        Some(subdir) => join_subdir(repo_root, subdir)?,
        None => repo_root.to_path_buf(),
    };
    if !base.exists() {
        bail!(
            "path `{}` does not exist in `{}`",
            subdir.as_deref().unwrap_or("."),
            request.url
        );
    }

    let default_name = repo_name(&parsed.clone_url);
    let (source, name, is_skill) = match request.kind {
        Kind::Skill => {
            let directory = resolve_skill(&base, request.name.as_deref(), repo_root)?;
            let name = requested_or(
                &request.name,
                declared_skill_name(&directory)
                    .unwrap_or_else(|| default_skill_name(&directory, repo_root, &default_name)),
            );
            (directory, name, true)
        }
        _ => {
            let file = resolve_file(&base, request.name.as_deref(), &default_name)?;
            let name = requested_or(
                &request.name,
                file.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| default_name.clone()),
            );
            (file, name, false)
        }
    };

    let name = asset::slugify(&name).trim_end_matches(".md").to_string();
    if name.is_empty() {
        bail!("could not derive a valid name; pass --name");
    }
    if name.chars().count() > 64 {
        bail!("name `{name}` is longer than 64 characters; pass --name");
    }

    let destination = if is_skill {
        request.dest.join(&name)
    } else {
        request.dest.join(format!("{name}.md"))
    };
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    if destination.exists() {
        if !request.force {
            bail!(
                "{} already exists; pass --force to replace it (the old copy is moved to trash)",
                destination.display()
            );
        }
        let trash = request.trash_dir.clone().unwrap_or_else(config::data_dir);
        asset::trash_path(&destination, &trash).map_err(anyhow::Error::msg)?;
    }

    let files = if is_skill {
        asset::copy_tree(&source, &destination).map_err(anyhow::Error::msg)?
    } else {
        fs::copy(&source, &destination).with_context(|| format!("copying {}", source.display()))?;
        1
    };

    if is_skill && !destination.join("SKILL.md").is_file() {
        bail!(
            "copied skill is missing SKILL.md at {}",
            destination.display()
        );
    }

    Ok(Installed {
        path: destination,
        kind: request.kind,
        name,
        files,
    })
}

fn requested_or(requested: &Option<String>, fallback: String) -> String {
    requested
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or(fallback)
}

fn default_skill_name(directory: &Path, repo_root: &Path, repo_name: &str) -> String {
    if directory == repo_root {
        return repo_name.to_string();
    }
    directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| repo_name.to_string())
}

fn declared_skill_name(directory: &Path) -> Option<String> {
    let text = fs::read_to_string(directory.join("SKILL.md")).ok()?;
    let frontmatter = asset::split_frontmatter(&text);
    frontmatter
        .fields
        .get("name")
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn validate_ref(reference: &str) -> Result<()> {
    if reference.trim().is_empty() {
        bail!("git ref is empty");
    }
    if reference.starts_with('-') || reference.chars().any(char::is_whitespace) {
        bail!("invalid git ref `{reference}`");
    }
    Ok(())
}

fn join_subdir(root: &Path, subdir: &str) -> Result<PathBuf> {
    if subdir.starts_with('/') {
        bail!("invalid subdirectory `{subdir}`");
    }
    let relative = Path::new(subdir.trim_matches('/'));
    for component in relative.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => bail!("invalid subdirectory `{subdir}`"),
        }
    }
    Ok(root.join(relative))
}

fn clone_repo(url: &str, reference: Option<&str>, destination: &Path) -> Result<()> {
    if let Err(error) = attempt_shallow_clone(url, reference, destination) {
        if reference.is_none() {
            return Err(error);
        }
        // A commit SHA (or a ref shallow clone cannot resolve) needs a full
        // clone. The failed attempt may have left a partial directory behind.
        let _ = fs::remove_dir_all(destination);
        fs::create_dir_all(destination)?;
        let status = Command::new("git")
            .arg("clone")
            .arg(url)
            .arg(destination)
            .status()
            .with_context(|| "running `git clone` (is git installed?)")?;
        if !status.success() {
            bail!("git clone failed for `{url}`");
        }
        let reference = reference.expect("checked above");
        let status = Command::new("git")
            .arg("-C")
            .arg(destination)
            .arg("checkout")
            .arg(reference)
            .status()
            .with_context(|| format!("checking out `{reference}`"))?;
        if !status.success() {
            bail!("git checkout `{reference}` failed in `{url}`");
        }
    }
    Ok(())
}

fn attempt_shallow_clone(url: &str, reference: Option<&str>, destination: &Path) -> Result<()> {
    let mut command = Command::new("git");
    command
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--no-tags");
    if let Some(reference) = reference {
        command.arg("--branch").arg(reference);
    }
    command.arg(url).arg(destination);
    let status = command
        .status()
        .with_context(|| "running `git clone` (is git installed?)")?;
    if !status.success() {
        bail!("git clone failed for `{url}`");
    }
    Ok(())
}

fn resolve_skill(base: &Path, requested_name: Option<&str>, repo_root: &Path) -> Result<PathBuf> {
    let base = if base.is_file() {
        if base.file_name().is_some_and(|name| name == "SKILL.md") {
            base.parent()
                .context("SKILL.md has no parent directory")?
                .to_path_buf()
        } else {
            bail!(
                "`{}` is not a SKILL.md file; point at a skill directory",
                display_relative(repo_root, base)
            );
        }
    } else {
        base.to_path_buf()
    };

    if base.join("SKILL.md").is_file() {
        return Ok(base);
    }

    let mut candidates = discover_skills(&base);
    candidates.sort();
    if let Some(requested) = requested_name {
        let wanted = asset::slugify(requested);
        if let Some(found) = candidates.iter().find(|path| {
            path.file_name()
                .map(|name| asset::slugify(&name.to_string_lossy()) == wanted)
                .unwrap_or(false)
        }) {
            return Ok(found.clone());
        }
    }
    match candidates.as_slice() {
        [] => bail!(
            "no SKILL.md found under `{}`; pass the GitHub tree URL of a skill or --subdir",
            display_relative(repo_root, &base)
        ),
        [only] => Ok(only.clone()),
        many => bail!(
            "found {} skills under `{}`; point at one with its GitHub tree URL or --subdir: {}",
            many.len(),
            display_relative(repo_root, &base),
            many.iter()
                .take(8)
                .map(|path| display_relative(repo_root, path))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn resolve_file(base: &Path, requested_name: Option<&str>, repo_name: &str) -> Result<PathBuf> {
    if base.is_file() {
        if is_markdown(base) {
            return Ok(base.to_path_buf());
        }
        bail!("`{}` is not a Markdown file", base.display());
    }

    let mut candidates = top_level_markdown(base)?;
    candidates.sort();
    if let Some(requested) = requested_name {
        let wanted = format!("{}.md", asset::slugify(requested).trim_end_matches(".md"));
        if let Some(found) = candidates.iter().find(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().eq_ignore_ascii_case(&wanted))
                .unwrap_or(false)
        }) {
            return Ok(found.clone());
        }
    }
    let repo_file = base.join(format!("{}.md", asset::slugify(repo_name)));
    if repo_file.is_file() {
        return Ok(repo_file);
    }
    let prose: Vec<PathBuf> = candidates
        .iter()
        .filter(|path| {
            !path
                .file_name()
                .map(|name| {
                    let name = name.to_string_lossy().to_ascii_lowercase();
                    matches!(
                        name.as_str(),
                        "readme.md" | "license.md" | "contributing.md" | "changelog.md"
                    )
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    if prose.len() == 1 {
        return Ok(prose[0].clone());
    }
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }
    match candidates.as_slice() {
        [] => bail!(
            "no Markdown asset found at `{}`; pass --subdir path/to/file.md",
            base.display()
        ),
        _ => bail!(
            "found {} Markdown files at `{}`; pass --subdir to choose one: {}",
            candidates.len(),
            base.display(),
            candidates
                .iter()
                .take(8)
                .map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn top_level_markdown(base: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(base).with_context(|| format!("reading {}", base.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_markdown(&entry.path()) {
            files.push(entry.path());
        }
    }
    Ok(files)
}

fn discover_skills(base: &Path) -> Vec<PathBuf> {
    WalkDir::new(base)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !(entry.depth() > 0
                && (name.starts_with('.')
                    || matches!(
                        name.as_ref(),
                        "node_modules" | "target" | "vendor" | "dist" | "build"
                    )))
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == "SKILL.md")
        .filter_map(|entry| entry.path().parent().map(Path::to_path_buf))
        .collect()
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown"))
        .unwrap_or(false)
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (path, content) in files {
            let target = root.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(target, content).unwrap();
        }
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "--quiet"])
            .status()
            .unwrap();
        assert!(status.success());
        for (key, value) in [("user.email", "test@example.com"), ("user.name", "Test")] {
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["config", key, value])
                .status()
                .unwrap();
        }
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "-A"])
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "--quiet", "-m", "fixture"])
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    fn request(url: &str, kind: Kind, dest: &Path) -> Request {
        Request {
            url: url.to_string(),
            kind,
            dest: dest.to_path_buf(),
            name: None,
            reference: None,
            subdir: None,
            force: false,
            trash_dir: None,
        }
    }

    #[test]
    fn parses_github_urls() {
        let parsed = parse_url("https://github.com/anthropics/skills").unwrap();
        assert_eq!(parsed.clone_url, "https://github.com/anthropics/skills.git");
        assert_eq!(parsed.reference, None);
        assert_eq!(parsed.subdir, None);

        let parsed =
            parse_url("https://github.com/anthropics/skills/tree/main/skills/pdf").unwrap();
        assert_eq!(parsed.clone_url, "https://github.com/anthropics/skills.git");
        assert_eq!(parsed.reference.as_deref(), Some("main"));
        assert_eq!(parsed.subdir.as_deref(), Some("skills/pdf"));

        let parsed =
            parse_url("https://github.com/anthropics/skills/blob/main/skills/pdf/SKILL.md")
                .unwrap();
        assert_eq!(parsed.subdir.as_deref(), Some("skills/pdf/SKILL.md"));

        let parsed = parse_url("owner/repo").unwrap();
        assert_eq!(parsed.clone_url, "https://github.com/owner/repo.git");

        let parsed = parse_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(parsed.clone_url, "git@github.com:owner/repo.git");

        assert!(parse_url("https://github.com/owner/repo/issues").is_err());
        assert!(parse_url("not a url").is_err());
        assert!(parse_url("").is_err());
        assert_eq!(repo_name("https://github.com/a/b.git"), "b");
        assert_eq!(repo_name("git@github.com:a/b.git"), "b");
    }

    #[test]
    fn root_skill_uses_repo_name() {
        assert_eq!(
            default_skill_name(Path::new("/tmp/abc"), Path::new("/tmp/abc"), "my-skill"),
            "my-skill"
        );
        assert_eq!(
            default_skill_name(
                Path::new("/tmp/abc/skills/deploy"),
                Path::new("/tmp/abc"),
                "my-repo"
            ),
            "deploy"
        );
    }

    #[test]
    fn installs_a_skill_subdirectory_with_scripts() {
        let repo = repo_with(&[
            ("README.md", "root readme"),
            (
                "skills/deploy/SKILL.md",
                "---\nname: deploy\ndescription: Deploys\n---\n# Deploy\n",
            ),
            ("skills/deploy/scripts/run.sh", "#!/bin/sh\necho hi\n"),
            ("skills/other/SKILL.md", "---\nname: other\n---\n"),
        ]);
        let dest = tempfile::tempdir().unwrap();
        let mut request = request(
            &format!("file://{}/", repo.path().display()),
            Kind::Skill,
            dest.path(),
        );
        request.subdir = Some("skills/deploy".into());
        let installed = install(&request).unwrap();
        assert_eq!(installed.name, "deploy");
        assert!(installed.path.ends_with("deploy"));
        assert!(installed.path.join("SKILL.md").is_file());
        assert!(installed.path.join("scripts/run.sh").is_file());
        assert_eq!(installed.files, 2);
    }

    #[test]
    fn installs_repo_root_skill_using_declared_name() {
        let repo = repo_with(&[("SKILL.md", "---\nname: local-demo\ndescription: x\n---\n")]);
        let dest = tempfile::tempdir().unwrap();
        let request = request(
            &format!("file://{}/", repo.path().display()),
            Kind::Skill,
            dest.path(),
        );
        let installed = install(&request).unwrap();
        assert_eq!(installed.name, "local-demo");
        assert!(installed.path.ends_with("local-demo"));
        assert!(installed.path.join("SKILL.md").is_file());
    }

    #[test]
    fn installs_repo_root_skill_and_renames() {
        let repo = repo_with(&[("SKILL.md", "---\nname: root-skill\ndescription: x\n---\n")]);
        let dest = tempfile::tempdir().unwrap();
        let mut request = request(
            &format!("file://{}/", repo.path().display()),
            Kind::Skill,
            dest.path(),
        );
        request.name = Some("Renamed Skill".into());
        let installed = install(&request).unwrap();
        assert_eq!(installed.name, "renamed-skill");
        assert!(installed.path.ends_with("renamed-skill"));
        assert!(installed.path.join("SKILL.md").is_file());
    }

    #[test]
    fn installs_a_single_agent_file() {
        let repo = repo_with(&[
            ("README.md", "readme"),
            (
                "reviewer.md",
                "---\nname: reviewer\ndescription: Reviews\n---\nBody\n",
            ),
            ("notes.txt", "ignored"),
        ]);
        let dest = tempfile::tempdir().unwrap();
        let mut request = request(
            &format!("file://{}/", repo.path().display()),
            Kind::Agent,
            dest.path(),
        );
        request.name = Some("reviewer".into());
        let installed = install(&request).unwrap();
        assert_eq!(installed.name, "reviewer");
        assert!(installed.path.ends_with("reviewer.md"));
        assert!(installed.path.is_file());
    }

    #[test]
    fn ambiguous_skills_are_reported_with_paths() {
        let repo = repo_with(&[
            ("skills/a/SKILL.md", "---\nname: a\n---\n"),
            ("skills/b/SKILL.md", "---\nname: b\n---\n"),
        ]);
        let dest = tempfile::tempdir().unwrap();
        let request = request(
            &format!("file://{}/", repo.path().display()),
            Kind::Skill,
            dest.path(),
        );
        let error = install(&request).unwrap_err().to_string();
        assert!(error.contains("found 2 skills"), "{error}");
        assert!(error.contains("skills/a"), "{error}");
    }

    #[test]
    fn refuses_existing_destination_unless_forced() {
        let repo = repo_with(&[("skills/demo/SKILL.md", "---\nname: demo\n---\n")]);
        let dest = tempfile::tempdir().unwrap();
        let existing = dest.path().join("demo");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("SKILL.md"), "old").unwrap();

        let mut request = request(
            &format!("file://{}/", repo.path().display()),
            Kind::Skill,
            dest.path(),
        );
        request.subdir = Some("skills/demo".into());
        let error = install(&request).unwrap_err().to_string();
        assert!(error.contains("already exists"), "{error}");

        request.force = true;
        request.trash_dir = Some(dest.path().join("data"));
        let installed = install(&request).unwrap();
        assert!(installed.path.join("SKILL.md").is_file());
        assert_ne!(
            fs::read_to_string(installed.path.join("SKILL.md")).unwrap(),
            "old"
        );
        let trash = WalkDir::new(dest.path().join("data").join("trash"))
            .into_iter()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == "demo");
        assert!(trash);
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(join_subdir(Path::new("/tmp"), "../etc").is_err());
        assert!(join_subdir(Path::new("/tmp"), "/etc").is_err());
        assert!(join_subdir(Path::new("/tmp"), "skills/x").is_ok());
    }

    #[test]
    fn invalid_ref_is_rejected() {
        assert!(validate_ref("--help").is_err());
        assert!(validate_ref("main branch").is_err());
        assert!(validate_ref("main").is_ok());
    }
}
