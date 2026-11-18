//! Rendering: header, asset list, preview, statusline, and modal dialogs.

use ratatui::prelude::*;
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Tabs, Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, FormField, InstallField, Mode, SourceField, TextInput, View};
use crate::asset::{Kind, format_age, format_size};
use crate::config::{Level, ResolvedTheme};
use crate::install::Phase;
use crate::keys::Action;
use crate::search;

const HELP_GROUPS: &[(&str, &[Action])] = &[
    (
        "Navigation",
        &[
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
        ],
    ),
    (
        "Actions",
        &[
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
        ],
    ),
    (
        "Search & views",
        &[
            Action::Search,
            Action::FocusSearch,
            Action::FocusPreview,
            Action::NextMatch,
            Action::PrevMatch,
            Action::FilterAll,
            Action::FilterSkill,
            Action::FilterAgent,
            Action::FilterCommand,
            Action::FilterRule,
            Action::FilterSnippet,
            Action::CycleSort,
            Action::ToggleContentSearch,
            Action::ShowSources,
            Action::TogglePreview,
            Action::CycleTheme,
        ],
    ),
    (
        "Application",
        &[
            Action::Help,
            Action::Command,
            Action::EditKeymap,
            Action::Quit,
        ],
    ),
];

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let theme = app.theme();
    if area.width < 24 || area.height < 9 {
        let notice = Paragraph::new(" Terminal too small for arsenal ")
            .style(Style::new().fg(theme.error))
            .alignment(Alignment::Center);
        f.render_widget(notice, area);
        return;
    }

    let tabs_height = if app.mode == Mode::Edit { 0 } else { 1 };
    let search_height = if app.mode == Mode::Edit { 0 } else { 3 };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(search_height),
        Constraint::Length(tabs_height),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    header(f, chunks[0], app, &theme);
    if search_height > 0 {
        search_bar(f, chunks[1], app, &theme);
    }
    if tabs_height > 0 {
        kind_tabs(f, chunks[2], app, &theme);
    }

    let body = chunks[3];
    if app.mode == Mode::Edit {
        editor_view(f, body, app, &theme);
    } else if app.view == View::Sources {
        if app.show_preview && body.width >= 60 {
            let list_width = app.cfg.layout.list_width.clamp(20, 70).min(80);
            let [list_area, preview_area] =
                Layout::horizontal([Constraint::Percentage(list_width), Constraint::Fill(1)])
                    .areas(body);
            source_list(f, list_area, app, &theme);
            source_preview(f, preview_area, app, &theme);
        } else {
            source_list(f, body, app, &theme);
        }
    } else if app.show_preview && body.width >= 60 {
        let list_width = app.cfg.layout.list_width.clamp(20, 70).min(80);
        let [list_area, preview_area] =
            Layout::horizontal([Constraint::Percentage(list_width), Constraint::Fill(1)])
                .areas(body);
        asset_list(f, list_area, app, &theme);
        preview(f, preview_area, app, &theme);
    } else {
        asset_list(f, body, app, &theme);
    }

    status_bar(f, chunks[4], app, &theme);

    match app.mode {
        Mode::Form => form_modal(f, area, app, &theme),
        Mode::Install => install_modal(f, area, app, &theme),
        Mode::InstallProgress => install_progress_modal(f, area, app, &theme),
        Mode::Source => source_modal(f, area, app, &theme),
        Mode::Keymap => keymap_modal(f, area, app, &theme),
        Mode::Confirm => confirm_modal(f, area, app, &theme),
        Mode::Help => help_modal(f, area, app, &theme),
        _ => {}
    }
    if app.mode == Mode::Edit && app.discard_prompt {
        discard_modal(f, area, &theme);
    }
}

fn search_bar(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let focused = app.mode == Mode::Search;
    let title = if app.view == View::Sources {
        " Search sources "
    } else {
        " Search assets "
    };
    let title_style = if focused {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.muted)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { theme.accent } else { theme.border }))
        .title(Span::styled(title, title_style));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let compact = inner.width < 72;
    let right_text = if compact {
        String::new()
    } else if focused {
        "ctrl-l preview · enter keep · esc revert".to_string()
    } else if app.mode == Mode::Normal && app.focus == Focus::Preview {
        "ctrl-h search".to_string()
    } else if app.content_search {
        format!("/ focus · content · sort {}", app.sort.label())
    } else {
        format!("/ focus · sort {}", app.sort.label())
    };
    let right_width = (right_text.width() as u16 + 1).min(inner.width / 2);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(4), Constraint::Length(right_width)]).areas(inner);

    let mut spans = vec![Span::styled(
        " / ",
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )];
    if focused {
        let budget = left_area.width.saturating_sub(3) as usize;
        spans.extend(input_spans_clipped(&app.input, theme, budget));
    } else if app.query.is_empty() {
        let placeholder = if app.view == View::Sources {
            if compact {
                "Search sources…"
            } else {
                "Search source folders…"
            }
        } else if compact {
            "Search skills…"
        } else {
            "Search skills, agents, commands, rules…"
        };
        spans.push(Span::styled(placeholder, Style::new().fg(theme.border)));
    } else {
        spans.push(Span::styled(
            app.query.clone(),
            Style::new().fg(Color::Reset),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), left_area);
    if right_width > 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{right_text} "),
                Style::new().fg(theme.muted),
            )))
            .alignment(Alignment::Right),
            right_area,
        );
    }
}

