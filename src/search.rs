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
/// tags, and description. With `content`, terms also match document bodies
/// (case-insensitive substring), with a bonus when both match. Returns `None`
/// when any term fails.
pub fn score(
    asset: &Asset,
    terms: &[String],
    matcher: &SkimMatcherV2,
    content: bool,
) -> Option<i64> {
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
            let metadata = matcher.fuzzy_match(&haystack, term);
            let body = content.then(|| content_score(&asset.body, term)).flatten();
            match (metadata, body) {
                (None, None) => return None,
                (Some(score), None) => total += score,
                (None, Some(score)) => total += score,
                (Some(metadata), Some(body)) => total += metadata + body,
            }
        }
    }
    Some(total)
}

fn content_score(body: &str, term: &str) -> Option<i64> {
    let needle = term.to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let matches = body
        .lines()
        .filter(|line| line.to_lowercase().contains(&needle))
        .count();
    if matches == 0 {
        None
    } else {
        Some(40 + (matches.min(10) as i64) * 2)
    }
}

/// Terms used for body highlighting; tag filters (`#tag`) are metadata-only.
pub fn content_terms(query: &str) -> Vec<String> {
    terms(query)
        .into_iter()
        .filter(|term| !term.starts_with('#'))
        .map(|term| term.to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

pub fn line_matches(line: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let lower = line.to_lowercase();
    terms.iter().any(|term| lower.contains(term))
}

pub fn matching_lines(body: &str, terms: &[String]) -> usize {
    if terms.is_empty() {
        return 0;
    }
    body.lines()
        .filter(|line| line_matches(line, terms))
        .count()
}

pub fn select(
    assets: &[Asset],
    kind: Option<Kind>,
    query: &str,
    sort: SortMode,
    content: bool,
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
        if let Some(score) = score(asset, &terms, matcher, content) {
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
            fingerprint: 0,
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
            false,
            &Fixture::matcher(),
        );
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0], 0);

        let indices = select(
            &fixture.assets,
            None,
            "ops",
            SortMode::Kind,
            false,
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
            false,
            &Fixture::matcher(),
        );
        assert_eq!(indices.len(), 2);
        let indices = select(
            &fixture.assets,
            Some(Kind::Agent),
            "",
            SortMode::Name,
            false,
            &Fixture::matcher(),
        );
        assert_eq!(indices, vec![1]);
        let indices = select(
            &fixture.assets,
            Some(Kind::Agent),
            "#ops",
            SortMode::Name,
            false,
            &Fixture::matcher(),
        );
        assert!(indices.is_empty());
    }

    #[test]
    fn sort_modes() {
        let fixture = Fixture::new();
        let matcher = Fixture::matcher();
        let by_kind = select(&fixture.assets, None, "", SortMode::Kind, false, &matcher);
        assert_eq!(by_kind, vec![0, 1, 2]);
        let by_name = select(&fixture.assets, None, "", SortMode::Name, false, &matcher);
        assert_eq!(by_name, vec![0, 1, 2]);
    }

    #[test]
    fn content_search_matches_body_only_when_enabled() {
        let mut fixture = Fixture::new();
        fixture.assets[0].body = "# Deploy\nRun the migration checklist.".into();
        let matcher = Fixture::matcher();

        let indices = select(
            &fixture.assets,
            None,
            "migration",
            SortMode::Kind,
            false,
            &matcher,
        );
        assert!(indices.is_empty());

        let indices = select(
            &fixture.assets,
            None,
            "migration",
            SortMode::Kind,
            true,
            &matcher,
        );
        assert_eq!(indices, vec![0]);

        let terms = content_terms("migration #ops");
        assert_eq!(terms, vec!["migration"]);
        assert!(line_matches("Run the migration checklist.", &terms));
        assert_eq!(matching_lines(&fixture.assets[0].body, &terms), 1);
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
