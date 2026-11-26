use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{
    Align, Align2, Color32, FontFamily, FontId, Label, Layout, Pos2, RichText, Sense, Shape, Stroke,
    UiBuilder, Vec2,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::lowimpact;
use crate::settings::{self, Settings};
use crate::worker::{Command, FileSnapshot, Phase, Shared, TorrentSnapshot};

// Классическая чёрная тема в духе qBittorrent: чёрный фон, тонкие линии,
// таблицы и нижние вкладки. Никакой рекламы.
const BG: Color32 = Color32::from_rgb(0, 0, 0);
const PANEL: Color32 = Color32::from_rgb(10, 10, 11);
const HEAD: Color32 = Color32::from_rgb(18, 18, 20);
const ROW: Color32 = Color32::from_rgb(0, 0, 0);
const ROW_HOVER: Color32 = Color32::from_rgb(22, 22, 25);
const ROW_SELECTED: Color32 = Color32::from_rgb(34, 40, 50);
const LINE: Color32 = Color32::from_rgb(48, 48, 52);
const LINE_DIM: Color32 = Color32::from_rgb(30, 30, 34);
const TEXT: Color32 = Color32::from_rgb(220, 220, 224);
const MUTED: Color32 = Color32::from_rgb(138, 138, 146);
const ACCENT: Color32 = Color32::from_rgb(72, 132, 224);
const GREEN: Color32 = Color32::from_rgb(92, 200, 120);
const RED: Color32 = Color32::from_rgb(230, 96, 96);
const YELLOW: Color32 = Color32::from_rgb(224, 190, 90);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Downloading,
    Seeding,
    Completed,
    Paused,
    Errors,
}