fn editor_view(f: &mut Frame, area: Rect, app: &mut App, theme: &ResolvedTheme) {
    let Some(editor) = app.editor.as_mut() else {
        return;
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if editor.dirty {
            theme.warning
        } else {
            theme.accent
        }))
        .title(Line::from(vec![
            Span::styled(
                " Editing ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("· ", Style::new().fg(theme.border)),
            Span::styled(
                editor.label.clone(),
                Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if editor.dirty { "  ●" } else { "" },
                Style::new().fg(theme.warning).add_modifier(Modifier::BOLD),
            ),
        ]))
        .title_bottom(Line::from(Span::styled(
            format!(" {} ", compact_path(&editor.path)),
            Style::new().fg(theme.muted),
        )));
    editor.textarea.set_block(block);
    editor.textarea.set_style(Style::new().fg(Color::Reset));
    editor
        .textarea
        .set_line_number_style(Style::new().fg(theme.border));
    editor.textarea.set_cursor_style(
        Style::new()
            .fg(theme.accent)
            .add_modifier(Modifier::REVERSED),
    );
    editor
        .textarea
        .set_cursor_line_style(Style::new().add_modifier(Modifier::DIM));
    f.render_widget(&editor.textarea, area);
}

fn discard_modal(f: &mut Frame, area: Rect, theme: &ResolvedTheme) {
    let width = (area.width.saturating_sub(4)).min(58);
    let height = 7.min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.warning))
        .title(Span::styled(
            " Unsaved changes ",
            Style::new().fg(theme.warning).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(Span::styled(
            "Discard edits to this document?",
            Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " y ",
                Style::new().fg(theme.error).add_modifier(Modifier::BOLD),
            ),
            Span::styled("discard", Style::new().fg(theme.muted)),
            Span::styled(
                "    n ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("keep editing", Style::new().fg(theme.muted)),
        ]),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn kind_tabs(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let compact = area.width < 96;
    let mut labels: Vec<Line<'static>> = Vec::new();
    let mut add = |name: &str, count: usize| {
        labels.push(Line::from(vec![
            Span::raw(format!(" {name} ")),
            Span::styled(format!("{count}"), Style::new().fg(theme.border)),
            Span::raw(" "),
        ]));
    };
    add("All", app.assets.len());
    for kind in Kind::ALL {
        let name = if compact {
            match kind {
                Kind::Skill => "S",
                Kind::Agent => "A",
                Kind::Command => "C",
                Kind::Rule => "R",
                Kind::Snippet => "P",
            }
        } else {
            match kind {
                Kind::Skill => "Skills",
                Kind::Agent => "Agents",
                Kind::Command => "Commands",
                Kind::Rule => "Rules",
                Kind::Snippet => "Snippets",
            }
        };
        add(name, app.counts[kind.index()]);
    }
    add(
        if compact { "Src" } else { "Sources" },
        app.cfg.sources.len(),
    );

    let selected = if app.view == View::Sources {
        6
    } else {
        app.kind_filter.map(|kind| kind.index() + 1).unwrap_or(0)
    };
    let tabs = Tabs::new(labels)
        .select(selected)
        .style(Style::new().fg(theme.muted))
        .highlight_style(theme.selection)
        .divider(Span::styled(" ", Style::new().fg(theme.border)));
    f.render_widget(tabs, area);
}

fn header(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let right_text = if app.view == View::Sources {
        format!("{}/{} sources ", app.source_count(), app.cfg.sources.len())
    } else {
        format!("{}/{} assets ", app.filtered.len(), app.assets.len())
    };
    let right_width = right_text.width() as u16;
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(right_width)]).areas(area);

    let filter = if app.view == View::Sources {
        "sources"
    } else {
        match app.kind_filter {
            Some(kind) => kind.plural(),
            None => "all kinds",
        }
    };
    let pending = if app.pending.is_empty() {
        String::new()
    } else {
        format!(
            "  [{}]",
            app.pending
                .iter()
                .map(|chord| chord.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let left = Line::from(vec![
        Span::styled(
            " ARSENAL ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::new().fg(theme.border)),
        Span::styled(filter.to_string(), Style::new().fg(Color::Reset)),
        Span::styled(pending, Style::new().fg(theme.warning)),
    ]);
    f.render_widget(Paragraph::new(left), left_area);

    let right = Paragraph::new(Line::from(Span::styled(
        right_text,
        Style::new().fg(theme.muted),
    )))
    .alignment(Alignment::Right);
    f.render_widget(right, right_area);
}

fn source_list(f: &mut Frame, area: Rect, app: &mut App, theme: &ResolvedTheme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(Line::from(vec![
            Span::styled(
                " Sources ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", app.cfg.sources.len()),
                Style::new().fg(theme.muted),
            ),
        ]));
    let inner = block.inner(area);
    app.list_height = inner.height;
    app.ensure_visible();
    let rows = app.source_rows();

    if rows.is_empty() {
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No sources match.",
                Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  A  add a source folder",
                Style::new().fg(theme.muted),
            )),
            Line::from(Span::styled(
                "  R  rescan    g c  configuration",
                Style::new().fg(theme.muted),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let scrollbar = rows.len() > inner.height as usize;
    let content_area = Rect {
        width: inner.width.saturating_sub(u16::from(scrollbar)),
        ..inner
    };
    f.render_widget(block, area);

    let start = app.source_top.min(rows.len());
    let end = (start + inner.height as usize).min(rows.len());
    let width = content_area.width as usize;
    let mut items = Vec::with_capacity(end - start);
    for (position, row) in rows.iter().enumerate().skip(start).take(end - start) {
        let selected = position == app.source_selected;
        let badge = format!(" {} ", row.kind.badge());
        let status = if row.exists { "ok" } else { "missing" };
        let assets = format!(
            "{} asset{}",
            row.assets,
            if row.assets == 1 { "" } else { "s" }
        );
        let meta = format!("{} · {} · {}", row.kind.as_str(), status, assets);
        let path_budget = width
            .saturating_sub(badge.width() + meta.width() + 3)
            .max(8);
        let path = truncate_width(&row.path, path_budget);
        let mut spans = vec![
            Span::styled(
                badge,
                Style::new()
                    .fg(theme.kind_color(row.kind))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                path,
                Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                meta,
                Style::new().fg(if row.exists { theme.muted } else { theme.error }),
            ),
        ];
        if let Some(label) = &row.label {
            spans.push(Span::styled(
                format!("  [{label}]"),
                Style::new().fg(theme.accent),
            ));
        }
        let mut item = ListItem::new(Line::from(spans));
        if selected {
            item = item.style(theme.selection);
        }
        items.push(item);
    }
    f.render_widget(List::new(items), content_area);

    if scrollbar {
        let mut state = ScrollbarState::new(rows.len())
            .position(app.source_top)
            .viewport_content_length(inner.height as usize);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::new().fg(theme.accent))
            .track_style(Style::new().fg(theme.border))
            .track_symbol(Some("│"));
        f.render_stateful_widget(bar, inner, &mut state);
    }
}

fn source_preview(f: &mut Frame, area: Rect, app: &mut App, theme: &ResolvedTheme) {
    let Some(row) = app.selected_source() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.border))
            .title(Span::styled(" Source ", Style::new().fg(theme.muted)));
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Select a source with j/k",
                Style::new().fg(theme.muted),
            )))
            .block(block),
            area,
        );
        return;
    };

    let display = row.label.clone().unwrap_or_else(|| row.path.clone());
    let title = truncate_width(&display, area.width.saturating_sub(18) as usize);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(Line::from(vec![
            Span::styled(
                " Source ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("· ", Style::new().fg(theme.border)),
            Span::styled(title, Style::new().fg(Color::Reset)),
        ]));
    let inner = block.inner(area);

    let mut lines: Vec<Line<'static>> = vec![
        Line::from(Span::styled(
            row.path.clone(),
            Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                format!("{} ", row.kind.badge()),
                Style::new()
                    .fg(theme.kind_color(row.kind))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} · {} asset(s)", row.kind.as_str(), row.assets),
                Style::new().fg(theme.muted),
            ),
        ]),
        Line::from(Span::styled(
            row.resolved.display().to_string(),
            Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            if row.exists {
                "exists"
            } else {
                "missing — check the path"
            },
            Style::new().fg(if row.exists {
                theme.success
            } else {
                theme.error
            }),
        )),
    ];
    if let Some(label) = &row.label {
        lines.push(Line::from(Span::styled(
            format!("label: {label}"),
            Style::new().fg(theme.muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "assets",
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    let mut assets: Vec<&str> = app
        .assets
        .iter()
        .filter(|asset| asset.source == display)
        .map(|asset| asset.name.as_str())
        .collect();
    assets.sort_unstable();
    if assets.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none",
            Style::new().fg(theme.muted),
        )));
    } else {
        for name in assets.iter().take(30) {
            lines.push(Line::from(Span::styled(
                format!("  {name}"),
                Style::new().fg(Color::Reset),
            )));
        }
        if assets.len() > 30 {
            lines.push(Line::from(Span::styled(
                format!("  … {} more", assets.len() - 30),
                Style::new().fg(theme.muted),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "A add source · R rescan · 0-5 back to assets",
        Style::new().fg(theme.border),
    )));

    app.preview_max_scroll = lines
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    if app.preview_scroll > app.preview_max_scroll {
        app.preview_scroll = app.preview_max_scroll;
    }
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.preview_scroll, 0)),
        area,
    );
}

