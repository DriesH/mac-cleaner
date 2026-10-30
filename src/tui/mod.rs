mod ui;

use std::collections::HashSet;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use rayon::prelude::*;

use crate::category::Category;
use crate::clean::{Cleaner, Mode};
use crate::disk::{self, DiskUsage};
use crate::format;
use crate::scan::{CategoryScan, Item, ScanConfig, scan_category};

pub fn run(config: ScanConfig, mode: Mode) -> Result<()> {
    let mut app = App::new(config, mode);
    app.start_scan();
    ratatui::run(|terminal| app.run(terminal))
}

enum Message {
    Scanned(CategoryScan),
    Started {
        label: String,
        size: u64,
    },
    Removed {
        path: PathBuf,
        size: u64,
        error: Option<String>,
    },
    CleanFinished,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Categories,
    Items,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Size,
    Age,
    Name,
}

enum Dialog {
    Confirm,
    Help,
    Info(String),
}

struct Group {
    category: Category,
    items: Vec<Item>,
    issues: Vec<String>,
    scanning: bool,
}

impl Group {
    fn total(&self) -> u64 {
        self.items.iter().map(|item| item.size).sum()
    }
}

struct CleanProgress {
    total: usize,
    done: usize,
    total_bytes: u64,
    /// Bytes of finished items, also the failed ones, so the bar always reaches the end.
    done_bytes: u64,
    freed: u64,
    current: Option<(String, u64)>,
    /// Bytes freed so far in the current item. Only grows for permanent deletes.
    live: Arc<AtomicU64>,
    started: Instant,
    errors: Vec<String>,
}

impl CleanProgress {
    fn bytes_done(&self) -> u64 {
        let live = self
            .current
            .as_ref()
            .map_or(0, |(_, size)| self.live.load(Ordering::Relaxed).min(*size));
        self.done_bytes + live
    }
}

struct App {
    config: Arc<ScanConfig>,
    mode: Mode,
    groups: Vec<Group>,
    category_state: TableState,
    item_state: TableState,
    focus: Focus,
    sort: Sort,
    selected: HashSet<PathBuf>,
    dialog: Option<Dialog>,
    cleaning: Option<CleanProgress>,
    disk: Option<DiskUsage>,
    /// Drives the spinner animation.
    started: Instant,
    tx: Sender<Message>,
    rx: Receiver<Message>,
    quit: bool,
}

impl App {
    fn new(config: ScanConfig, mode: Mode) -> Self {
        let (tx, rx) = mpsc::channel();

        Self {
            config: Arc::new(config),
            mode,
            groups: Vec::new(),
            category_state: TableState::default().with_selected(0),
            item_state: TableState::default(),
            focus: Focus::Categories,
            sort: Sort::Size,
            selected: HashSet::new(),
            dialog: None,
            cleaning: None,
            disk: disk::usage(),
            started: Instant::now(),
            tx,
            rx,
            quit: false,
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            terminal.draw(|frame| ui::draw(frame, self))?;
            self.handle_messages();

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
            }
        }