impl Filter {
    const ALL: [Filter; 6] = [
        Filter::All,
        Filter::Downloading,
        Filter::Seeding,
        Filter::Completed,
        Filter::Paused,
        Filter::Errors,
    ];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "Все",
            Filter::Downloading => "Качаются",
            Filter::Seeding => "Раздаются",
            Filter::Completed => "Завершённые",
            Filter::Paused => "На паузе",
            Filter::Errors => "С ошибкой",
        }
    }

    fn matches(self, t: &TorrentSnapshot) -> bool {
        match self {
            Filter::All => true,
            Filter::Downloading => !t.finished && t.phase == Phase::Live,
            Filter::Seeding => t.finished && t.phase == Phase::Live,
            Filter::Completed => t.finished,
            Filter::Paused => t.phase == Phase::Paused,
            Filter::Errors => t.error.is_some() || t.phase == Phase::Error,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Number,
    Name,
    Size,
    Progress,
    Down,
    Up,
    Eta,
    Peers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailsTab {
    Files,
    General,
    Trackers,
    Speed,
}

impl DetailsTab {
    const ALL: [DetailsTab; 4] = [
        DetailsTab::Files,
        DetailsTab::General,
        DetailsTab::Trackers,
        DetailsTab::Speed,
    ];

    fn label(self) -> &'static str {
        match self {
            DetailsTab::Files => "Файлы",
            DetailsTab::General => "Общее",
            DetailsTab::Trackers => "Трекеры",
            DetailsTab::Speed => "Скорость",
        }
    }
}

enum UiAction {
    Select(usize),
    Pause(usize),
    Resume(usize),
    Delete(usize, String),
    SetPriority(usize, u8),
    SetFiles(usize, Vec<usize>),
    OpenFolder(String),
    Copy(String),
    Error(String),
}

pub struct App {
    shared: Shared,
    tx: UnboundedSender<Command>,
    settings: Settings,
    draft: Settings,
    show_settings: bool,
    show_add: bool,
    show_about: bool,
    magnet_input: String,
    search: String,
    search_open: bool,
    torrents: Vec<TorrentSnapshot>,
    last_version: u64,
    filter: Filter,
    selected: Option<usize>,
    auto_selected: bool,
    sort_by: SortBy,
    sort_asc: bool,
    confirm_delete: Option<(usize, String)>,
    delete_files: bool,
    details_tab: DetailsTab,
    speed_hist: VecDeque<(u64, u64)>,
    speed_hist_id: Option<usize>,
    local_event: Option<(String, bool, Instant)>,
    shared_event: Option<(String, bool, Instant)>,
    pending_adds: usize,
    session_ready: bool,
    session_error: Option<String>,
    listen_port: Option<u16>,
    dht_nodes: usize,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        tx: UnboundedSender<Command>,
        shared: Shared,
    ) -> Self {
        setup_style(&cc.egui_ctx);
        Self {
            shared,
            tx,
            draft: settings.clone(),
            settings,
            show_settings: false,
            show_add: false,
            show_about: false,
            magnet_input: String::new(),
            search: String::new(),
            search_open: false,
            torrents: Vec::new(),
            last_version: u64::MAX,
            filter: Filter::All,
            selected: None,
            auto_selected: false,
            sort_by: SortBy::Number,
            sort_asc: true,
            confirm_delete: None,
            delete_files: false,
            details_tab: DetailsTab::Files,
            speed_hist: VecDeque::new(),
            speed_hist_id: None,
            local_event: None,
            shared_event: None,
            pending_adds: 0,
            session_ready: false,
            session_error: None,
            listen_port: None,
            dht_nodes: 0,
        }
    }

    fn event(&mut self, text: impl Into<String>, is_error: bool) {
        self.local_event = Some((text.into(), is_error, Instant::now()));
    }

    fn add_magnet(&mut self) {
        let text = self.magnet_input.trim().to_string();
        if text.is_empty() {
            self.event("Вставьте magnet-ссылку, ссылку или info-hash", true);
            return;
        }
        let links: Vec<String> = text
            .lines()
            .flat_map(|line| line.split_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        for link in links {
            let link = normalize_link(&link);
            let _ = self.tx.send(Command::AddUrl(link));
        }
        self.magnet_input.clear();
        self.show_add = false;
    }

    fn pick_torrent_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("BitTorrent", &["torrent"])
            .set_title("Выберите .torrent файл")
            .pick_file()
        {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "torrent".to_string());
                    let _ = self.tx.send(Command::AddFile { name, bytes });
                }
                Err(e) => self.event(format!("Не удалось прочитать файл: {e}"), true),
            }
        }
    }

    fn apply_settings(&mut self) {
        self.draft.sanitize();
        if let Err(e) = self.draft.save() {
            self.event(format!("Не удалось сохранить настройки: {e}"), true);
            return;
        }
        self.settings = self.draft.clone();
        lowimpact::apply(lowimpact::desired(self.settings.low_impact, false));
        let _ = self.tx.send(Command::UpdateSettings(self.settings.clone()));
        self.event("Настройки сохранены", false);
    }

    /// Держит приоритет процесса в соответствии с настройкой и состоянием окна:
    /// в фоне клиент уходит на самый низкий приоритет и не мешает звуку/играм.
    fn sync_priority(&self, ctx: &egui::Context) {
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        lowimpact::apply(lowimpact::desired(self.settings.low_impact, minimized));
    }

    fn handle_action(&mut self, action: UiAction, ctx: &egui::Context) {
        match action {
            UiAction::Select(id) => {
                self.selected = Some(id);
            }
            UiAction::Pause(id) => {
                if let Some(t) = self.torrents.iter_mut().find(|t| t.id == id) {
                    t.phase = Phase::Paused;
                    t.download_bps = 0;
                    t.upload_bps = 0;
                }
                let _ = self.tx.send(Command::Pause(id));
            }
            UiAction::Resume(id) => {
                if let Some(t) = self.torrents.iter_mut().find(|t| t.id == id) {
                    if t.phase == Phase::Paused {
                        t.phase = Phase::Live;
                    }
                }
                let _ = self.tx.send(Command::Resume(id));
            }
            UiAction::Delete(id, name) => {
                self.confirm_delete = Some((id, name));
                self.delete_files = false;
            }
            UiAction::SetPriority(id, priority) => {
                if let Some(t) = self.torrents.iter_mut().find(|t| t.id == id) {
                    t.priority = priority;
                }
                let _ = self.tx.send(Command::SetPriority { id, priority });
            }
            UiAction::SetFiles(id, files) => {
                if let Some(t) = self.torrents.iter_mut().find(|t| t.id == id) {
                    t.selected_files = Some(files.clone());
                }
                let _ = self.tx.send(Command::SetFiles { id, files });
            }
            UiAction::OpenFolder(path) => {
                let _ = std::fs::create_dir_all(&path);
                let _ = std::process::Command::new("explorer").arg(path).spawn();
            }
            UiAction::Copy(text) => ctx.copy_text(text),
            UiAction::Error(text) => self.event(text, true),
        }
    }

    fn drain_shared(&mut self) {
        let (toasts, version) = {
            let mut g = match self.shared.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            let toasts = std::mem::take(&mut g.toasts);
            self.pending_adds = g.pending_adds;
            self.session_ready = g.session_ready;
            self.session_error = g.session_error.clone();
            self.listen_port = g.listen_port;
            self.dht_nodes = g.dht_nodes;
            self.shared_event = g.event.clone();
            (toasts, g.version)
        };

        for toast in toasts {
            if self.settings.notify_on_finish {
                let _ = settings::notify_finished(
                    &toast.name,
                    &toast.folder,
                    self.settings.notify_sound,
                );
            }
            self.local_event = Some((
                format!("Загрузка завершена: {}", toast.name),
                false,
                Instant::now(),
            ));
        }

        if version != self.last_version {
            self.last_version = version;
            if let Ok(g) = self.shared.lock() {
                self.torrents = g.torrents.clone();
            }
            // Первый появившийся торрент выбираем один раз, чтобы нижняя панель
            // не оставалась пустой. Дальше выбор — только кликом пользователя.
            if !self.auto_selected && self.selected.is_none() {
                if let Some(first) = self.torrents.first() {
                    self.selected = Some(first.id);
                    self.auto_selected = true;
                }
            }
        }
    }

    fn current_event(&self) -> Option<(String, bool)> {
        let local = self
            .local_event
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < Duration::from_secs(12));
        let shared = self
            .shared_event
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < Duration::from_secs(12));
        match (local, shared) {
            (Some(a), Some(b)) => {
                if a.2 >= b.2 {
                    Some((a.0.clone(), a.1))
                } else {
                    Some((b.0.clone(), b.1))
                }
            }
            (Some(a), None) => Some((a.0.clone(), a.1)),
            (None, Some(b)) => Some((b.0.clone(), b.1)),
            (None, None) => None,
        }
    }

    fn total_speeds(&self) -> (u64, u64) {
        self.torrents
            .iter()
            .fold((0u64, 0u64), |(d, u), t| (d + t.download_bps, u + t.upload_bps))
    }

    fn total_traffic(&self) -> (u64, u64) {
        self.torrents.iter().fold((0u64, 0u64), |(d, u), t| {
            (d + t.progress_bytes, u + t.uploaded_bytes)
        })
    }

    fn selected_torrent(&self) -> Option<TorrentSnapshot> {
        let id = self.selected?;
        self.torrents.iter().find(|t| t.id == id).cloned()
    }

    fn push_speed_history(&mut self) {
        let id = self.selected;
        if id != self.speed_hist_id {
            self.speed_hist_id = id;
            self.speed_hist.clear();
        }
        let (d, u) = match id.and_then(|id| self.torrents.iter().find(|t| t.id == id)) {
            Some(t) => (t.download_bps, t.upload_bps),
            None => self.total_speeds(),
        };
        self.speed_hist.push_back((d, u));
        while self.speed_hist.len() > 240 {
            self.speed_hist.pop_front();
        }
    }

    // ---------------------------------------------------------------- menu

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(6, 1)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button(RichText::new("Файл").size(11.5), |ui| {
                        if ui.button("Добавить ссылку / magnet…").clicked() {
                            self.show_add = true;
                            ui.close();
                        }
                        if ui.button("Добавить .torrent…").clicked() {
                            self.pick_torrent_file();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Открыть папку загрузок").clicked() {
                            let _ = std::fs::create_dir_all(&self.settings.download_dir);
                            let _ = std::process::Command::new("explorer")
                                .arg(&self.settings.download_dir)
                                .spawn();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Выход").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.menu_button(RichText::new("Торрент").size(11.5), |ui| {
                        let picked = self.selected_torrent();
                        let paused = picked.as_ref().map(|t| t.phase == Phase::Paused);
                        if ui
                            .add_enabled(picked.is_some(), egui::Button::new("Пауза"))
                            .clicked()
                        {
                            if let Some(t) = &picked {
                                let _ = self.tx.send(Command::Pause(t.id));
                            }
                            ui.close();
                        }
                        if ui
                            .add_enabled(picked.is_some(), egui::Button::new("Продолжить"))
                            .clicked()
                        {
                            if let Some(t) = &picked {
                                if paused == Some(true) {
                                    let _ = self.tx.send(Command::Resume(t.id));
                                }
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(picked.is_some(), egui::Button::new("Открыть папку"))
                            .clicked()
                        {
                            if let Some(t) = &picked {
                                let _ = std::process::Command::new("explorer")
                                    .arg(&t.output_folder)
                                    .spawn();
                            }
                            ui.close();
                        }
                        if ui
                            .add_enabled(picked.is_some(), egui::Button::new("Копировать info-hash"))
                            .clicked()
                        {
                            if let Some(t) = &picked {
                                ui.ctx().copy_text(t.hash.clone());
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(picked.is_some(), egui::Button::new("Удалить…"))
                            .clicked()
                        {
                            if let Some(t) = &picked {
                                self.confirm_delete = Some((t.id, t.name.clone()));
                                self.delete_files = false;
                            }
                            ui.close();
                        }
                    });

                    ui.menu_button(RichText::new("Настройки").size(11.5), |ui| {
                        if ui.button("Параметры…").clicked() {
                            self.draft = self.settings.clone();
                            self.show_settings = true;
                            ui.close();
                        }
                    });

                    ui.menu_button(RichText::new("Справка").size(11.5), |ui| {
                        if ui.button("О программе").clicked() {
                            self.show_about = true;
                            ui.close();
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(ui, |ui| {
                let (down, up) = self.total_speeds();
                let (down_total, up_total) = self.total_traffic();
                let active = self
                    .torrents
                    .iter()
                    .filter(|t| !t.finished && t.phase == Phase::Live)
                    .count();
                let event = self.current_event();
                let ready = self.session_ready;

                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 16.0),
                    Sense::hover(),
                );
                let painter = ui.painter().clone();
                let y = rect.center().y;
                let font = FontId::proportional(10.0);
                let mut x = rect.left();

                painter.text(
                    Pos2::new(x, y),
                    Align2::LEFT_CENTER,
                    format!("DHT: {} nodes", self.dht_nodes),
                    font.clone(),
                    MUTED,
                );
                x += 118.0;
                x = status_sep(&painter, x, rect.y_range());
                x = speed_seg(&painter, x, y, "D:", down, down_total);
                x = status_sep(&painter, x, rect.y_range());
                x = speed_seg(&painter, x, y, "U:", up, up_total);
                x = status_sep(&painter, x, rect.y_range());
                painter.text(
                    Pos2::new(x, y),
                    Align2::LEFT_CENTER,
                    format!(
                        "Порт: {}",
                        self.listen_port
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| "—".to_string())
                    ),
                    font.clone(),
                    MUTED,
                );
                x += 84.0;
                x = status_sep(&painter, x, rect.y_range());
                let torrents_text = format!(
                    "Торрентов: {} ({active} активных)",
                    self.torrents.len()
                );
                let torrents_galley =
                    painter.layout_no_wrap(torrents_text, font.clone(), MUTED);
                painter.galley(
                    Pos2::new(x, y - torrents_galley.size().y * 0.5),
                    torrents_galley.clone(),
                    MUTED,
                );
                x += torrents_galley.size().x;

                // Статус справа — постоянная ширина, чтобы ничего не «прыгало».
                let right_text = if ready {
                    "● Максимальная скорость"
                } else {
                    "● Запуск движка…"
                };
                painter.text(
                    Pos2::new(rect.right() - 10.0, y),
                    Align2::RIGHT_CENTER,
                    right_text,
                    font.clone(),
                    if ready { GREEN } else { YELLOW },
                );

                if let Some((text, is_error)) = event {
                    let ev_right = rect.right() - 168.0;
                    let ev_left = x + 14.0;
                    if ev_right > ev_left + 24.0 {
                        let ev_rect = egui::Rect::from_min_max(
                            Pos2::new(ev_left, rect.top()),
                            Pos2::new(ev_right, rect.bottom()),
                        );
                        let mut child = ui.new_child(
                            UiBuilder::new()
                                .max_rect(ev_rect)
                                .layout(Layout::left_to_right(Align::Center)),
                        );
                        child.add(
                            Label::new(
                                RichText::new(text)
                                    .color(if is_error { RED } else { GREEN })
                                    .size(10.0),
                            )
                            .truncate(),
                        );
                    }
                }
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("sidebar")
            .resizable(false)
            .default_size(176.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("").color(MUTED).size(9.0));
                    ui.label(RichText::new("Torrents").color(TEXT).size(11.5));
                });
                ui.add_space(3.0);
                for filter in Filter::ALL {
                    let count = self.torrents.iter().filter(|t| filter.matches(t)).count();
                    if filter_row(ui, filter.label(), count, self.filter == filter) {
                        self.filter = filter;
                        if let Some(sel) = self.selected {
                            if !self
                                .torrents
                                .iter()
                                .any(|t| t.id == sel && filter.matches(t))
                            {
                                self.selected = None;
                            }
                        }
                    }
                }

                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.add_space(3.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Открыть папку загрузок").size(10.0),
                            )
                            .small(),
                        )
                        .clicked()
                    {
                        let _ = std::fs::create_dir_all(&self.settings.download_dir);
                        let _ = std::process::Command::new("explorer")
                            .arg(&self.settings.download_dir)
                            .spawn();
                    }
                    ui.label(
                        RichText::new(&self.settings.download_dir)
                            .color(MUTED)
                            .size(8.5),
                    );
                });
            });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let selected = self.selected_torrent();
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("+ Добавить").color(TEXT).size(11.0),
                            )
                            .small(),
                        )
                        .clicked()
                    {
                        self.show_add = true;
                    }
                    if ui
                        .add_enabled(
                            selected.is_some(),
                            egui::Button::new(RichText::new("Пауза").size(11.0)).small(),
                        )
                        .clicked()
                    {
                        if let Some(t) = &selected {
                            let _ = self.tx.send(Command::Pause(t.id));
                        }
                    }
                    if ui
                        .add_enabled(
                            selected.is_some(),
                            egui::Button::new(RichText::new("Продолжить").size(11.0)).small(),
                        )
                        .clicked()
                    {
                        if let Some(t) = &selected {
                            let _ = self.tx.send(Command::Resume(t.id));
                        }
                    }
                    if ui
                        .add_enabled(
                            selected.is_some(),
                            egui::Button::new(RichText::new("- Удалить").size(11.0)).small(),
                        )
                        .clicked()
                    {
                        if let Some(t) = &selected {
                            self.confirm_delete = Some((t.id, t.name.clone()));
                            self.delete_files = false;
                        }
                    }
                    if let Some(t) = &selected {
                        let folder = t.output_folder.clone();
                        if ui
                            .add(egui::Button::new(RichText::new("Папка").size(11.0)).small())
                            .clicked()
                        {
                            let _ = std::process::Command::new("explorer").arg(folder).spawn();
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("▼ Smart Search")
                                        .color(if self.search_open { TEXT } else { MUTED })
                                        .size(10.5),
                                )
                                .fill(if self.search_open { HEAD } else { PANEL })
                                .stroke(Stroke::new(1.0, LINE))
                                .small(),
                            )
                            .clicked()
                        {
                            self.search_open = !self.search_open;
                            if !self.search_open {
                                self.search.clear();
                            }
                        }
                        if self.search_open {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.search)
                                    .hint_text("имя или info-hash…")
                                    .desired_width(240.0)
                                    .margin(Vec2::new(6.0, 3.0)),
                            );
                        }
                        if self.pending_adds > 0 {
                            ui.spinner();
                        }
                    });
                });
            });
    }

    fn torrent_table(&mut self, ui: &mut egui::Ui) {
        if let Some(err) = self.session_error.clone() {
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Движок не запустился:").color(RED).strong());
                ui.label(RichText::new(err).color(RED));
            });
            return;
        }

        let mut list: Vec<TorrentSnapshot> = self
            .torrents
            .iter()
            .filter(|t| self.filter.matches(t))
            .cloned()
            .collect();
        let needle = self.search.trim().to_lowercase();
        if !needle.is_empty() {
            list.retain(|t| {
                t.name.to_lowercase().contains(&needle) || t.hash.contains(&needle)
            });
        }
        sort_torrents(&mut list, self.sort_by, self.sort_asc);

        let mut header_sort: Option<SortBy> = None;
        let table_width = (ui.available_width() - 10.0).max(420.0);
        table_header(ui, table_width, self.sort_by, self.sort_asc, &mut header_sort);
        if let Some(key) = header_sort {
            if self.sort_by == key {
                self.sort_asc = !self.sort_asc;
            } else {
                self.sort_by = key;
                self.sort_asc = true;
            }
        }

        ui.painter().hline(
            ui.available_rect_before_wrap().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, LINE),
        );
        ui.add_space(2.0);

        if list.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                if self.torrents.is_empty() {
                    ui.label(RichText::new("Список торрентов пуст").color(MUTED).size(15.0));
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Файл → Добавить ссылку, .torrent или перетащите файл в окно.",
                        )
                        .color(MUTED)
                        .size(11.0),
                    );
                } else {
                    ui.label(RichText::new("Ничего не найдено").color(MUTED).size(13.0));
                }
            });
            return;
        }

        let mut actions = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("torrent_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (idx, t) in list.iter().enumerate() {
                    torrent_row(
                        ui,
                        t,
                        idx + 1,
                        self.selected == Some(t.id),
                        &mut actions,
                        table_width,
                    );
                }
            });

        for action in actions {
            self.handle_action(action, &ui.ctx().clone());
        }
    }

    fn details_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("details")
            .resizable(true)
            .default_size(238.0)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(6, 2)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for tab in DetailsTab::ALL {
                        if tab_button(ui, tab.label(), self.details_tab == tab) {
                            self.details_tab = tab;
                        }
                    }
                });
                ui.add_space(2.0);
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.cursor().top(),
                    Stroke::new(1.0, LINE),
                );
                ui.add_space(3.0);

                let picked = self.selected_torrent();
                let Some(t) = picked else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(40.0);
                        ui.label(RichText::new("Торрент не выбран").color(MUTED).size(12.0));
                    });
                    return;
                };

                let mut actions = Vec::new();
                match self.details_tab {
                    DetailsTab::Files => files_tab(ui, &t, &mut actions),
                    DetailsTab::General => general_tab(ui, &t),
                    DetailsTab::Trackers => trackers_tab(ui, &t, &self.settings),
                    DetailsTab::Speed => speed_tab(ui, &self.speed_hist, &t),
                }
                for action in actions {
                    self.handle_action(action, &ui.ctx().clone());
                }
            });
    }

    // ------------------------------------------------------------- windows

    fn add_window(&mut self, ctx: &egui::Context) {
        if !self.show_add {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("Добавить торрент").size(13.0).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Magnet-ссылка, ссылка на .torrent или info-hash. Можно вставить несколько строк.",
                    )
                    .color(MUTED)
                    .size(10.5),
                );
                ui.add_space(4.0);
                let resp = ui.add(
                    egui::TextEdit::multiline(&mut self.magnet_input)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("magnet:?xt=urn:btih:…"),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let add = ui.add(
                        egui::Button::new(
                            RichText::new("Добавить").color(Color32::WHITE).strong(),
                        )
                        .fill(ACCENT),
                    );
                    if add.clicked() {
                        self.add_magnet();
                    }
                    if ui.button("Из файла…").clicked() {
                        self.pick_torrent_file();
                    }
                    if ui.button("Отмена").clicked() {
                        self.show_add = false;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if resp.has_focus() {
                            ui.label(RichText::new("Ctrl+Enter — добавить").color(MUTED).size(9.5));
                        }
                    });
                });
                let ctrl_enter = resp.has_focus()
                    && ui.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                if ctrl_enter {
                    self.add_magnet();
                }
            });
        if !open {
            self.show_add = false;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.show_about {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("О программе").size(13.0).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("Unkn0WnTorrent {}", settings::APP_VERSION))
                        .color(TEXT)
                        .size(14.0)
                        .strong(),
                );
                ui.label(
                    RichText::new("Быстрый торрент-клиент без рекламы и лимитов скорости.")
                        .color(MUTED)
                        .size(10.5),
                );
                ui.add_space(6.0);
                ui.label(RichText::new("Движок: librqbit").color(MUTED).size(10.0));
                ui.label(
                    RichText::new(format!(
                        "Порт: {}   DHT: {} узлов",
                        self.listen_port
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| "—".to_string()),
                        self.dht_nodes
                    ))
                    .color(MUTED)
                    .size(10.0),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Папка загрузок: {}",
                        self.settings.download_dir
                    ))
                    .color(MUTED)
                    .size(10.0),
                );
                ui.add_space(8.0);
                if ui.button("Закрыть").clicked() {
                    self.show_about = false;
                }
            });
        if !open {
            self.show_about = false;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new(RichText::new("Параметры").size(13.0).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(368.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                section(ui, "ЗАГРУЗКИ");
                ui.horizontal(|ui| {
                    let mut dir = self.draft.download_dir.clone();
                    ui.add(
                        egui::TextEdit::singleline(&mut dir)
                            .desired_width(ui.available_width() - 96.0)
                            .margin(Vec2::new(6.0, 4.0)),
                    );
                    if dir != self.draft.download_dir {
                        self.draft.download_dir = dir;
                    }
                    if ui.button("Выбрать…").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Куда сохранять торренты")
                            .set_directory(&self.draft.download_dir)
                            .pick_folder()
                        {
                            self.draft.download_dir = path.to_string_lossy().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Открыть папку").clicked() {
                        let _ = std::fs::create_dir_all(&self.draft.download_dir);
                        let _ = std::process::Command::new("explorer")
                            .arg(&self.draft.download_dir)
                            .spawn();
                    }
                    ui.label(
                        RichText::new("каждый торрент — в свою папку с названием игры")
                            .color(MUTED)
                            .size(10.0),
                    );
                });

                section(ui, "СКОРОСТЬ И СЕТЬ");
                ui.horizontal(|ui| {
                    ui.label("Пиров на торрент:");
                    ui.add(
                        egui::DragValue::new(&mut self.draft.max_peers_per_torrent)
                            .range(20..=2000)
                            .speed(5),
                    );
                    ui.label(
                        RichText::new("лимитов скорости нет")
                            .color(GREEN)
                            .size(10.0),
                    );
                });
                ui.checkbox(
                    &mut self.draft.add_public_trackers,
                    "Добавлять публичные трекеры (больше сидов)",
                );
                ui.horizontal(|ui| {
                    ui.label("Отдача при загрузке:");
                    ui.add(
                        egui::DragValue::new(&mut self.draft.upload_limit_kbps)
                            .range(0..=1_000_000)
                            .speed(64),
                    );
                    ui.label(
                        RichText::new("КБ/с; 0 — без лимита")
                            .color(MUTED)
                            .size(10.0),
                    );
                });
                ui.label(
                    RichText::new(
                        "Лимит отдачи нужен, чтобы раздача не забирала канал у загрузки. \
                         После завершения отдача снова без лимита.",
                    )
                    .color(MUTED)
                    .size(9.5),
                );
                ui.horizontal(|ui| {
                    ui.label("Порт:");
                    ui.add(
                        egui::DragValue::new(&mut self.draft.listen_port)
                            .range(0..=65535)
                            .speed(64),
                    );
                    ui.label(RichText::new("0 — случайный").color(MUTED).size(10.0));
                });
                ui.checkbox(&mut self.draft.upnp, "Проброс порта через UPnP");
                ui.checkbox(&mut self.draft.utp, "uTP (UDP)");
                ui.label(
                    RichText::new("Порт, UPnP и uTP применятся после перезапуска.")
                        .color(MUTED)
                        .size(9.5),
                );

                section(ui, "ОЧЕРЕДЬ");
                ui.horizontal(|ui| {
                    ui.label("Одновременно качать:");
                    ui.add(
                        egui::DragValue::new(&mut self.draft.max_active_downloads)
                            .range(0..=999)
                            .speed(1),
                    );
                    ui.label(
                        RichText::new("0 — всё сразу, максимум скорости")
                            .color(MUTED)
                            .size(10.0),
                    );
                });

                section(ui, "СИСТЕМА");
                ui.checkbox(
                    &mut self.draft.low_impact,
                    "Щадящий режим — не мешать играм, Discord и звуку",
                );
                ui.label(
                    RichText::new(
                        "Низкий приоритет и редкие перерисовки: микрофон и игры не лагают. \
                         Скорость загрузки не уменьшается.",
                    )
                    .color(MUTED)
                    .size(9.5),
                );

                section(ui, "УВЕДОМЛЕНИЯ");
                ui.checkbox(
                    &mut self.draft.notify_on_finish,
                    "Уведомление Windows о завершении",
                );
                ui.checkbox(&mut self.draft.notify_sound, "Со звуком");
                if ui.button("Проверить уведомление").clicked() {
                    if let Err(e) = settings::notify_test(self.draft.notify_sound) {
                        self.event(format!("{e}"), true);
                    }
                }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    let apply = ui.add(
                        egui::Button::new(
                            RichText::new("Применить").color(Color32::WHITE).strong(),
                        )
                        .fill(ACCENT),
                    );
                    if apply.clicked() {
                        self.apply_settings();
                    }
                    if ui.button("Закрыть").clicked() {
                        self.show_settings = false;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("v{}", settings::APP_VERSION))
                                .color(MUTED)
                                .size(9.5),
                        );
                    });
                });
            });
        if !open {
            self.show_settings = false;
        }
    }

    fn delete_modal(&mut self, ctx: &egui::Context) {
        let Some((id, name)) = self.confirm_delete.clone() else {
            return;
        };
        let mut open = true;
        let mut delete_files = self.delete_files;
        let mut apply = false;
        egui::Window::new(RichText::new("Удалить торрент?").size(13.0).strong())
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new(&name).color(TEXT));
                ui.add_space(6.0);
                ui.checkbox(&mut delete_files, "Удалить скачанные файлы с диска");
                ui.label(
                    RichText::new("Без галочки удалится только запись из списка.")
                        .color(MUTED)
                        .size(10.0),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Удалить").color(RED)).clicked() {
                        apply = true;
                    }
                    if ui.button("Отмена").clicked() {
                        self.confirm_delete = None;
                    }
                });
            });
        self.delete_files = delete_files;
        if apply {
            let _ = self.tx.send(Command::Delete {
                id,
                files: delete_files,
            });
            if self.selected == Some(id) {
                self.selected = None;
            }
            self.torrents.retain(|t| t.id != id);
            self.confirm_delete = None;
            self.event("Торрент удаляется…", false);
        }
        if !open {
            self.confirm_delete = None;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Вызывается и при свёрнутом окне — уведомления не пропадут.
        self.drain_shared();
        self.sync_priority(ctx);
        ctx.request_repaint_after(Duration::from_millis(700));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_shared();
        self.push_speed_history();

        // Drag & drop .torrent-файлов.
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let path = file.path().to_path_buf();
            if path.extension().map(|e| e.eq_ignore_ascii_case("torrent")) == Some(true) {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let name = path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "torrent".to_string());
                        let _ = self.tx.send(Command::AddFile { name, bytes });
                    }
                    Err(e) => self.event(format!("Не удалось прочитать файл: {e}"), true),
                }
            } else {
                self.event("Перетащите файл с расширением .torrent", true);
            }
        }

        // Горячие клавиши.
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                if let Some(t) = self.selected_torrent() {
                    self.confirm_delete = Some((t.id, t.name));
                    self.delete_files = false;
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                if let Some(t) = self.selected_torrent() {
                    if t.phase == Phase::Paused {
                        let _ = self.tx.send(Command::Resume(t.id));
                    } else {
                        let _ = self.tx.send(Command::Pause(t.id));
                    }
                }
            }
        }

        self.menu_bar(ui);
        self.status_bar(ui);
        self.sidebar(ui);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(6, 0)),
            )
            .show(ui, |ui| {
                self.details_panel(ui);
                self.toolbar(ui);
                self.torrent_table(ui);
            });

        self.add_window(&ctx);
        self.settings_window(&ctx);
        self.about_window(&ctx);
        self.delete_modal(&ctx);

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