fn asset_list(f: &mut Frame, area: Rect, app: &mut App, theme: &ResolvedTheme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border))
        .title(Line::from(vec![
            Span::styled(
                " Assets ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", app.filtered.len()),
                Style::new().fg(theme.muted),
            ),
        ]));

    let inner = block.inner(area);
    app.list_height = inner.height;
    app.ensure_visible();

    if app.filtered.is_empty() {
        let query = if app.query.is_empty() {
            String::new()
        } else {
            format!(" for `{}`", app.query)
        };
        let kind = match app.kind_filter {
            Some(kind) => format!(" in {}", kind.plural()),
            None => String::new(),
        };
        let text = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  No assets{query}{kind}."),
                Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled("  a  create", Style::new().fg(theme.muted))),
            Line::from(Span::styled(
                "  /  search    0-5  filter kinds",
                Style::new().fg(theme.muted),
            )),
            Line::from(Span::styled(
                "  g c  configuration    R  rescan",
                Style::new().fg(theme.muted),
            )),
            Line::from(Span::styled(
                "  ?  keymap help",
                Style::new().fg(theme.muted),
            )),
        ];
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let start = app.list_top.min(app.filtered.len());
    let visible = inner.height as usize;
    let end = (start + visible).min(app.filtered.len());
    let scrollbar = app.filtered.len() > visible;
    let content_area = Rect {
        width: inner.width.saturating_sub(u16::from(scrollbar)),
        ..inner
    };
    let inner_width = content_area.width as usize;
    let mut items = Vec::with_capacity(end - start);
    f.render_widget(block, area);

    for position in start..end {
        let asset = &app.assets[app.filtered[position]];
        let selected = position == app.selected;
        let badge = format!(" {} ", asset.kind.badge());
        let available = inner_width.saturating_sub(badge.width() + 4).max(8);
        let name_budget = (available / 2).max(10);
        let name = truncate_width(&asset.name, name_budget);
        let name_width = name.width();
        let description_budget = available.saturating_sub(name_width);
        let description = truncate_width(&asset.description, description_budget);

        let mut spans = vec![
            Span::styled(
                badge,
                Style::new()
                    .fg(theme.kind_color(asset.kind))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                name,
                Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
            ),
        ];
        let duplicates = app.duplicate_count(asset);
        if duplicates > 0 {
            spans.push(Span::styled(
                format!(" ×{}", duplicates + 1),
                Style::new().fg(theme.warning),
            ));
        }
        if !description.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(description, Style::new().fg(theme.muted)));
        }
        let mut item = ListItem::new(Line::from(spans));
        if selected {
            item = item.style(theme.selection);
        }
        items.push(item);
    }

    f.render_widget(List::new(items), content_area);

    if scrollbar {
        let mut state = ScrollbarState::new(app.filtered.len())
            .position(app.list_top)
            .viewport_content_length(visible);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::new().fg(theme.accent))
            .track_style(Style::new().fg(theme.border))
            .track_symbol(Some("│"));
        f.render_stateful_widget(bar, inner, &mut state);
    }
}

