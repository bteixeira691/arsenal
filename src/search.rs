//! Fuzzy search and ordering shared by the TUI and `arsenal list`.

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

use crate::asset::{Asset, Kind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    Kind,
    Name,
    Modified,
    Path,
}

impl SortMode {
    pub const ALL: [SortMode; 4] = [
        SortMode::Kind,
        SortMode::Name,
        SortMode::Modified,
        SortMode::Path,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SortMode::Kind => "kind",
            SortMode::Name => "name",
            SortMode::Modified => "modified",
            SortMode::Path => "path",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortMode::Kind => "kind",
            SortMode::Name => "name",
            SortMode::Modified => "modified",
            SortMode::Path => "path",
        }
    }

    pub fn next(self) -> Self {
        match self {
            SortMode::Kind => SortMode::Name,
            SortMode::Name => SortMode::Modified,
            SortMode::Modified => SortMode::Path,
            SortMode::Path => SortMode::Kind,
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim().to_ascii_lowercase();
        SortMode::ALL.into_iter().find(|mode| mode.as_str() == s)
    }
}

pub fn terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.to_string())
        .collect()
}

/// Score an asset against the query terms. A term prefixed with `#` matches
/// tags by substring; everything else is fuzzy-matched against name, kind,
/// tags, and description. Returns `None` when any term fails.
pub fn score(asset: &Asset, terms: &[String], matcher: &SkimMatcherV2) -> Option<i64> {
    if terms.is_empty() {
        return Some(0);
    }
    let haystack = asset.haystack();
    let mut total = 0;
    for term in terms {
        if let Some(tag) = term.strip_prefix('#') {
            if tag.is_empty() {
                continue;
            }
            let needle = tag.to_lowercase();
            if !asset
                .tags
                .iter()
                .any(|candidate| candidate.to_lowercase().contains(&needle))
            {
                return None;
            }
            total += 20;
        } else {
            total += matcher.fuzzy_match(&haystack, term)?;
        }
    }
    Some(total)
}

pub fn select(
    assets: &[Asset],
    kind: Option<Kind>,
    query: &str,
    sort: SortMode,
    matcher: &SkimMatcherV2,
) -> Vec<usize> {
    let terms = terms(query);
    let mut rows: Vec<(usize, i64)> = Vec::new();
    for (index, asset) in assets.iter().enumerate() {
        if let Some(kind) = kind
            && asset.kind != kind
        {
            continue;
        }
        if let Some(score) = score(asset, &terms, matcher) {
            rows.push((index, score));
        }
    }

    if terms.is_empty() {
        sort_rows(assets, &mut rows, sort);
    } else {
        rows.sort_by(|&(a, a_score), &(b, b_score)| {
            b_score
                .cmp(&a_score)
                .then_with(|| name_cmp(&assets[a], &assets[b]))
        });
    }
    rows.into_iter().map(|(index, _)| index).collect()
}

fn sort_rows(assets: &[Asset], rows: &mut [(usize, i64)], sort: SortMode) {
    match sort {
        SortMode::Kind => rows.sort_by(|&(a, _), &(b, _)| {
            assets[a]
                .kind
                .cmp(&assets[b].kind)
                .then_with(|| name_cmp(&assets[a], &assets[b]))
        }),
        SortMode::Name => rows.sort_by(|&(a, _), &(b, _)| name_cmp(&assets[a], &assets[b])),
        SortMode::Modified => rows.sort_by(|&(a, _), &(b, _)| {
            assets[b]
                .modified
                .cmp(&assets[a].modified)
                .then_with(|| name_cmp(&assets[a], &assets[b]))
        }),
        SortMode::Path => rows.sort_by(|&(a, _), &(b, _)| {
            assets[a]
                .path
                .to_string_lossy()
                .to_lowercase()
                .cmp(&assets[b].path.to_string_lossy().to_lowercase())
        }),
    }
}

fn name_cmp(a: &Asset, b: &Asset) -> std::cmp::Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.path.cmp(&b.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn asset(kind: Kind, name: &str, description: &str, tags: &[&str]) -> Asset {
        Asset {
            path: PathBuf::from(format!("/tmp/{name}.md")),
            kind,
            source: "test".into(),
            name: name.into(),
            description: description.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            fields: BTreeMap::new(),
            body: String::new(),
            modified: None,
            size: None,
        }
    }

    struct Fixture {
        assets: Vec<Asset>,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture {
                assets: vec![
                    asset(
                        Kind::Skill,
                        "deploy",
                        "Deploy services safely",
                        &["ops", "infra"],
                    ),
                    asset(
                        Kind::Agent,
                        "reviewer",
                        "Reviews code for defects",
                        &["quality"],
                    ),
                    asset(Kind::Command, "ship", "Ship the release", &["ops"]),
                ],
            }
        }
        fn matcher() -> SkimMatcherV2 {
            SkimMatcherV2::default()
        }
    }

    #[test]
    fn fuzzy_matches_and_orders_by_score() {
        let fixture = Fixture::new();
        let indices = select(
            &fixture.assets,
            None,
            "deploy",
            SortMode::Kind,
            &Fixture::matcher(),
        );
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0], 0);

        let indices = select(
            &fixture.assets,
            None,
            "ops",
            SortMode::Kind,
            &Fixture::matcher(),
        );
        assert_eq!(indices.len(), 2);
    }

    #[test]
    fn tag_filter_and_kind_filter() {
        let fixture = Fixture::new();
        let indices = select(
            &fixture.assets,
            None,
            "#ops",
            SortMode::Kind,
            &Fixture::matcher(),
        );
        assert_eq!(indices.len(), 2);
        let indices = select(
            &fixture.assets,
            Some(Kind::Agent),
            "",
            SortMode::Name,
            &Fixture::matcher(),
        );
        assert_eq!(indices, vec![1]);
        let indices = select(
            &fixture.assets,
            Some(Kind::Agent),
            "#ops",
            SortMode::Name,
            &Fixture::matcher(),
        );
        assert!(indices.is_empty());
    }

    #[test]
    fn sort_modes() {
        let fixture = Fixture::new();
        let matcher = Fixture::matcher();
        let by_kind = select(&fixture.assets, None, "", SortMode::Kind, &matcher);
        assert_eq!(by_kind, vec![0, 1, 2]);
        let by_name = select(&fixture.assets, None, "", SortMode::Name, &matcher);
        assert_eq!(by_name, vec![0, 1, 2]);
    }

    #[test]
    fn cycles_sort_modes() {
        let mut mode = SortMode::Kind;
        let mut seen = vec![mode];
        for _ in 0..3 {
            mode = mode.next();
            seen.push(mode);
        }
        assert_eq!(
            seen,
            vec![
                SortMode::Kind,
                SortMode::Name,
                SortMode::Modified,
                SortMode::Path
            ]
        );
    }
}