        Ok(())
    }

    fn start_scan(&mut self) {
        self.reset_groups();

        let config = Arc::clone(&self.config);
        let tx = self.tx.clone();
        thread::spawn(move || {
            Category::ALL.par_iter().for_each(|&category| {
                let _ = tx.send(Message::Scanned(scan_category(category, &config)));
            });
        });
    }

    fn reset_groups(&mut self) {
        self.selected.clear();
        self.item_state.select(None);
        self.groups = Category::ALL
            .iter()
            .map(|&category| Group {
                category,
                items: Vec::new(),
                issues: Vec::new(),
                scanning: true,
            })
            .collect();
    }

    fn start_clean(&mut self) {
        let items: Vec<Item> = self
            .groups
            .iter()
            .flat_map(|group| &group.items)
            .filter(|item| self.selected.contains(&item.path))
            .cloned()
            .collect();

        let live = Arc::new(AtomicU64::new(0));
        self.cleaning = Some(CleanProgress {
            total: items.len(),
            done: 0,
            total_bytes: items.iter().map(|item| item.size).sum(),
            done_bytes: 0,
            freed: 0,
            current: None,
            live: Arc::clone(&live),
            started: Instant::now(),
            errors: Vec::new(),
        });

        let config = Arc::clone(&self.config);
        let tx = self.tx.clone();
        let mode = self.mode;
        thread::spawn(move || {
            let cleaner = Cleaner::new(&config);
            for item in items {
                live.store(0, Ordering::Relaxed);
                let _ = tx.send(Message::Started {
                    label: item.label.clone(),
                    size: item.size,
                });
                let error = cleaner
                    .remove(&item, mode, &live)
                    .err()
                    .map(|e| format!("{e:#}"));
                let _ = tx.send(Message::Removed {
                    path: item.path,
                    size: item.size,
                    error,
                });
            }
            let _ = tx.send(Message::CleanFinished);
        });
    }

    fn handle_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Scanned(scan) => {
                    let sort = self.sort;
                    if let Some(group) =
                        self.groups.iter_mut().find(|g| g.category == scan.category)
                    {
                        group.items = scan.items;
                        group.issues = scan.issues;
                        group.scanning = false;
                        sort_items(&mut group.items, sort);
                    }
                }
                Message::Started { label, size } => {
                    if let Some(progress) = self.cleaning.as_mut() {
                        progress.current = Some((label, size));
                    }
                }
                Message::Removed { path, size, error } => {
                    let Some(progress) = self.cleaning.as_mut() else {
                        continue;
                    };
                    progress.done += 1;
                    progress.done_bytes += size;
                    progress.current = None;

                    match error {
                        Some(error) => progress.errors.push(error),
                        None => {
                            progress.freed += size;
                            self.selected.remove(&path);
                            for group in &mut self.groups {
                                group.items.retain(|item| item.path != path);
                            }
                        }
                    }
                }
                Message::CleanFinished => self.finish_clean(),
            }
        }
        self.clamp_item_selection();
    }

    fn finish_clean(&mut self) {
        let Some(progress) = self.cleaning.take() else {
            return;
        };
        self.disk = disk::usage();

        let mut summary = format!("Cleaned {}.", format::size(progress.freed));
        if self.mode == Mode::Trash {
            summary.push_str("\nThe files are in the Trash. Empty it to get the space back.");
        }
        if !progress.errors.is_empty() {
            summary.push_str(&format!(
                "\n\n{} failed:",
                format::count(progress.errors.len(), "item")
            ));
            for error in progress.errors.iter().take(5) {
                summary.push_str(&format!("\n• {error}"));
            }
        }

        self.dialog = Some(Dialog::Info(summary));
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.cleaning.is_some() {
            return;
        }

        if let Some(dialog) = &self.dialog {
            if matches!(dialog, Dialog::Confirm)
                && matches!(key.code, KeyCode::Char('y') | KeyCode::Enter)
            {
                self.start_clean();
            }
            self.dialog = None;
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.focus == Focus::Items => self.focus = Focus::Categories,
            KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::Home | KeyCode::Char('g') => self.move_cursor(isize::MIN),
            KeyCode::End | KeyCode::Char('G') => self.move_cursor(isize::MAX),
            KeyCode::Tab => self.set_focus(match self.focus {
                Focus::Categories => Focus::Items,
                Focus::Items => Focus::Categories,
            }),
            KeyCode::Left | KeyCode::Char('h') => self.set_focus(Focus::Categories),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.set_focus(Focus::Items),
            KeyCode::Char(' ') => match self.focus {
                Focus::Categories => self.toggle_group(),
                Focus::Items => self.toggle_item(),
            },
            KeyCode::Char('a') => self.toggle_group(),
            KeyCode::Char('c') => self.selected.clear(),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace
                if !self.selected.is_empty() =>
            {
                self.dialog = Some(Dialog::Confirm);
            }
            KeyCode::Char('p') => self.mode = self.mode.toggle(),
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('o') => self.reveal_in_finder(),
            KeyCode::Char('r') => self.start_scan(),
            KeyCode::Char('?') => self.dialog = Some(Dialog::Help),
            _ => {}
        }
    }

    fn current_group(&self) -> Option<&Group> {
        self.groups.get(self.category_state.selected()?)
    }

    fn current_item(&self) -> Option<&Item> {
        self.current_group()?.items.get(self.item_state.selected()?)
    }

    fn set_focus(&mut self, focus: Focus) {
        if focus == Focus::Items && self.current_group().is_none_or(|g| g.items.is_empty()) {
            return;
        }
        self.focus = focus;
        if focus == Focus::Items && self.item_state.selected().is_none() {
            self.item_state.select(Some(0));
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let (state, len) = match self.focus {
            Focus::Categories => (&mut self.category_state, self.groups.len()),
            Focus::Items => {
                let len = self
                    .category_state
                    .selected()
                    .and_then(|i| self.groups.get(i))
                    .map_or(0, |g| g.items.len());
                (&mut self.item_state, len)
            }
        };
        if len == 0 {
            return;
        }

        let current = state.selected().unwrap_or(0);
        let next = current.saturating_add_signed(delta).min(len - 1);
        state.select(Some(next));

        if self.focus == Focus::Categories {
            self.item_state = TableState::default();
        }
    }

    fn clamp_item_selection(&mut self) {
        let len = self.current_group().map_or(0, |g| g.items.len());
        match self.item_state.selected() {
            _ if len == 0 => {
                self.item_state.select(None);
                self.focus = Focus::Categories;
            }
            Some(i) if i >= len => self.item_state.select(Some(len - 1)),
            _ => {}
        }
    }

    fn toggle_item(&mut self) {
        let Some(path) = self.current_item().map(|item| item.path.clone()) else {
            return;
        };
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
        self.move_cursor(1);
    }

    /// Selects every item in the category, or clears them when all are selected already.
    fn toggle_group(&mut self) {
        let Some(group) = self.current_group() else {
            return;
        };
        let paths: Vec<PathBuf> = group.items.iter().map(|item| item.path.clone()).collect();

        if paths.iter().all(|path| self.selected.contains(path)) {
            for path in &paths {
                self.selected.remove(path);
            }
        } else {
            self.selected.extend(paths);
        }
    }

    fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            Sort::Size => Sort::Age,
            Sort::Age => Sort::Name,
            Sort::Name => Sort::Size,
        };
        for group in &mut self.groups {
            sort_items(&mut group.items, self.sort);
        }
    }

    fn reveal_in_finder(&self) {
        if let Some(item) = self.current_item() {
            let _ = process::Command::new("open")
                .arg("-R")
                .arg(&item.path)
                .spawn();
        }
    }

    fn selected_items(&self) -> impl Iterator<Item = &Item> {
        self.groups
            .iter()
            .flat_map(|group| &group.items)
            .filter(|item| self.selected.contains(&item.path))
    }
}

