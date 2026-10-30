use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Gauge, Padding, Paragraph, Row, Table, Wrap};

use super::{App, Dialog, Focus, Sort};
use crate::category::Safety;
use crate::clean::Mode;
use crate::format;

const ACCENT: Color = Color::Cyan;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, details, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Length(42), Constraint::Min(30)]).areas(body);

    draw_disk(frame, header, app);
    draw_categories(frame, left, app);
    draw_items(frame, right, app);
    draw_details(frame, details, app);
    draw_footer(frame, footer, app);

    if app.cleaning.is_some() {
        draw_cleaning(frame, app);
    }

    match &app.dialog {
        Some(Dialog::Confirm) => draw_confirm(frame, app),
        Some(Dialog::Help) => draw_text_dialog(frame, " Help ", help_text(), 76, 28),
        Some(Dialog::Info(message)) => {
            let height = message.lines().count() as u16 + 4;
            draw_text_dialog(frame, " Done ", Text::from(message.as_str()), 70, height);
        }
        None => {}
    }
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner(app: &App) -> &'static str {
    let frame = app.started.elapsed().as_millis() / 80;
    SPINNER[frame as usize % SPINNER.len()]
}

fn draw_cleaning(frame: &mut Frame, app: &App) {
    let Some(progress) = &app.cleaning else {
        return;
    };

    let area = popup(frame.area(), 70, 9);
    let block = Block::bordered()
        .title(format!(
            " {} Cleaning {} of {} ",
            spinner(app),
            (progress.done + 1).min(progress.total),
            progress.total
        ))
        .border_style(Style::new().fg(ACCENT))
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(area);
    let [current, _, bar, _, elapsed] = Layout::vertical([Constraint::Length(1); 5]).areas(inner);

    let action = match app.mode {
        Mode::Trash => "Moving to the Trash:",
        Mode::Permanent => "Deleting:",
    };
    let label = progress
        .current
        .as_ref()
        .map_or("", |(label, _)| label.as_str());
    let bytes_done = progress.bytes_done();
    let ratio = bytes_done as f64 / progress.total_bytes.max(1) as f64;

    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(
        Line::from(vec![format!("{action} ").dim(), label.into()]),
        current,
    );
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(ACCENT).bg(Color::DarkGray))
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!(
                "{} of {}",
                format::size(bytes_done),
                format::size(progress.total_bytes)
            )),
        bar,
    );
    frame.render_widget(
        Line::from(format!("{}s elapsed", progress.started.elapsed().as_secs())).dim(),
        elapsed,
    );
}

fn draw_disk(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" mac-cleaner ".bold().fg(ACCENT));
    let Some(disk) = &app.disk else {
        frame.render_widget(Paragraph::new("Disk usage unknown").block(block), area);
        return;
    };

    let ratio = disk.used() as f64 / disk.total.max(1) as f64;
    let color = match ratio {
        r if r > 0.9 => Color::Red,
        r if r > 0.75 => Color::Yellow,
        _ => Color::Green,
    };
    let label = format!(
        "{} used of {} · {} free",
        format::size(disk.used()),
        format::size(disk.total),
        format::size(disk.free)
    );

    frame.render_widget(
        Gauge::default()
            .block(block)
            .gauge_style(Style::new().fg(color).bg(Color::DarkGray))
            .ratio(ratio.clamp(0.0, 1.0))
            .label(label.bold()),
        area,
    );
}

fn draw_categories(frame: &mut Frame, area: Rect, app: &mut App) {
    let rows = app.groups.iter().map(|group| {
        let selected = group
            .items
            .iter()
            .filter(|item| app.selected.contains(&item.path))
            .count();
        let marker = match selected {
            0 => Span::raw("  "),
            n if n == group.items.len() => Span::from("● ").fg(ACCENT),
            _ => Span::from("◐ ").fg(ACCENT),
        };
        let name_style = match group.category.safety() {
            Safety::Safe => Style::new(),
            Safety::Review => Style::new().fg(Color::Yellow),
        };
        let size = if group.scanning {
            Line::from(format!("{} scanning", spinner(app)))
                .dim()
                .right_aligned()
        } else if !group.issues.is_empty() && group.items.is_empty() {
            Line::from("no access").fg(Color::Red).right_aligned()
        } else {
            Line::from(format::size(group.total())).right_aligned()
        };

        Row::new([
            Cell::from(Line::from(vec![
                marker,
                Span::styled(group.category.name(), name_style),
            ])),
            Cell::from(size),
        ])
    });

    let total: u64 = app.groups.iter().map(super::Group::total).sum();
    let scanning = app.groups.iter().filter(|g| g.scanning).count();
    let title = if scanning > 0 {
        format!(" {} Categories · scanning {scanning} ", spinner(app))
    } else {
        format!(" Categories · {} ", format::size(total))
    };

    let table = Table::new(rows, [Constraint::Min(20), Constraint::Length(11)])
        .block(focus_block(title, app.focus == Focus::Categories))
        .row_highlight_style(highlight(app.focus == Focus::Categories));

    frame.render_stateful_widget(table, area, &mut app.category_state);
}