fn preview(f: &mut Frame, area: Rect, app: &mut App, theme: &ResolvedTheme) {
    let duplicates = app.duplicate_paths();
    let focused = app.mode == Mode::Normal && app.focus == Focus::Preview;
    let border_color = if focused { theme.accent } else { theme.border };
    let Some(asset) = app.current() else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(border_color))
            .title(Span::styled(" Preview ", Style::new().fg(theme.muted)));
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Select an asset with j/k",
                Style::new().fg(theme.muted),
            )))
            .block(block),
            area,
        );
        return;
    };

    let match_terms = if app.content_search {
        search::content_terms(&app.query)
    } else {
        Vec::new()
    };
    let match_lines = search::matching_lines(&asset.body, &match_terms);

    let title = truncate_width(&asset.name, area.width.saturating_sub(16) as usize);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .title(Line::from(vec![
            Span::styled(
                " Preview ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("· ", Style::new().fg(theme.border)),
            Span::styled(title, Style::new().fg(Color::Reset)),
        ]));
    let inner = block.inner(area);
    let width = inner.width as usize;

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(Span::styled(
        asset.name.clone(),
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", asset.kind.badge()),
            Style::new()
                .fg(theme.kind_color(asset.kind))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} · ", asset.kind.as_str()),
            Style::new().fg(theme.muted),
        ),
        Span::styled(
            asset.source.clone(),
            Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        asset.path.display().to_string(),
        Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "{} · {}",
            format_age(asset.modified),
            format_size(asset.size)
        ),
        Style::new().fg(theme.muted),
    )));

    if !asset.description.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            asset.description.clone(),
            Style::new().fg(theme.muted),
        )));
    }

    if !asset.tags.is_empty() {
        lines.push(Line::from(""));
        let mut tag_spans = vec![Span::styled("tags  ", Style::new().fg(theme.border))];
        for tag in &asset.tags {
            tag_spans.push(Span::styled(
                format!("#{tag} "),
                Style::new().fg(theme.accent),
            ));
        }
        lines.push(Line::from(tag_spans));
    }

    let mut metadata: Vec<(&String, &String)> = asset
        .fields
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "name" | "description" | "tags" | "keywords"))
        .collect();
    metadata.sort_by_key(|(key, _)| key.as_str());
    if !metadata.is_empty() {
        lines.push(Line::from(""));
        for (key, value) in metadata {
            lines.push(Line::from(vec![
                Span::styled(format!("{key}: "), Style::new().fg(theme.muted)),
                Span::raw(truncate_width(value, width.saturating_sub(key.width() + 2))),
            ]));
        }
    }

    if !duplicates.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "identical copy{} in {} other location{}:",
                if duplicates.len() == 1 { "" } else { "ies" },
                duplicates.len(),
                if duplicates.len() == 1 { "" } else { "s" }
            ),
            Style::new().fg(theme.warning),
        )));
        for path in duplicates.iter().take(4) {
            lines.push(Line::from(Span::styled(
                compact_path(path),
                Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
            )));
        }
    }

    if match_lines > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("content matches: {match_lines} line(s)"),
            Style::new().fg(theme.warning).add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(Span::styled(
        "─".repeat(width.max(1)),
        Style::new().fg(theme.border),
    )));

    let body_lines: Vec<&str> = asset.body.lines().collect();
    let first = body_lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(body_lines.len());
    for line in body_lines.iter().skip(first).take(4000) {
        lines.push(body_line(
            line,
            theme,
            search::line_matches(line, &match_terms),
        ));
    }

    let wrap_width = inner.width.max(1) as usize;
    let wrapped_rows: usize = lines
        .iter()
        .map(|line| {
            let line_width = line.width();
            if line_width == 0 {
                1
            } else {
                line_width.div_ceil(wrap_width)
            }
        })
        .sum();
    app.preview_max_scroll = (wrapped_rows + wrapped_rows / 4 + 2)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    if app.preview_scroll > app.preview_max_scroll {
        app.preview_scroll = app.preview_max_scroll;
    }

    let scrollbar = wrapped_rows > inner.height as usize;
    let content_area = Rect {
        width: inner.width.saturating_sub(u16::from(scrollbar)),
        ..inner
    };
    f.render_widget(block, area);
    let paragraph = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0));
    f.render_widget(paragraph, content_area);

    if scrollbar {
        let mut state = ScrollbarState::new(wrapped_rows)
            .position(app.preview_scroll as usize)
            .viewport_content_length(inner.height as usize);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::new().fg(theme.accent))
            .track_style(Style::new().fg(theme.border))
            .track_symbol(Some("│"));
        f.render_stateful_widget(bar, inner, &mut state);
    }
}