fn sort_items(items: &mut [Item], sort: Sort) {
    match sort {
        Sort::Size => items.sort_by_key(|item| std::cmp::Reverse(item.size)),
        Sort::Age => items.sort_by_key(|item| item.modified),
        Sort::Name => items.sort_by_key(|item| item.label.to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn app_with_caches() -> App {
        let config = ScanConfig {
            home: PathBuf::from("/Users/me"),
            temp: PathBuf::from("/tmp/me"),
            shared_temp: PathBuf::from("/tmp"),
            project_dirs: Vec::new(),
            downloads_days: 30,
        };
        let mut app = App::new(config, Mode::Trash);
        app.reset_groups();
        app.tx
            .send(Message::Scanned(CategoryScan {
                category: Category::UserCaches,
                items: ["com.spotify.client", "Homebrew"]
                    .iter()
                    .zip([3_000_000_000, 1_000_000_000])
                    .map(|(name, size)| Item {
                        category: Category::UserCaches,
                        path: PathBuf::from(format!("/Users/me/Library/Caches/{name}")),
                        label: (*name).to_string(),
                        size,
                        modified: None,
                    })
                    .collect(),
                issues: Vec::new(),
            }))
            .unwrap();
        app.handle_messages();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn selecting_category_should_select_all_its_items() {
        let mut app = app_with_caches();
        press(&mut app, KeyCode::Char(' '));

        assert!(render(&mut app).contains("2 selected · 4.0 GB"));
    }

    #[test]
    fn delete_key_should_ask_for_confirmation() {
        let mut app = app_with_caches();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('d'));

        assert!(render(&mut app).contains("Move 1 item (3.0 GB) to the Trash?"));
    }

    #[test]
    fn cleaning_dialog_should_show_current_item_and_live_bytes() {
        let mut app = app_with_caches();
        let live = Arc::new(AtomicU64::new(1_500_000_000));
        app.cleaning = Some(CleanProgress {
            total: 2,
            done: 0,
            total_bytes: 4_000_000_000,
            done_bytes: 0,
            freed: 0,
            current: Some(("com.spotify.client".to_string(), 3_000_000_000)),
            live,
            started: Instant::now(),
            errors: Vec::new(),
        });

        let screen = render(&mut app);

        assert!(screen.contains("com.spotify.client") && screen.contains("1.5 GB of 4.0 GB"));
    }

    #[test]
    #[ignore = "prints the screen for a manual check"]
    fn print_screen() {
        let mut app = app_with_caches();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char(' '));
        println!("{}", render(&mut app));
        press(&mut app, KeyCode::Char('p'));
        app.cleaning = Some(CleanProgress {
            total: 19,
            done: 1,
            total_bytes: 30_200_000_000,
            done_bytes: 14_000_000_000,
            freed: 14_000_000_000,
            current: Some((".cache/uv".to_string(), 9_000_000_000)),
            live: Arc::new(AtomicU64::new(3_100_000_000)),
            started: Instant::now(),
            errors: Vec::new(),
        });
        println!("{}", render(&mut app));
    }
}