// ------------------------------------------------------------------ pieces

fn section(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).color(MUTED).size(9.0).strong());
    ui.add_space(1.0);
}

/// Тонкий разделитель между сегментами статус-бара.
fn status_sep(painter: &egui::Painter, x: f32, y_range: egui::Rangef) -> f32 {
    painter.vline(x, y_range.shrink(3.0), Stroke::new(1.0, LINE_DIM));
    x + 11.0
}

/// Скорость + суммарный трафик в колонках фиксированной ширины:
/// цифры выровнены по правому краю и не «прыгают» при обновлении.
fn speed_seg(
    painter: &egui::Painter,
    x: f32,
    y: f32,
    prefix: &str,
    speed: u64,
    total: u64,
) -> f32 {
    let font = FontId::proportional(10.0);
    painter.text(
        Pos2::new(x, y),
        Align2::LEFT_CENTER,
        prefix,
        font.clone(),
        MUTED,
    );
    painter.text(
        Pos2::new(x + 74.0, y),
        Align2::RIGHT_CENTER,
        fmt_speed(speed),
        font.clone(),
        TEXT,
    );
    painter.text(
        Pos2::new(x + 84.0, y),
        Align2::LEFT_CENTER,
        "T:",
        font.clone(),
        MUTED,
    );
    painter.text(
        Pos2::new(x + 168.0, y),
        Align2::RIGHT_CENTER,
        fmt_bytes(total),
        font,
        TEXT,
    );
    x + 174.0
}