fn body_line<'a>(line: &str, theme: &ResolvedTheme, highlight: bool) -> Line<'a> {
    let trimmed = line.trim_start();
    let mut style = if trimmed.starts_with('#') {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else if trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
        || trimmed.starts_with("---")
        || trimmed.starts_with("***")
    {
        Style::new().fg(theme.border)
    } else if trimmed.starts_with('>') {
        Style::new().fg(theme.muted).add_modifier(Modifier::ITALIC)
    } else {
        Style::new().fg(Color::Reset)
    };
    if highlight {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    Line::from(Span::styled(line.to_string(), style))
}

fn status_bar(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let status_text = app
        .status
        .as_ref()
        .map(|status| (status.text.clone(), status.level))
        .or_else(|| {
            app.current()
                .map(|asset| (compact_path(&asset.path), Level::Info))
        })
        .unwrap_or_else(|| (String::new(), Level::Info));
    let status_width = (status_text.0.width() as u16 + 1).min(area.width / 2);

    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(status_width)]).areas(area);

    let chip_style = if theme.chip_bg == Color::Reset {
        Style::new()
            .fg(theme.chip_fg)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::new()
            .fg(theme.chip_fg)
            .bg(theme.chip_bg)
            .add_modifier(Modifier::BOLD)
    };
    let mode_label = if app.mode == Mode::Normal && app.focus == Focus::Preview {
        "PREVIEW"
    } else {
        app.mode.label()
    };
    let mut spans = vec![Span::styled(format!(" {mode_label} "), chip_style)];

    match app.mode {
        Mode::Command => {
            spans.push(Span::styled(
                " :",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
            spans.extend(input_spans(&app.input, theme));
            spans.push(Span::styled(
                format!("   {}/{}", app.filtered.len(), app.assets.len()),
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Search => {
            spans.push(Span::styled(
                " typing filters live",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Edit => {
            spans.push(Span::styled(
                " ctrl-s save · esc exit · ctrl-z undo · ctrl-r redo",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Form => {
            spans.push(Span::styled(
                " tab next · ←/→ change · enter create · esc cancel",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Install => {
            spans.push(Span::styled(
                " tab next · ←/→ change · enter install · esc cancel",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::InstallProgress => {
            spans.push(Span::styled(
                " cloning in the background · please wait",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Source => {
            spans.push(Span::styled(
                " tab next · ←/→ kind · enter add · esc cancel",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Keymap => {
            spans.push(Span::styled(
                " enter rebind · backspace unbind · r reset · esc close",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Confirm => {
            spans.push(Span::styled(
                " y delete · n cancel",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Help => {
            spans.push(Span::styled(
                " j/k scroll · esc close",
                Style::new().fg(theme.muted),
            ));
        }
        Mode::Normal => {
            if app.focus == Focus::Preview {
                for (keys, label) in [
                    ("j/k", "scroll"),
                    ("g g/G", "top/bottom"),
                    ("ctrl-h", "search"),
                    ("q", "quit"),
                ] {
                    spans.push(Span::styled(
                        format!("  {keys}"),
                        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        format!(" {label}"),
                        Style::new().fg(theme.muted),
                    ));
                }
            } else {
                for (keys, label) in [
                    ("/", "search"),
                    ("a", "new"),
                    ("i", "install"),
                    ("A", "source"),
                    ("6", "sources"),
                    ("enter", "edit"),
                    ("e", "external"),
                    ("d", "delete"),
                    ("?", "help"),
                ] {
                    spans.push(Span::styled(
                        format!("  {keys}"),
                        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        format!(" {label}"),
                        Style::new().fg(theme.muted),
                    ));
                }
            }
        }
    }

    f.render_widget(Paragraph::new(Line::from(spans)), left_area);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status_text.0,
            Style::new().fg(theme.status_color(status_text.1)),
        )))
        .alignment(Alignment::Right),
        right_area,
    );
}

fn form_modal(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let Some(form) = &app.form else {
        return;
    };
    let width = (area.width.saturating_sub(4)).min(76);
    let height = 10.min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(
            format!(" New {} ", form.kind.as_str()),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let label = Style::new().fg(theme.muted);
    let mut lines: Vec<Line<'static>> = Vec::new();

    let mut kind_spans = vec![Span::styled(format!("{:<12}", "kind"), label)];
    let kind_value = if form.focus == FormField::Kind {
        Span::styled(
            format!("‹ {} ›", form.kind.as_str()),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            form.kind.as_str().to_string(),
            Style::new().fg(Color::Reset),
        )
    };
    kind_spans.push(kind_value);
    lines.push(Line::from(kind_spans));

    lines.push(text_field_line(
        "name",
        &form.name,
        form.focus == FormField::Name,
        theme,
    ));
    lines.push(text_field_line(
        "description",
        &form.description,
        form.focus == FormField::Description,
        theme,
    ));

    let mut source_spans = vec![Span::styled(format!("{:<12}", "source"), label)];
    let source_value = form
        .source(&app.cfg)
        .map(|source| source.display())
        .unwrap_or_else(|| "— no source configured —".to_string());
    let source_width = inner.width.saturating_sub(13) as usize;
    let source_value = truncate_width(&source_value, source_width);
    if form.focus == FormField::Source {
        source_spans.push(Span::styled(
            format!("‹ {source_value} ›"),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    } else {
        source_spans.push(Span::styled(source_value, Style::new().fg(Color::Reset)));
    }
    lines.push(Line::from(source_spans));

    lines.push(Line::from(""));
    let error = form
        .error
        .clone()
        .unwrap_or_else(|| "enter creates the file; tab cycles fields".into());
    lines.push(Line::from(Span::styled(
        error,
        Style::new()
            .fg(if form.error.is_some() {
                theme.error
            } else {
                theme.muted
            })
            .add_modifier(if form.error.is_some() {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn install_progress_modal(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let Some(job) = &app.install_job else {
        return;
    };
    let width = (area.width.saturating_sub(4)).min(78);
    let height = 10.min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(
            " Installing from git ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let [target_area, gauge_area, detail_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let target = truncate_width(&job.target, inner.width as usize);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            target,
            Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
        ))),
        target_area,
    );

    let (ratio, phase) = match job.progress.phase {
        Phase::Clone => {
            let ratio = job
                .progress
                .percent
                .map(|percent| f64::from(percent) / 100.0 * 0.8)
                .unwrap_or(0.0);
            (ratio, "cloning")
        }
        Phase::Resolve => (0.82, "locating asset"),
        Phase::Copy => (0.92, "copying files"),
        Phase::Done => (1.0, "complete"),
    };
    let spinner = ["|", "/", "-", "\\"][job.spinner % 4];
    let label = if let Some(percent) = job.progress.percent {
        format!("{:>3.0}%  {phase} ({percent}% clone)", ratio * 100.0)
    } else {
        format!("{spinner}  {phase}")
    };
    let gauge = Gauge::default()
        .gauge_style(Style::new().fg(theme.accent))
        .style(Style::new().fg(theme.border))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label);
    f.render_widget(gauge, gauge_area);

    let detail = truncate_width(&job.progress.detail, inner.width as usize);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            detail,
            Style::new().fg(theme.muted),
        ))),
        detail_area,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "{}s elapsed · clone runs in the background; this closes when done",
                job.elapsed().as_secs()
            ),
            Style::new().fg(theme.border),
        ))),
        footer_area,
    );
}

fn install_modal(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let Some(form) = &app.install else {
        return;
    };
    let width = (area.width.saturating_sub(4)).min(78);
    let height = 11.min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(
            " Install from git ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let label = Style::new().fg(theme.muted);
    let field_width = inner.width.saturating_sub(11) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    let mut url_spans = vec![Span::styled(format!("{:<10}", "url"), label)];
    if form.focus == InstallField::Url {
        url_spans.extend(input_spans_clipped(&form.url, theme, field_width));
    } else if form.url.value.is_empty() {
        url_spans.push(Span::styled(
            "https://github.com/owner/repo or tree URL",
            Style::new().fg(theme.border),
        ));
    } else {
        url_spans.push(Span::styled(
            truncate_width(&form.url.value, field_width),
            Style::new().fg(Color::Reset),
        ));
    }
    lines.push(Line::from(url_spans));

    let mut kind_spans = vec![Span::styled(format!("{:<10}", "kind"), label)];
    if form.focus == InstallField::Kind {
        kind_spans.push(Span::styled(
            format!("‹ {} ›", form.kind.as_str()),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    } else {
        kind_spans.push(Span::styled(
            form.kind.as_str().to_string(),
            Style::new().fg(Color::Reset),
        ));
    }
    lines.push(Line::from(kind_spans));

    let source_value = form
        .source(&app.cfg)
        .map(|source| source.display())
        .unwrap_or_else(|| "— no source configured for this kind —".to_string());
    let source_value = truncate_width(&source_value, field_width);
    let mut source_spans = vec![Span::styled(format!("{:<10}", "source"), label)];
    if form.focus == InstallField::Source {
        source_spans.push(Span::styled(
            format!("‹ {source_value} ›"),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    } else {
        source_spans.push(Span::styled(source_value, Style::new().fg(Color::Reset)));
    }
    lines.push(Line::from(source_spans));

    let mut name_spans = vec![Span::styled(format!("{:<10}", "name"), label)];
    if form.focus == InstallField::Name {
        name_spans.extend(input_spans_clipped(&form.name, theme, field_width));
    } else if form.name.value.is_empty() {
        name_spans.push(Span::styled(
            "default: repository or directory name",
            Style::new().fg(theme.border),
        ));
    } else {
        name_spans.push(Span::styled(
            truncate_width(&form.name.value, field_width),
            Style::new().fg(Color::Reset),
        ));
    }
    lines.push(Line::from(name_spans));

    lines.push(Line::from(""));
    let message = form
        .error
        .clone()
        .unwrap_or_else(|| "clones to a temp dir, copies only the selected asset".into());
    lines.push(Line::from(Span::styled(
        message,
        Style::new()
            .fg(if form.error.is_some() {
                theme.error
            } else {
                theme.muted
            })
            .add_modifier(if form.error.is_some() {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn source_modal(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let Some(form) = &app.source_form else {
        return;
    };
    let width = (area.width.saturating_sub(4)).min(76);
    let height = 9.min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(
            " Add source folder ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let label = Style::new().fg(theme.muted);
    let field_width = inner.width.saturating_sub(10) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    let mut path_spans = vec![Span::styled(format!("{:<10}", "path"), label)];
    if form.focus == SourceField::Path {
        path_spans.extend(input_spans_clipped(&form.path, theme, field_width));
    } else if form.path.value.is_empty() {
        path_spans.push(Span::styled(
            "~/extra-skills or /absolute/path",
            Style::new().fg(theme.border),
        ));
    } else {
        path_spans.push(Span::styled(
            truncate_width(&form.path.value, field_width),
            Style::new().fg(Color::Reset),
        ));
    }
    lines.push(Line::from(path_spans));

    let mut kind_spans = vec![Span::styled(format!("{:<10}", "kind"), label)];
    if form.focus == SourceField::Kind {
        kind_spans.push(Span::styled(
            format!("‹ {} ›", form.kind.as_str()),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    } else {
        kind_spans.push(Span::styled(
            form.kind.as_str().to_string(),
            Style::new().fg(Color::Reset),
        ));
    }
    lines.push(Line::from(kind_spans));

    lines.push(Line::from(""));
    let message = form
        .error
        .clone()
        .unwrap_or_else(|| "enter adds the folder to config.sources.toml and rescans".into());
    lines.push(Line::from(Span::styled(
        message,
        Style::new()
            .fg(if form.error.is_some() {
                theme.error
            } else {
                theme.muted
            })
            .add_modifier(if form.error.is_some() {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )));

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn text_field_line(
    name: &str,
    input: &TextInput,
    focused: bool,
    theme: &ResolvedTheme,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{name:<12}"),
        Style::new().fg(theme.muted),
    )];
    if focused {
        spans.extend(input_spans(input, theme));
    } else {
        let value = if input.value.is_empty() {
            Span::styled("—", Style::new().fg(theme.border))
        } else {
            Span::styled(input.value.clone(), Style::new().fg(Color::Reset))
        };
        spans.push(value);
    }
    Line::from(spans)
}

fn input_spans(input: &TextInput, theme: &ResolvedTheme) -> Vec<Span<'static>> {
    let chars: Vec<char> = input.value.chars().collect();
    let cursor = input.cursor.min(chars.len());
    let before: String = chars[..cursor].iter().collect();
    let mut spans = vec![Span::styled(before, Style::new().fg(Color::Reset))];
    if cursor < chars.len() {
        spans.push(Span::styled(
            chars[cursor].to_string(),
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::REVERSED),
        ));
        let after: String = chars[cursor + 1..].iter().collect();
        spans.push(Span::styled(after, Style::new().fg(Color::Reset)));
    } else {
        spans.push(Span::styled(
            " ",
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::REVERSED),
        ));
    }
    spans
}

/// Render a text input within `max_width` characters, keeping the cursor
/// visible by sliding the window. Used for long URLs.
fn input_spans_clipped(
    input: &TextInput,
    theme: &ResolvedTheme,
    max_width: usize,
) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }
    let chars: Vec<char> = input.value.chars().collect();
    if chars.len() <= max_width {
        return input_spans(input, theme);
    }
    let cursor = input.cursor.min(chars.len());
    let start = cursor.saturating_sub(max_width.saturating_sub(1));
    let end = (start + max_width).min(chars.len());
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled("…", Style::new().fg(theme.border)));
    }
    for (offset, ch) in chars[start..end].iter().enumerate() {
        let position = start + offset;
        let style = if position == cursor {
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::new().fg(Color::Reset)
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    if cursor >= chars.len() {
        spans.push(Span::styled(
            " ",
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::REVERSED),
        ));
    }
    if end < chars.len() {
        spans.push(Span::styled("…", Style::new().fg(theme.border)));
    }
    spans
}

fn confirm_modal(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let Some(confirm) = &app.confirm else {
        return;
    };
    let width = (area.width.saturating_sub(4)).min(64);
    let height = 9.min(area.height.saturating_sub(2));
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.warning))
        .title(Span::styled(
            " Delete asset ",
            Style::new().fg(theme.warning).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let lines = vec![
        Line::from(Span::styled(
            format!("Delete `{}`?", confirm.label),
            Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            truncate_width(&confirm.path.display().to_string(), inner.width as usize),
            Style::new().fg(theme.muted),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " y ",
                Style::new().fg(theme.error).add_modifier(Modifier::BOLD),
            ),
            Span::styled("delete", Style::new().fg(theme.muted)),
            Span::styled(
                "    n ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled("cancel", Style::new().fg(theme.muted)),
        ]),
        Line::from(Span::styled(
            if app.cfg.behavior.trash {
                "moves the file to the arsenal trash directory"
            } else {
                "behavior.trash = false: this unlinks the file"
            },
            Style::new().fg(theme.muted),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn help_key_text(app: &App, action: Action) -> String {
    let configured = app.keys.display_or_dash(action);
    let alias = match action {
        Action::Up => Some("↑"),
        Action::Down => Some("↓"),
        Action::Top => Some("home"),
        Action::Bottom => Some("end"),
        Action::PageUp => Some("pgup"),
        Action::PageDown => Some("pgdn"),
        _ => None,
    };
    match alias {
        Some(alias) if configured == "—" => alias.to_string(),
        Some(alias) => format!("{configured} / {alias}"),
        None => configured,
    }
}

fn keymap_modal(f: &mut Frame, area: Rect, app: &App, theme: &ResolvedTheme) {
    let Some(editor) = &app.keymap_editor else {
        return;
    };
    let width = (area.width.saturating_sub(4)).min(84);
    let height = (area.height.saturating_sub(2)).min(24);
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Line::from(vec![
            Span::styled(
                " Key bindings ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{}/{} ",
                    editor.selected.min(Action::ALL.len().saturating_sub(1)) + 1,
                    Action::ALL.len()
                ),
                Style::new().fg(theme.muted),
            ),
        ]));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let visible = inner.height.saturating_sub(2).max(1) as usize;
    let start = editor
        .selected
        .saturating_sub(visible / 2)
        .min(Action::ALL.len().saturating_sub(visible));
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (offset, action) in Action::ALL.iter().enumerate().skip(start).take(visible) {
        let index = start + offset;
        let keys = app.keys.display_or_dash(*action);
        let style = if index == editor.selected {
            theme.selection
        } else {
            Style::new().fg(Color::Reset)
        };
        lines.push(Line::from(Span::styled(
            format!(" {:<18} {}", keys, action.label()),
            style,
        )));
    }

    if let Some(error) = &editor.error {
        lines.push(Line::from(Span::styled(
            format!(" {error}"),
            Style::new().fg(theme.error).add_modifier(Modifier::BOLD),
        )));
    } else if editor.capturing {
        let action = Action::ALL[editor.selected.min(Action::ALL.len() - 1)];
        lines.push(Line::from(Span::styled(
            format!(" Press a key for `{}` · esc cancels", action.label()),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            " enter rebind · backspace unbind · r reset · esc close",
            Style::new().fg(theme.muted),
        )));
    }

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn help_modal(f: &mut Frame, area: Rect, app: &mut App, theme: &ResolvedTheme) {
    let width = (area.width.saturating_sub(4)).min(78);
    let height = (area.height.saturating_sub(2)).min(30);
    let rect = centered(area, width, height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .title(Span::styled(
            " Keymap & commands · j/k scroll ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (title, actions) in HELP_GROUPS {
        lines.push(Line::from(Span::styled(
            format!(" {title}"),
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )));
        for action in actions.iter() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("   {:<18}", help_key_text(app, *action)),
                    Style::new().fg(Color::Reset),
                ),
                Span::styled(action.label(), Style::new().fg(theme.muted)),
            ]));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        " Commands",
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
    )));
    for command in [
        ":new <kind>        create a skill, agent, command, rule, or snippet",
        ":install <url>     clone a skill or asset from GitHub",
        ":source <path>     add an extra source folder",
        ":sources           show the sources tab",
        ":keys              edit key bindings",
        ":filter <kind|all> restrict the list",
        ":sort <mode>       kind | name | modified | path",
        ":theme <preset>    dark | light | mono",
        ":content <on|off>  include document bodies in search",
        ":edit  :delete  :yank  :reveal  :rescan  :config  :warnings  :quit",
        ":inline            edit the selected document in-app",
    ] {
        lines.push(Line::from(Span::styled(
            format!("   {command}"),
            Style::new().fg(theme.muted),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" config  ", Style::new().fg(theme.border)),
        Span::styled(
            app.config_path.display().to_string(),
            Style::new().fg(theme.muted),
        ),
    ]));
    if app.warnings.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(" warnings  ", Style::new().fg(theme.border)),
            Span::styled("none", Style::new().fg(theme.success)),
        ]));
    } else {
        for warning in app.warnings.iter().take(6) {
            lines.push(Line::from(vec![
                Span::styled(" warning  ", Style::new().fg(theme.warning)),
                Span::styled(
                    truncate_width(warning, inner.width.saturating_sub(14) as usize),
                    Style::new().fg(theme.muted),
                ),
            ]));
        }
        if app.warnings.len() > 6 {
            lines.push(Line::from(Span::styled(
                format!(" … {} more", app.warnings.len() - 6),
                Style::new().fg(theme.muted),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " esc close · j/k scroll",
        Style::new().fg(theme.border),
    )));

    let max_scroll = lines
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    if app.help_scroll == u16::MAX || app.help_scroll > max_scroll {
        app.help_scroll = max_scroll;
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((app.help_scroll, 0))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn truncate_width(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width + 1 > max {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

fn compact_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    let home = crate::config::home_dir().display().to_string();
    if !home.is_empty() && text.starts_with(&home) {
        format!("~{}", &text[home.len()..])
    } else {
        text
    }
}
