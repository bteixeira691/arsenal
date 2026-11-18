//! Known agent-tool layouts.
//!
//! Each integration contributes user and project roots for the asset kinds it
//! supports. The paths intentionally mirror the tools' documented discovery
//! roots; a missing root is harmless and simply produces no assets.

use std::path::PathBuf;

use crate::asset::Kind;
use crate::config::Source;

#[derive(Clone, Copy, Debug)]
pub struct Integration {
    pub id: &'static str,
    pub name: &'static str,
    pub user_bases: &'static [&'static str],
    pub project_bases: &'static [&'static str],
    pub kinds: &'static [Kind],
}

pub const INTEGRATIONS: &[Integration] = &[
    Integration {
        id: "omp",
        name: "OMP",
        user_bases: &["~/.omp/agent"],
        project_bases: &["./.omp"],
        kinds: &[Kind::Skill, Kind::Agent, Kind::Command, Kind::Rule],
    },
    Integration {
        id: "claude",
        name: "Claude Code",
        user_bases: &["~/.claude"],
        project_bases: &["./.claude"],
        kinds: &[Kind::Skill, Kind::Agent, Kind::Command, Kind::Rule],
    },
    Integration {
        id: "opencode",
        name: "OpenCode",
        user_bases: &["~/.config/opencode"],
        project_bases: &["./.opencode"],
        kinds: &[Kind::Skill, Kind::Agent, Kind::Command],
    },
    Integration {
        id: "codex",
        name: "Codex",
        user_bases: &["~/.codex"],
        project_bases: &["./.codex"],
        kinds: &[Kind::Skill],
    },
    Integration {
        id: "gemini",
        name: "Gemini CLI",
        user_bases: &["~/.gemini"],
        project_bases: &["./.gemini"],
        kinds: &[Kind::Skill],
    },
    Integration {
        id: "agents",
        name: "Agents",
        user_bases: &["~/.agents", "~/.agent"],
        project_bases: &["./.agents", "./.agent"],
        kinds: &[Kind::Skill, Kind::Agent, Kind::Command, Kind::Rule],
    },
    Integration {
        id: "github",
        name: "GitHub",
        user_bases: &[],
        project_bases: &["./.github"],
        kinds: &[Kind::Skill],
    },
];

/// Every configured integration root, as a labeled source.
pub fn sources() -> Vec<Source> {
    let mut sources = Vec::new();
    for integration in INTEGRATIONS {
        for base in integration
            .user_bases
            .iter()
            .chain(integration.project_bases.iter())
        {
            for kind in integration.kinds {
                let mut source = Source::new(*kind, format!("{}/{}", base, kind.plural()));
                source.label = Some(integration.name.to_string());
                sources.push(source);
            }
        }
    }
    sources
}

/// Infer the integration name for an existing source path. Custom paths still
/// have no label.
pub fn label_for_path(path: &str) -> Option<&'static str> {
    let normalized = format!("/{}", path.replace('\\', "/"));
    let normalized = normalized.trim_end_matches('/');
    if normalized.contains("/.omp/agent/") || normalized.contains("/.omp/") {
        Some("OMP")
    } else if normalized.contains("/.claude/") {
        Some("Claude Code")
    } else if normalized.contains("/.config/opencode/") || normalized.contains("/.opencode/") {
        Some("OpenCode")
    } else if normalized.contains("/.codex/") {
        Some("Codex")
    } else if normalized.contains("/.gemini/") {
        Some("Gemini CLI")
    } else if normalized.contains("/.agents/") || normalized.contains("/.agent/") {
        Some("Agents")
    } else if normalized.contains("/.github/") {
        Some("GitHub")
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct IntegrationRoot {
    pub integration: Integration,
    pub kind: Kind,
    pub path: PathBuf,
    pub exists: bool,
}

/// Paths shown by `arsenal integrations`.
pub fn roots() -> Vec<IntegrationRoot> {
    let mut roots = Vec::new();
    for integration in INTEGRATIONS {
        for base in integration
            .user_bases
            .iter()
            .chain(integration.project_bases.iter())
        {
            for kind in integration.kinds {
                let path = crate::config::expand_path(&format!("{}/{}", base, kind.plural()));
                let exists = path.is_dir();
                roots.push(IntegrationRoot {
                    integration: *integration,
                    kind: *kind,
                    path,
                    exists,
                });
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_sources_are_labeled_and_unique() {
        let sources = sources();
        assert!(sources.iter().any(|source| {
            source.label.as_deref() == Some("OpenCode")
                && source.kind == Kind::Skill
                && source.path == "~/.config/opencode/skills"
        }));
        assert!(sources.iter().any(|source| {
            source.label.as_deref() == Some("Claude Code")
                && source.kind == Kind::Command
                && source.path == "~/.claude/commands"
        }));
        let mut seen = std::collections::BTreeSet::new();
        for source in sources {
            assert!(seen.insert((source.kind, source.path)));
        }
    }

    #[test]
    fn labels_are_inferred_from_common_paths() {
        assert_eq!(label_for_path("~/.omp/agent/skills"), Some("OMP"));
        assert_eq!(label_for_path("./.claude/agents"), Some("Claude Code"));
        assert_eq!(
            label_for_path("~/.config/opencode/commands"),
            Some("OpenCode")
        );
        assert_eq!(label_for_path("~/.codex/skills"), Some("Codex"));
        assert_eq!(label_for_path("./.github/skills"), Some("GitHub"));
        assert_eq!(label_for_path("~/custom/skills"), None);
    }
}