fn filter_row(ui: &mut egui::Ui, label: &str, count: usize, selected: bool) -> bool {
    let height = 20.0;
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        Sense::click(),
    );
    let hovered = ui.rect_contains_pointer(rect);
    if selected {
        ui.painter().rect_filled(rect, 0.0, ROW_SELECTED);
    } else if hovered {
        ui.painter().rect_filled(rect, 0.0, ROW_HOVER);
    }
    let painter = ui.painter();
    painter.text(
        Pos2::new(rect.left() + 18.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(if selected { 11.5 } else { 11.0 }),
        if selected { TEXT } else { MUTED },
    );
    painter.text(
        Pos2::new(rect.right() - 6.0, rect.center().y),
        Align2::RIGHT_CENTER,
        count.to_string(),
        FontId::proportional(10.0),
        MUTED,
    );
    resp.clicked()
}

fn tab_button(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let button = egui::Button::new(
        RichText::new(label)
            .size(11.0)
            .color(if selected { TEXT } else { MUTED }),
    )
    .fill(if selected { HEAD } else { BG })
    .stroke(Stroke::new(1.0, if selected { LINE } else { BG }))
    .corner_radius(2.0);
    ui.add(button).clicked()
}

fn col_widths(total: f32) -> [f32; 8] {
    let fixed = 28.0 + 80.0 + 132.0 + 80.0 + 74.0 + 76.0 + 62.0 + 9.0 * 7.0 + 14.0;
    let name = (total - fixed).max(140.0);
    [28.0, name, 80.0, 132.0, 80.0, 74.0, 76.0, 62.0]
}