fn draw_items(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Items;
    let Some(group) = app
        .category_state
        .selected()
        .and_then(|i| app.groups.get(i))
    else {
        return;
    };

    let title = format!(
        " {} · {} · {} ",
        group.category.name(),
        format::count(group.items.len(), "item"),
        format::size(group.total())
    );
    let block = focus_block(title, focused);

    if group.items.is_empty() {
        let message = if group.scanning {
            "Scanning…"
        } else if !group.issues.is_empty() {
            "Could not read this folder. See the details below."
        } else {
            "Nothing to clean here."
        };
        frame.render_widget(Paragraph::new(message.dim()).block(block), area);
        return;
    }

    let sort_mark = |sort: Sort, name: &'static str| {
        if app.sort == sort {
            Span::from(format!("{name} ▾")).fg(ACCENT)
        } else {
            Span::raw(name)
        }
    };
    let header = Row::new([
        Cell::from(""),
        Cell::from(sort_mark(Sort::Name, "Name")),
        Cell::from(Line::from(sort_mark(Sort::Size, "Size")).right_aligned()),
        Cell::from(sort_mark(Sort::Age, "Changed")),
    ])
    .bold();

    let rows = group.items.iter().map(|item| {
        let checked = app.selected.contains(&item.path);
        let check = if checked {
            "[x]".fg(ACCENT)
        } else {
            "[ ]".dim()
        };
        let row = Row::new([
            Cell::from(check),
            Cell::from(item.label.as_str()),
            Cell::from(Line::from(format::size(item.size)).right_aligned()),
            Cell::from(format::age(item.modified)),
        ]);
        if checked { row.fg(ACCENT) } else { row }
    });

    let widths = [
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(10),
        Constraint::Length(9),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(highlight(focused));

    frame.render_stateful_widget(table, area, &mut app.item_state);
}

fn draw_details(frame: &mut Frame, area: Rect, app: &App) {
    let Some(group) = app.current_group() else {
        return;
    };

    let mut lines = vec![Line::from(group.category.description())];
    if group.category.safety() == Safety::Review {
        lines.push(
            Line::from("Review what you select: some of this cannot be downloaded again.")
                .fg(Color::Yellow),
        );
    }
    for issue in &group.issues {
        lines.push(Line::from(issue.as_str()).fg(Color::Red));
    }
    if app.focus == Focus::Items
        && let Some(item) = app.current_item()
    {
        lines.push(Line::from(item.path.display().to_string()).dim());
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" Details ")),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let (count, size) = app
        .selected_items()
        .fold((0, 0), |(count, size), item| (count + 1, size + item.size));
    let mode = match app.mode {
        Mode::Trash => " TRASH ".black().on_green(),
        Mode::Permanent => " PERMANENT ".white().on_red(),
    };

    let line = Line::from(vec![
        format!(" {count} selected · {} ", format::size(size)).bold(),
        mode,
        "  space select · a all · d delete · p mode · s sort · o finder · r rescan · ? help · q quit".dim(),
    ]);
    frame.render_widget(line, area);
}

fn draw_confirm(frame: &mut Frame, app: &App) {
    let items: Vec<_> = app.selected_items().collect();
    let size: u64 = items.iter().map(|item| item.size).sum();
    let permanent = items
        .iter()
        .filter(|item| item.category.always_permanent())
        .count();

    let mut lines = match app.mode {
        Mode::Trash => vec![Line::from(format!(
            "Move {} ({}) to the Trash?",
            format::count(items.len(), "item"),
            format::size(size)
        ))],
        Mode::Permanent => vec![
            Line::from(format!(
                "Delete {} ({}) for good?",
                format::count(items.len(), "item"),
                format::size(size)
            )),
            Line::from("This cannot be undone.").fg(Color::Red).bold(),
        ],
    };
    if app.mode == Mode::Trash && permanent > 0 {
        lines.push(
            Line::from(format!(
                "{} cannot go to the Trash and are deleted for good.",
                format::count(permanent, "item")
            ))
            .fg(Color::Yellow),
        );
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        "y".bold().fg(ACCENT),
        " yes   ".into(),
        "n".bold().fg(ACCENT),
        " no".into(),
    ]));

    let height = lines.len() as u16 + 2;
    draw_text_dialog(frame, " Confirm ", Text::from(lines), 60, height);
}

fn draw_text_dialog(frame: &mut Frame, title: &str, text: Text, width: u16, height: u16) {
    let area = popup(frame.area(), width, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .title(title)
                .border_style(Style::new().fg(ACCENT)),
        ),
        area,
    );
}

fn help_text() -> Text<'static> {
    let keys = [
        ("↑ ↓ / j k", "move"),
        ("← → / tab / enter", "switch between categories and items"),
        ("space", "select item (or whole category)"),
        ("a", "select or clear all items in this category"),
        ("c", "clear selection"),
        ("d / delete", "clean selected items"),
        ("p", "switch between Trash and permanent delete"),
        ("s", "sort by size, age or name"),
        ("o", "show item in Finder"),
        ("r", "scan again"),
        ("q / esc", "quit"),
    ];

    let mut lines: Vec<Line> = keys
        .iter()
        .map(|(key, action)| Line::from(vec![format!("{key:<20}").fg(ACCENT), (*action).into()]))
        .collect();

    lines.extend([
        Line::from(""),
        Line::from("What is \"System Data\"?").bold(),
        Line::from(
            "Everything macOS cannot put in another group: caches, logs, temp files, \
             Time Machine snapshots, iPhone backups, simulators, Docker disk images and \
             app data in ~/Library. This app cleans the parts you can safely remove.",
        ),
        Line::from(""),
        Line::from("Yellow categories need a review: deleted items may not come back.")
            .fg(Color::Yellow),
    ]);

    Text::from(lines)
}

fn focus_block(title: String, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    Block::bordered().title(title).border_style(style)
}

fn highlight(focused: bool) -> Style {
    if focused {
        Style::new()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}