fn table_header(
    ui: &mut egui::Ui,
    width: f32,
    sort_by: SortBy,
    sort_asc: bool,
    clicked: &mut Option<SortBy>,
) {
    let widths = col_widths(width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, HEAD);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, LINE));

    let mut x = rect.left() + 6.0;
    let headers = [
        ("#", SortBy::Number),
        ("Название", SortBy::Name),
        ("Размер", SortBy::Size),
        ("Статус", SortBy::Progress),
        ("↓", SortBy::Down),
        ("↑", SortBy::Up),
        ("Осталось", SortBy::Eta),
        ("Пиры", SortBy::Peers),
    ];
    for (idx, (label, key)) in headers.iter().enumerate() {
        let w = widths[idx];
        let cell = egui::Rect::from_min_size(
            Pos2::new(x, rect.top()),
            Vec2::new(w, rect.height()),
        );
        let resp = ui.interact(
            cell,
            ui.scope_id().with(("th", idx)),
            Sense::click(),
        );
        let arrow = if sort_by == *key {
            if sort_asc {
                " ▲"
            } else {
                " ▼"
            }
        } else {
            ""
        };
        let align = if idx >= 2 {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        let text_x = match align {
            Align2::RIGHT_CENTER => cell.right() - 4.0,
            _ => cell.left() + 4.0,
        };
        ui.painter().text(
            Pos2::new(text_x, cell.center().y),
            align,
            format!("{label}{arrow}"),
            FontId::proportional(10.0),
            if sort_by == *key { TEXT } else { MUTED },
        );
        if resp.clicked() {
            *clicked = Some(*key);
        }
        if idx < headers.len() - 1 {
            ui.painter().vline(
                cell.right() + 4.0,
                cell.y_range().shrink(3.0),
                Stroke::new(1.0, LINE_DIM),
            );
        }
        x += w + 9.0;
    }
}

fn torrent_row(
    ui: &mut egui::Ui,
    t: &TorrentSnapshot,
    number: usize,
    selected: bool,
    actions: &mut Vec<UiAction>,
    width: f32,
) {
    let height = 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let hovered = ui.rect_contains_pointer(rect);
    let bg = if selected {
        ROW_SELECTED
    } else if hovered {
        ROW_HOVER
    } else {
        ROW
    };
    ui.painter().rect_filled(rect, 0.0, bg);
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, LINE_DIM));

    let widths = col_widths(width);
    let mut x = rect.left() + 6.0;
    let cell = |ui: &mut egui::Ui, x: f32, w: f32| {
        ui.new_child(
            UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    Pos2::new(x, rect.top()),
                    Vec2::new(w, height),
                ))
                .layout(Layout::left_to_right(Align::Center)),
        )
    };

    // №
    let mut c = cell(ui, x, widths[0]);
    c.label(RichText::new(number.to_string()).color(MUTED).size(10.0));
    x += widths[0] + 9.0;

    // Название + иконка состояния
    let mut c = cell(ui, x, widths[1]);
    {
        let (_, color) = state_color(t);
        c.label(RichText::new("●").color(color).size(7.0));
        c.add(
            Label::new(
                RichText::new(&t.name)
                    .color(if t.phase == Phase::Paused { MUTED } else { TEXT })
                    .size(11.0),
            )
            .truncate(),
        );
    }
    x += widths[1] + 9.0;

    // Размер
    let mut c = cell(ui, x, widths[2]);
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(RichText::new(fmt_bytes(t.total_bytes)).color(MUTED).size(10.0));
    });
    x += widths[2] + 9.0;

    // Статус / прогресс
    {
        let frac = pct(t);
        let (status, color) = status_text(t, frac);
        ui.painter().text(
            Pos2::new(x, rect.top() + 3.0),
            Align2::LEFT_TOP,
            status,
            FontId::proportional(10.0),
            color,
        );
        let bar_rect = egui::Rect::from_min_size(
            Pos2::new(x, rect.top() + 15.0),
            Vec2::new(56.0, 3.0),
        );
        ui.painter()
            .rect_filled(bar_rect, 0.0, Color32::from_rgb(26, 26, 30));
        let mut filled = bar_rect;
        filled.set_width(bar_rect.width() * frac);
        ui.painter()
            .rect_filled(filled, 0.0, if t.finished { GREEN } else { ACCENT });
    }
    x += widths[3] + 9.0;

    // ↓
    let mut c = cell(ui, x, widths[4]);
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(
            RichText::new(if t.download_bps > 0 {
                fmt_speed(t.download_bps)
            } else {
                "—".to_string()
            })
            .color(if t.download_bps > 0 { GREEN } else { MUTED })
            .size(10.0),
        );
    });
    x += widths[4] + 9.0;

    // ↑
    let mut c = cell(ui, x, widths[5]);
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(
            RichText::new(if t.upload_bps > 0 {
                fmt_speed(t.upload_bps)
            } else {
                "—".to_string()
            })
            .color(if t.upload_bps > 0 { ACCENT } else { MUTED })
            .size(10.0),
        );
    });
    x += widths[5] + 9.0;

    // Осталось
    let mut c = cell(ui, x, widths[6]);
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(
            RichText::new(eta_text(t))
                .color(MUTED)
                .size(10.0),
        );
    });
    x += widths[6] + 9.0;

    // Пиры
    let mut c = cell(ui, x, widths[7]);
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(
            RichText::new(format!("{}/{}", t.seeders, t.peers))
                .color(MUTED)
                .size(10.0),
        );
    });

    // Клик обрабатываем по ответу самой строки: дочерние подписи не перехватывают
    // его, потому что у них нет Sense::click().
    if resp.clicked() {
        actions.push(UiAction::Select(t.id));
    }
    if resp.double_clicked() {
        actions.push(UiAction::OpenFolder(t.output_folder.clone()));
    }
    resp.context_menu(|ui| {
        let paused = t.phase == Phase::Paused;
        if ui
            .add_enabled(!paused, egui::Button::new("Пауза"))
            .clicked()
        {
            actions.push(UiAction::Pause(t.id));
            ui.close();
        }
        if ui
            .add_enabled(paused, egui::Button::new("Продолжить"))
            .clicked()
        {
            actions.push(UiAction::Resume(t.id));
            ui.close();
        }
        ui.separator();
        ui.menu_button("Приоритет", |ui| {
            for (value, label) in [(0u8, "Высокий"), (1, "Обычный"), (2, "Низкий")] {
                if ui
                    .selectable_label(t.priority == value, label)
                    .clicked()
                {
                    actions.push(UiAction::SetPriority(t.id, value));
                    ui.close();
                }
            }
        });
        if ui.button("Открыть папку").clicked() {
            actions.push(UiAction::OpenFolder(t.output_folder.clone()));
            ui.close();
        }
        if ui.button("Копировать info-hash").clicked() {
            actions.push(UiAction::Copy(t.hash.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button(RichText::new("Удалить…").color(RED)).clicked() {
            actions.push(UiAction::Delete(t.id, t.name.clone()));
            ui.close();
        }
    });
}

fn files_tab(ui: &mut egui::Ui, t: &TorrentSnapshot, actions: &mut Vec<UiAction>) {
    if t.files.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            ui.label(RichText::new("Список файлов появится после метаданных").color(MUTED).size(11.0));
        });
        return;
    }

    let selected_count = t
        .selected_files
        .as_ref()
        .map(|v| v.len())
        .unwrap_or(t.files.len());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "Файлов: {}   ·   отмечено: {}",
                t.files.len(),
                selected_count
            ))
            .color(MUTED)
            .size(10.0),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Ничего").clicked() {
                actions.push(UiAction::Error(
                    "Нужно оставить хотя бы один файл".to_string(),
                ));
            }
            if ui.small_button("Все").clicked() {
                let all: Vec<usize> = (0..t.files.len()).collect();
                actions.push(UiAction::SetFiles(t.id, all));
            }
        });
    });

    let width = (ui.available_width() - 10.0).max(560.0);
    let (path_w, cols) = file_widths(width);

    let (hrect, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    ui.painter().rect_filled(hrect, 0.0, HEAD);
    let mut x = hrect.left() + 2.0;
    let headers = [
        ("✓", false),
        ("Path", false),
        ("Size", true),
        ("Done", true),
        ("%", true),
        ("# Pieces", true),
        ("Pieces", true),
        ("Priority", false),
        ("Rate", true),
        ("Resolution", false),
        ("Duration", false),
        ("Streamable", false),
        ("Has header", false),
    ];
    for (idx, (label, right)) in headers.iter().enumerate() {
        let w = match idx {
            0 => cols[0],
            1 => path_w,
            _ => cols[idx - 1],
        };
        let align = if *right {
            Align2::RIGHT_CENTER
        } else {
            Align2::LEFT_CENTER
        };
        let tx = if *right { x + w - 3.0 } else { x + 2.0 };
        ui.painter().text(
            Pos2::new(tx, hrect.center().y),
            align,
            label,
            FontId::proportional(9.0),
            MUTED,
        );
        x += w + FILE_GAP;
    }
    ui.painter()
        .hline(hrect.x_range(), hrect.bottom(), Stroke::new(1.0, LINE));

    egui::ScrollArea::vertical()
        .id_salt(("files_scroll", t.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for f in &t.files {
                file_row(ui, t, f, width, path_w, &cols, actions);
            }
        });
}

const FILE_GAP: f32 = 5.0;

fn file_widths(avail: f32) -> (f32, [f32; 12]) {
    let mut cols = [
        20.0, 62.0, 62.0, 38.0, 50.0, 64.0, 56.0, 58.0, 60.0, 52.0, 58.0, 58.0,
    ];
    let gaps = FILE_GAP * 12.0 + 6.0;
    let sum: f32 = cols.iter().sum();
    let mut path = (avail - gaps - sum).max(110.0);
    if gaps + sum + path > avail {
        let scale = ((avail - gaps - 110.0) / sum).clamp(0.65, 1.0);
        for c in &mut cols {
            *c = (*c * scale).max(26.0);
        }
        path = (avail - gaps - cols.iter().sum::<f32>()).max(90.0);
    }
    (path, cols)
}

fn file_row(
    ui: &mut egui::Ui,
    t: &TorrentSnapshot,
    f: &FileSnapshot,
    width: f32,
    path_w: f32,
    cols: &[f32; 12],
    actions: &mut Vec<UiAction>,
) {
    let check_w = cols[0];
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    if ui.rect_contains_pointer(rect) {
        ui.painter().rect_filled(rect, 0.0, ROW_HOVER);
    }

    let mut enabled = t
        .selected_files
        .as_ref()
        .map(|v| v.contains(&f.index))
        .unwrap_or(true);

    let mut x = rect.left() + 2.0;
    let mut c = ui.new_child(
        UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                Pos2::new(x, rect.top()),
                Vec2::new(check_w, rect.height()),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    if c.add(egui::Checkbox::without_text(&mut enabled)).changed() {
        let mut set: HashSet<usize> = t
            .selected_files
            .clone()
            .map(|v| v.into_iter().collect())
            .unwrap_or_else(|| (0..t.files.len()).collect());
        if enabled {
            set.insert(f.index);
        } else {
            set.remove(&f.index);
        }
        if set.is_empty() {
            actions.push(UiAction::Error(
                "Нужно оставить хотя бы один файл".to_string(),
            ));
        } else {
            let mut v: Vec<usize> = set.into_iter().collect();
            v.sort_unstable();
            actions.push(UiAction::SetFiles(t.id, v));
        }
    }
    x += check_w + FILE_GAP;

    let mut c = ui.new_child(
        UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                Pos2::new(x, rect.top()),
                Vec2::new(path_w, rect.height()),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    c.add(
        Label::new(
            RichText::new(f.name.as_str())
                .color(if enabled { TEXT } else { MUTED })
                .size(9.5),
        )
        .truncate(),
    );
    x += path_w + FILE_GAP;

    let pct_file = if f.len > 0 {
        (f.progress as f64 / f.len as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        100.0
    };
    let pieces_done = if f.len > 0 {
        ((f.progress as f64 / f.len as f64) * f.piece_count as f64)
            .round()
            .clamp(0.0, f.piece_count as f64) as u32
    } else {
        0
    };

    for (idx, (value, right)) in [
        (fmt_bytes(f.len), true),
        (fmt_bytes(f.progress), true),
        (format!("{pct_file:.0}%"), true),
        (f.piece_count.to_string(), true),
        (
            format!("{pieces_done}/{}", f.piece_count),
            true,
        ),
        (if enabled { "Normal".to_string() } else { "Skip".to_string() }, false),
        ("—".to_string(), true),
        ("—".to_string(), false),
        ("—".to_string(), false),
        ("No".to_string(), false),
        ("—".to_string(), false),
    ]
    .into_iter()
    .enumerate()
    {
        let w = cols[idx + 1];
        let layout = if right {
            Layout::right_to_left(Align::Center)
        } else {
            Layout::left_to_right(Align::Center)
        };
        let mut c = ui.new_child(
            UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    Pos2::new(x, rect.top()),
                    Vec2::new(w, rect.height()),
                ))
                .layout(layout),
        );
        let color = match idx {
            5 if !enabled => RED,
            4 if pieces_done == f.piece_count && f.piece_count > 0 => GREEN,
            _ => MUTED,
        };
        c.label(RichText::new(value).color(color).size(9.0));
        x += w + FILE_GAP;
    }
}

fn general_tab(ui: &mut egui::Ui, t: &TorrentSnapshot) {
    egui::ScrollArea::vertical()
        .id_salt("general")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("general_grid")
                .num_columns(2)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    kv(ui, "Название", &t.name);
                    kv(ui, "Info-hash", &t.hash);
                    kv(ui, "Состояние", state_label(t));
                    kv(ui, "Приоритет", priority_label(t.priority));
                    kv(
                        ui,
                        "Размер",
                        &format!(
                            "{} (скачано {})",
                            fmt_bytes(t.total_bytes),
                            fmt_bytes(t.progress_bytes)
                        ),
                    );
                    kv(ui, "Отдано", &fmt_bytes(t.uploaded_bytes));
                    kv(
                        ui,
                        "Скорость",
                        &format!(
                            "↓ {}   ↑ {}",
                            fmt_speed(t.download_bps),
                            fmt_speed(t.upload_bps)
                        ),
                    );
                    kv(
                        ui,
                        "Пиры",
                        &format!("{} (сидов {})", t.peers, t.seeders),
                    );
                    kv(ui, "Осталось", &eta_text(t));
                    kv(ui, "Папка", &t.output_folder);
                });
        });
}

fn trackers_tab(ui: &mut egui::Ui, t: &TorrentSnapshot, settings: &Settings) {
    egui::ScrollArea::vertical()
        .id_salt("trackers")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!("Info-hash: {}", t.hash))
                    .color(MUTED)
                    .size(9.5),
            );
            ui.add_space(3.0);
            let mut x = 4.0;
            let (rect, _) = ui.allocate_exact_size(
                Vec2::new(ui.available_width(), 18.0),
                Sense::hover(),
            );
            ui.painter().rect_filled(rect, 0.0, HEAD);
            for (label, w) in [("Трекер", rect.width() - 130.0), ("Статус", 120.0)] {
                ui.painter().text(
                    Pos2::new(rect.left() + x, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(9.5),
                    MUTED,
                );
                x += w;
            }
            ui.painter()
                .hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, LINE));

            for tracker in settings.public_trackers() {
                ui.horizontal(|ui| {
                    ui.add(
                        Label::new(
                            RichText::new(tracker).color(TEXT).size(10.0),
                        )
                        .truncate(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("авто").color(GREEN).size(9.5));
                    });
                });
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("Публичные трекеры добавляются автоматически и опрашиваются раз в минуту.")
                    .color(MUTED)
                    .size(9.5),
            );
        });
}

fn speed_tab(ui: &mut egui::Ui, hist: &VecDeque<(u64, u64)>, t: &TorrentSnapshot) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("↓ {}", fmt_speed(t.download_bps)))
                .color(GREEN)
                .size(11.0)
                .strong(),
        );
        ui.label(
            RichText::new(format!("↑ {}", fmt_speed(t.upload_bps)))
                .color(ACCENT)
                .size(11.0),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("последние 4 минуты").color(MUTED).size(9.0));
        });
    });
    ui.add_space(2.0);
    draw_graph(ui, hist);
}

fn draw_graph(ui: &mut egui::Ui, hist: &VecDeque<(u64, u64)>) {
    let height = (ui.available_height() - 8.0).max(60.0);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Color32::from_rgb(5, 5, 6));
    for i in 0..=4 {
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        painter.hline(rect.x_range(), y, Stroke::new(1.0, LINE_DIM));
    }

    let max = hist
        .iter()
        .map(|(d, u)| (*d).max(*u))
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    painter.text(
        Pos2::new(rect.left() + 4.0, rect.top() + 2.0),
        Align2::LEFT_TOP,
        fmt_speed(max as u64),
        FontId::proportional(9.0),
        MUTED,
    );

    let n = hist.len();
    if n < 2 {
        return;
    }
    let step = rect.width() / (n - 1) as f32;
    let mk = |idx: usize, v: u64| {
        Pos2::new(
            rect.left() + idx as f32 * step,
            rect.bottom() - (v as f32 / max) * rect.height(),
        )
    };
    let down: Vec<Pos2> = hist.iter().enumerate().map(|(i, (d, _))| mk(i, *d)).collect();
    let up: Vec<Pos2> = hist.iter().enumerate().map(|(i, (_, u))| mk(i, *u)).collect();
    painter.add(Shape::line(down, Stroke::new(1.2, GREEN)));
    painter.add(Shape::line(up, Stroke::new(1.0, ACCENT)));
}

fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.label(RichText::new(key).color(MUTED).size(10.0));
    ui.add(Label::new(RichText::new(value).color(TEXT).size(10.0)).truncate());
    ui.end_row();
}

fn sort_torrents(list: &mut [TorrentSnapshot], by: SortBy, asc: bool) {
    list.sort_by(|a, b| {
        let ord = match by {
            SortBy::Number | SortBy::Name => {
                a.name.to_lowercase().cmp(&b.name.to_lowercase())
            }
            SortBy::Size => a.total_bytes.cmp(&b.total_bytes),
            SortBy::Progress => pct(a)
                .partial_cmp(&pct(b))
                .unwrap_or(std::cmp::Ordering::Equal),
            SortBy::Down => a.download_bps.cmp(&b.download_bps),
            SortBy::Up => a.upload_bps.cmp(&b.upload_bps),
            SortBy::Eta => a.eta_secs.unwrap_or(u64::MAX).cmp(&b.eta_secs.unwrap_or(u64::MAX)),
            SortBy::Peers => a.peers.cmp(&b.peers),
        };
        if asc {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn pct(t: &TorrentSnapshot) -> f32 {
    if t.total_bytes > 0 {
        (t.progress_bytes as f64 / t.total_bytes as f64).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

fn state_color(t: &TorrentSnapshot) -> (&'static str, Color32) {
    if t.error.is_some() || t.phase == Phase::Error {
        return ("Ошибка", RED);
    }
    if t.finished {
        return ("Завершён", GREEN);
    }
    match t.phase {
        Phase::Live => ("Скачивается", ACCENT),
        Phase::Paused => ("Пауза", MUTED),
        Phase::Checking => ("Проверка", YELLOW),
        Phase::Error => ("Ошибка", RED),
    }
}

fn status_text(t: &TorrentSnapshot, frac: f32) -> (String, Color32) {
    if t.error.is_some() || t.phase == Phase::Error {
        return ("Ошибка".to_string(), RED);
    }
    if t.finished {
        return (
            if t.phase == Phase::Live {
                "Раздаётся".to_string()
            } else {
                "Завершён".to_string()
            },
            GREEN,
        );
    }
    match t.phase {
        Phase::Live => (format!("Скачивается {:.1}%", frac * 100.0), ACCENT),
        Phase::Paused => ("Пауза".to_string(), MUTED),
        Phase::Checking => ("Проверка…".to_string(), YELLOW),
        Phase::Error => ("Ошибка".to_string(), RED),
    }
}

fn state_label(t: &TorrentSnapshot) -> &'static str {
    if t.error.is_some() || t.phase == Phase::Error {
        return "Ошибка";
    }
    if t.finished {
        return if t.phase == Phase::Live {
            "Раздаётся"
        } else {
            "Завершён"
        };
    }
    match t.phase {
        Phase::Live => "Скачивается",
        Phase::Paused => "На паузе",
        Phase::Checking => "Проверка",
        Phase::Error => "Ошибка",
    }
}

fn priority_label(priority: u8) -> &'static str {
    match priority {
        0 => "Высокий",
        2 => "Низкий",
        _ => "Обычный",
    }
}

fn eta_text(t: &TorrentSnapshot) -> String {
    match t.eta_secs {
        Some(secs) if !t.finished => fmt_eta(secs),
        _ => {
            if t.finished {
                "готово".to_string()
            } else {
                "—".to_string()
            }
        }
    }
}

/// Принимает info-hash и превращает его в magnet-ссылку.
fn normalize_link(link: &str) -> String {
    let trimmed = link.trim();
    let looks_hash = (trimmed.len() == 40 && trimmed.chars().all(|c| c.is_ascii_hexdigit()))
        || (trimmed.len() == 32 && trimmed.chars().all(|c| c.is_ascii_alphanumeric()));
    if looks_hash && !trimmed.contains(':') {
        format!("magnet:?xt=urn:btih:{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for path in [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\tahoma.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "win_ui".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
                family.insert(0, "win_ui".to_owned());
            }
            if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
                family.push("win_ui".to_owned());
            }
            break;
        }
    }
    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(8, 8, 9);
    visuals.faint_bg_color = ROW_HOVER;
    visuals.selection.bg_fill = ROW_SELECTED;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.window_stroke = Stroke::new(1.0, LINE);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(27, 28, 33);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(23, 24, 28);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(52, 54, 61));
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.corner_radius = 4.into();
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(37, 41, 50);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(37, 41, 50);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(92, 106, 132));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.hovered.corner_radius = 4.into();
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(130, 174, 245));
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.active.corner_radius = 4.into();
    visuals.widgets.open = visuals.widgets.hovered;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.corner_radius = 4.into();
    visuals.hyperlink_color = ACCENT;
    visuals.window_corner_radius = 6.into();
    visuals.menu_corner_radius = 6.into();
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(160),
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);

    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = Vec2::new(6.0, 3.0);
        style.spacing.button_padding = Vec2::new(9.0, 4.0);
        // Тонкий «плавающий» скроллбар: не занимает место и не рисует
        // постоянную светлую полосу с рамкой.
        style.spacing.scroll.floating = true;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating_width = 5.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.spacing.scroll.handle_min_length = 28.0;
        style.spacing.scroll.foreground_color = true;
        style.spacing.scroll.dormant_handle_opacity = 0.14;
        style.spacing.scroll.active_handle_opacity = 0.5;
        style.spacing.scroll.interact_handle_opacity = 0.9;
        style.visuals.override_text_color = Some(TEXT);
    });
}

pub fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn fmt_speed(bytes_per_sec: u64) -> String {
    format!("{}/s", fmt_bytes(bytes_per_sec))
}

pub fn fmt_eta(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h} ч {m} мин")
    } else if m > 0 {
        format!("{m} мин")
    } else {
        format!("{s} с")
    }
}
