use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use librqbit::api::TorrentIdOrHash;
use librqbit::dht::DhtPersistenceConfig;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, ConnectionOptions, DhtSessionConfig, ListenerMode,
    ListenerOptions, ManagedTorrent, PeerConnectionOptions, Session, SessionOptions,
    SessionPersistenceConfig, TorrentStatsState,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::settings::{self, Settings};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Checking,
    Live,
    Paused,
    Error,
}

#[derive(Clone, Debug)]
pub struct FileSnapshot {
    pub index: usize,
    pub name: String,
    pub len: u64,
    pub progress: u64,
    pub piece_count: u32,
}

#[derive(Clone, Debug)]
pub struct TorrentSnapshot {
    pub id: usize,
    pub name: String,
    pub hash: String,
    pub phase: Phase,
    pub error: Option<String>,
    pub total_bytes: u64,
    pub progress_bytes: u64,
    pub uploaded_bytes: u64,
    pub download_bps: u64,
    pub upload_bps: u64,
    pub eta_secs: Option<u64>,
    pub finished: bool,
    pub peers: u32,
    pub seeders: u32,
    pub files: Vec<FileSnapshot>,
    pub selected_files: Option<Vec<usize>>,
    pub output_folder: String,
    pub priority: u8,
}

#[derive(Clone, Debug)]
pub struct ToastEvent {
    pub name: String,
    pub folder: String,
}

pub struct SharedState {
    pub torrents: Vec<TorrentSnapshot>,
    pub version: u64,
    pub session_ready: bool,
    pub session_error: Option<String>,
    pub listen_port: Option<u16>,
    pub dht_nodes: usize,
    pub pending_adds: usize,
    pub event: Option<(String, bool, Instant)>,
    pub toasts: Vec<ToastEvent>,
}

impl SharedState {
    fn new() -> Self {
        Self {
            torrents: Vec::new(),
            version: 0,
            session_ready: false,
            session_error: None,
            listen_port: None,
            dht_nodes: 0,
            pending_adds: 0,
            event: None,
            toasts: Vec::new(),
        }
    }

    pub fn set_event(&mut self, text: impl Into<String>, is_error: bool) {
        self.event = Some((text.into(), is_error, Instant::now()));
        self.version = self.version.wrapping_add(1);
    }
}

pub type Shared = Arc<Mutex<SharedState>>;

pub enum Command {
    AddUrl(String),
    AddFile { name: String, bytes: Vec<u8> },
    Pause(usize),
    Resume(usize),
    Delete { id: usize, files: bool },
    SetFiles { id: usize, files: Vec<usize> },
    SetPriority { id: usize, priority: u8 },
    UpdateSettings(Settings),
}

pub fn start(settings: Settings, rx: UnboundedReceiver<Command>) -> Shared {
    let shared = Arc::new(Mutex::new(SharedState::new()));
    let shared2 = shared.clone();
    std::thread::Builder::new()
        .name("torrent-engine".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("torrent-io")
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    if let Ok(mut g) = shared2.lock() {
                        g.session_error = Some(format!("не удалось создать runtime: {e}"));
                    }
                    return;
                }
            };
            runtime.block_on(run(settings, rx, shared2));
        })
        .expect("не удалось запустить поток торрент-движка");
    shared
}

fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let appender = tracing_appender::rolling::daily(settings::logs_dir(), "unkn0wntorrent.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    Some(guard)
}

fn session_options(
    settings: &Settings,
    listen_port: u16,
    persistence: bool,
    upnp: bool,
) -> SessionOptions {
    let peer_opts = PeerConnectionOptions {
        connect_timeout: Some(Duration::from_secs(8)),
        read_write_timeout: Some(Duration::from_secs(120)),
        keep_alive_interval: Some(Duration::from_secs(60)),
        // Максимально большое окно запросов: на быстром канале и больших
        // задержках это главный способ не простаивать между блоками.
        max_request_window: Some(1024),
    };

    SessionOptions {
        dht: Some(DhtSessionConfig {
            bootstrap_addrs: None,
            port: None,
            persistence: Some(DhtPersistenceConfig::default()),
        }),
        disable_trackers: false,
        fastresume: persistence,
        persistence: if persistence {
            Some(SessionPersistenceConfig::Json {
                folder: Some(settings::data_dir().join("session")),
            })
        } else {
            None
        },
        listen: Some(ListenerOptions {
            mode: if settings.utp {
                ListenerMode::TcpAndUtp
            } else {
                ListenerMode::TcpOnly
            },
            listen_addr: SocketAddr::from(([0, 0, 0, 0], listen_port)),
            enable_upnp_port_forwarding: upnp,
            announce_port: if listen_port != 0 && upnp {
                Some(listen_port)
            } else {
                None
            },
            ipv4_only: false,
            ..Default::default()
        }),
        connect: Some(ConnectionOptions {
            enable_tcp: true,
            peer_opts: Some(peer_opts),
            ..Default::default()
        }),
        peer_limit: Some(settings.peers_per_torrent()),
        // Публичные трекеры на уровне сессии: подхватываются всеми торрентами,
        // в том числе восстановленными из прошлой сессии.
        trackers: settings
            .public_trackers()
            .iter()
            .filter_map(|t| t.parse().ok())
            .collect(),
        // Больше параллельных инициализаций — быстрее старт пачки торрентов.
        // Но каждая инициализация хеширует куски, поэтому не больше четырёх:
        // так старт всё равно быстрый, а процессор не загружается на 100%.
        concurrent_init_limit: Some(4),
        // Дисковые операции идут в blocking-пул: несколько потоков с запасом
        // покрывают запись на любой скорости канала, а лишние потоки только
        // создают рывки нагрузки (из-за них трещат звук и микрофон).
        runtime_worker_threads: Some(
            std::thread::available_parallelism()
                .map(|n| (n.get() + 2).clamp(6, 12))
                .unwrap_or(6),
        ),
        client_name_and_version: Some(format!(
            "{}/{} (librqbit)",
            settings::APP_NAME,
            settings::APP_VERSION
        )),
        ratelimits: LimitsConfig {
            upload_bps: upload_limit(settings),
            download_bps: None,
        },
        ..Default::default()
    }
}

fn upload_limit(settings: &Settings) -> Option<NonZeroU32> {
    NonZeroU32::new(settings.upload_limit_kbps.saturating_mul(1024))
}

/// Пока идёт загрузка — придерживаем отдачу, иначе она забивает канал
/// (на гигабитном линке отдача легко съедает 900 Мбит/с, и на загрузку
/// остаются считанные мегабайты). Когда всё скачано — отдаём без лимита.
fn apply_upload_limit(session: &Session, list: &[TorrentSnapshot], settings: &Settings) {
    let downloading = list
        .iter()
        .any(|t| t.phase == Phase::Live && !t.finished);
    let wanted = if downloading {
        upload_limit(settings)
    } else {
        None
    };
    if session.ratelimits.get_upload_bps() != wanted {
        session.ratelimits.set_upload_bps(wanted);
    }
}

async fn make_session(settings: &Settings) -> anyhow::Result<Arc<Session>> {
    // 1. Полная конфигурация. 2. Если порт занят — случайный порт. 3. Без сохранения
    // сессии и UPnP, чтобы приложение запустилось даже при проблемах с диском/сетью.
    let attempts = [
        (settings.listen_port, true, settings.upnp),
        (0, true, false),
        (0, false, false),
    ];

    let mut last_error = None;
    for (port, persistence, upnp) in attempts {
        match Session::new_with_opts(
            settings.download_dir.clone().into(),
            session_options(settings, port, persistence, upnp),
        )
        .await
        {
            Ok(session) => return Ok(session),
            Err(e) => {
                tracing::warn!("не удалось создать сессию (порт={port}, persistence={persistence}): {e:#}");
                last_error = Some(e);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("неизвестная ошибка создания сессии")))
}

fn priorities_path() -> std::path::PathBuf {
    settings::data_dir().join("priorities.json")
}

fn load_priorities() -> HashMap<String, u8> {
    std::fs::read_to_string(priorities_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_priorities(map: &HashMap<String, u8>) {
    if let Ok(data) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(priorities_path(), data);
    }
}

fn get_handle(session: &Session, id: usize) -> Option<Arc<ManagedTorrent>> {
    session.with_torrents(|it| {
        for (tid, h) in it {
            if tid == id {
                return Some(h.clone());
            }
        }
        None
    })
}

fn get_hash(session: &Session, id: usize) -> Option<String> {
    session.with_torrents(|it| {
        for (tid, h) in it {
            if tid == id {
                return Some(h.info_hash().as_string());
            }
        }
        None
    })
}

fn collect_snapshots(session: &Session, priorities: &HashMap<String, u8>) -> Vec<TorrentSnapshot> {
    session.with_torrents(|it| {
        let mut out = Vec::new();
        for (id, handle) in it {
            let stats = handle.stats();
            let hash = handle.info_hash().as_string();
            let name = handle
                .name()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    if hash.len() >= 10 {
                        format!("Торрент {}", &hash[..10])
                    } else {
                        format!("Торрент #{id}")
                    }
                });

            let (phase, error) = match stats.state {
                TorrentStatsState::Live => (Phase::Live, None),
                TorrentStatsState::Paused => (Phase::Paused, None),
                TorrentStatsState::Initializing { paused } => {
                    if paused {
                        (Phase::Paused, None)
                    } else {
                        (Phase::Checking, None)
                    }
                }
                TorrentStatsState::Error => (Phase::Error, stats.error.clone()),
            };

            let files = handle
                .with_metadata(|meta| {
                    meta.file_infos
                        .iter()
                        .enumerate()
                        .map(|(index, f)| FileSnapshot {
                            index,
                            name: f.relative_filename.to_string_lossy().replace('\\', "/"),
                            len: f.len,
                            progress: stats.file_progress.get(index).copied().unwrap_or(0),
                            piece_count: f.piece_range.end.saturating_sub(f.piece_range.start),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let download_bps = stats
                .live
                .as_ref()
                .map(|l| l.download_speed.as_bytes())
                .unwrap_or(0);
            let upload_bps = stats
                .live
                .as_ref()
                .map(|l| l.upload_speed.as_bytes())
                .unwrap_or(0);
            let (peers, seeders) = stats
                .live
                .as_ref()
                .map(|l| {
                    (
                        l.snapshot.peer_stats.live,
                        l.snapshot.peer_stats.live_seeders,
                    )
                })
                .unwrap_or((0, 0));

            let eta_secs = if download_bps > 0 && stats.total_bytes > stats.progress_bytes {
                Some((stats.total_bytes - stats.progress_bytes) / download_bps)
            } else {
                None
            };

            out.push(TorrentSnapshot {
                id,
                name,
                hash: hash.clone(),
                phase,
                error,
                total_bytes: stats.total_bytes,
                progress_bytes: stats.progress_bytes,
                uploaded_bytes: stats.uploaded_bytes,
                download_bps,
                upload_bps,
                eta_secs,
                finished: stats.finished,
                peers,
                seeders,
                files,
                selected_files: handle.only_files(),
                output_folder: handle.output_folder().to_string_lossy().to_string(),
                priority: priorities.get(&hash).copied().unwrap_or(1),
            });
        }
        out
    })
}

fn add_options(settings: &Settings, folder_name: Option<String>) -> AddTorrentOptions {
    let trackers = settings.public_trackers();
    // Каждый торрент — в свою папку с его названием:
    // %USERPROFILE%\Downloads\Unkn0WnTorrent\<Название игры>
    let output_folder = match folder_name.and_then(|f| settings::sanitize_folder_name(&f)) {
        Some(folder) => std::path::Path::new(&settings.download_dir).join(folder),
        None => std::path::PathBuf::from(&settings.download_dir),
    };
    AddTorrentOptions {
        overwrite: true,
        output_folder: Some(output_folder.to_string_lossy().to_string()),
        peer_limit: Some(settings.peers_per_torrent()),
        // Трекеры опрашиваем раз в минуту, чтобы быстрее набирать пиров.
        force_tracker_interval: Some(Duration::from_secs(60)),
        trackers: if trackers.is_empty() {
            None
        } else {
            Some(trackers)
        },
        ..Default::default()
    }
}

/// Имя торрента из magnet-ссылки (параметр dn), если оно там есть.
fn magnet_display_name(url: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key.eq_ignore_ascii_case("dn") {
            let decoded = percent_decode(value);
            if !decoded.trim().is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_digit(bytes[i + 1]);
                let lo = hex_digit(bytes[i + 2]);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi * 16 + lo);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Папка для торрента: многофайловый — по имени торрента, однофайловый — по имени
/// файла без расширения (чтобы не было папки «Game.iso» с файлом «Game.iso» внутри).
fn torrent_folder_name(bytes: &[u8]) -> Option<String> {
    let meta = librqbit::torrent_from_bytes(bytes).ok()?;
    let info = &meta.info.data;
    let raw = info.name.as_ref()?;
    let name = String::from_utf8_lossy(raw.as_ref()).to_string();
    if name.trim().is_empty() {
        return None;
    }
    if info.length.is_some() {
        Some(folder_stem(&name))
    } else {
        Some(name)
    }
}

fn folder_stem(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| name.to_string())
}

/// Для magnet-ссылки известен только dn. Если это явно имя файла-образа/архива,
/// берём имя без расширения, иначе — как есть.
fn magnet_folder_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    const FILE_EXTS: [&str; 12] = [
        "iso", "exe", "zip", "rar", "7z", "bin", "msi", "img", "dmg", "tar", "gz", "xz",
    ];
    if FILE_EXTS.iter().any(|ext| lower.ends_with(&format!(".{ext}"))) {
        folder_stem(name)
    } else {
        name.to_string()
    }
}

/// Сглаживаем скачки индикатора скорости, чтобы цифры не «прыгали» каждые полсекунды.
fn smooth_speeds(list: &mut [TorrentSnapshot], ema: &mut HashMap<usize, (f64, f64)>) {
    for t in list.iter_mut() {
        if t.phase != Phase::Live {
            t.download_bps = 0;
            t.upload_bps = 0;
            ema.remove(&t.id);
            continue;
        }
        let cur = (t.download_bps as f64, t.upload_bps as f64);
        let e = ema.entry(t.id).or_insert(cur);
        let alpha = 0.3;
        e.0 = e.0 * (1.0 - alpha) + cur.0 * alpha;
        e.1 = e.1 * (1.0 - alpha) + cur.1 * alpha;
        t.download_bps = e.0.round() as u64;
        t.upload_bps = e.1.round() as u64;
    }
}

async fn enforce_queue(
    session: &Arc<Session>,
    list: &[TorrentSnapshot],
    settings: &Settings,
    user_paused: &HashSet<usize>,
    auto_paused: &mut HashSet<usize>,
) {
    if settings.max_active_downloads == 0 {
        let ids: Vec<usize> = auto_paused.drain().collect();
        for id in ids {
            if user_paused.contains(&id) {
                continue;
            }
            if let Some(h) = get_handle(session, id) {
                if h.is_paused() {
                    let _ = session.unpause(&h).await;
                }
            }
        }
        return;
    }

    let mut candidates: Vec<&TorrentSnapshot> = list
        .iter()
        .filter(|t| !t.finished && t.phase != Phase::Error && !user_paused.contains(&t.id))
        .collect();
    candidates.sort_by_key(|t| (std::cmp::Reverse(t.priority), t.id));

    let limit = settings.max_active_downloads as usize;
    for (rank, t) in candidates.iter().enumerate() {
        let Some(handle) = get_handle(session, t.id) else {
            continue;
        };
        if rank < limit {
            if handle.is_paused() {
                if auto_paused.remove(&t.id) {
                    let _ = session.unpause(&handle).await;
                }
            }
        } else if !handle.is_paused() {
            if session.pause(&handle).await.is_ok() {
                auto_paused.insert(t.id);
            }
        }
    }
}

async fn run(settings: Settings, mut rx: UnboundedReceiver<Command>, shared: Shared) {
    let _log_guard = init_logging();

    let mut settings = settings;
    settings.sanitize();
    let session = match make_session(&settings).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("не удалось создать торрент-сессию: {e:#}");
            if let Ok(mut g) = shared.lock() {
                g.session_error = Some(format!("Не удалось запустить торрент-движок:\n{e:#}"));
                g.version += 1;
            }
            return;
        }
    };

    let listen_port = session
        .announce_port()
        .or_else(|| session.listen_addr().map(|a| a.port()));

    let mut priorities = load_priorities();
    let mut user_paused: HashSet<usize> = HashSet::new();
    let mut auto_paused: HashSet<usize> = HashSet::new();
    let mut known: HashSet<usize> = HashSet::new();
    let mut notified: HashSet<String> = HashSet::new();
    let mut speed_ema: HashMap<usize, (f64, f64)> = HashMap::new();

    {
        if let Ok(mut g) = shared.lock() {
            g.session_ready = true;
            g.listen_port = listen_port;
            g.set_event(
                match listen_port {
                    Some(p) => format!("Движок готов. Порт {p}. DHT и обмен включены."),
                    None => "Движок готов. DHT и обмен включены.".to_string(),
                },
                false,
            );
        }
    }

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(cmd) => {
                        handle_command(cmd, &session, &shared, &mut settings, &mut priorities,
                            &mut user_paused, &mut auto_paused, &mut notified, &mut known).await;
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(700)) => {}
        }

        let mut list = collect_snapshots(&session, &priorities);
        smooth_speeds(&mut list, &mut speed_ema);
        apply_upload_limit(&session, &list, &settings);
        let mut new_toasts = Vec::new();

        for t in &list {
            if known.insert(t.id) && t.finished {
                // Торрент был завершён ещё до запуска приложения — не спамим уведомлением.
                notified.insert(t.hash.clone());
            }
            if t.finished && notified.insert(t.hash.clone()) {
                tracing::info!(name = %t.name, "загрузка завершена");
                new_toasts.push(ToastEvent {
                    name: t.name.clone(),
                    folder: t.output_folder.clone(),
                });
            }
        }

        enforce_queue(
            &session,
            &list,
            &settings,
            &user_paused,
            &mut auto_paused,
        )
        .await;

        if let Ok(mut g) = shared.lock() {
            g.torrents = list;
            g.dht_nodes = session
                .get_dht()
                .map(|d| {
                    let stats = d.stats();
                    stats.routing_table_size + stats.routing_table_size_v6
                })
                .unwrap_or(0);
            g.version = g.version.wrapping_add(1);
            g.toasts.extend(new_toasts);
        }
    }

    session.stop().await;
}

async fn handle_command(
    cmd: Command,
    session: &Arc<Session>,
    shared: &Shared,
    settings: &mut Settings,
    priorities: &mut HashMap<String, u8>,
    user_paused: &mut HashSet<usize>,
    auto_paused: &mut HashSet<usize>,
    notified: &mut HashSet<String>,
    known: &mut HashSet<usize>,
) {
    match cmd {
        Command::AddUrl(url) => {
            let url = url.trim().to_string();
            if url.is_empty() {
                return;
            }
            let display_name = magnet_display_name(&url);
            let folder = display_name.as_deref().map(magnet_folder_name);
            let opts = add_options(settings, folder);
            {
                if let Ok(mut g) = shared.lock() {
                    g.pending_adds += 1;
                    g.set_event("Добавляю торрент…", false);
                }
            }
            let session = session.clone();
            let shared = shared.clone();
            tokio::spawn(async move {
                let result = session
                    .add_torrent(AddTorrent::from_url(url), Some(opts))
                    .await;
                finish_add(result, shared, None).await;
            });
        }
        Command::AddFile { name, bytes } => {
            let folder = torrent_folder_name(&bytes).or_else(|| {
                std::path::Path::new(&name)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            });
            let opts = add_options(settings, folder);
            {
                if let Ok(mut g) = shared.lock() {
                    g.pending_adds += 1;
                    g.set_event(format!("Добавляю {name}…"), false);
                }
            }
            let session = session.clone();
            let shared = shared.clone();
            tokio::spawn(async move {
                let result = session
                    .add_torrent(AddTorrent::from_bytes(bytes), Some(opts))
                    .await;
                finish_add(result, shared, Some(name)).await;
            });
        }
        Command::Pause(id) => {
            user_paused.insert(id);
            auto_paused.remove(&id);
            if let Some(h) = get_handle(session, id) {
                if let Err(e) = session.pause(&h).await {
                    if let Ok(mut g) = shared.lock() {
                        g.set_event(format!("Не удалось поставить на паузу: {e:#}"), true);
                    }
                }
            }
        }
        Command::Resume(id) => {
            user_paused.remove(&id);
            auto_paused.remove(&id);
            if let Some(h) = get_handle(session, id) {
                if h.is_paused() {
                    if let Err(e) = session.unpause(&h).await {
                        if let Ok(mut g) = shared.lock() {
                            g.set_event(format!("Не удалось продолжить: {e:#}"), true);
                        }
                    }
                }
            }
        }
        Command::Delete { id, files } => {
            let hash = get_hash(session, id);
            match session.delete(TorrentIdOrHash::Id(id), files).await {
                Ok(()) => {
                    user_paused.remove(&id);
                    auto_paused.remove(&id);
                    known.remove(&id);
                    if let Some(hash) = hash {
                        notified.remove(&hash);
                        priorities.remove(&hash);
                        save_priorities(priorities);
                    }
                    if let Ok(mut g) = shared.lock() {
                        g.set_event(
                            if files {
                                "Торрент и файлы удалены"
                            } else {
                                "Торрент удалён из списка"
                            },
                            false,
                        );
                    }
                }
                Err(e) => {
                    if let Ok(mut g) = shared.lock() {
                        g.set_event(format!("Ошибка удаления: {e:#}"), true);
                    }
                }
            }
        }
        Command::SetFiles { id, files } => {
            if files.is_empty() {
                if let Ok(mut g) = shared.lock() {
                    g.set_event("Нужно оставить хотя бы один файл", true);
                }
                return;
            }
            if let Some(h) = get_handle(session, id) {
                let set: HashSet<usize> = files.into_iter().collect();
                if let Err(e) = session.update_only_files(&h, &set).await {
                    if let Ok(mut g) = shared.lock() {
                        g.set_event(format!("Не удалось изменить файлы: {e:#}"), true);
                    }
                }
            }
        }
        Command::SetPriority { id, priority } => {
            if let Some(hash) = get_hash(session, id) {
                priorities.insert(hash, priority.min(2));
                save_priorities(priorities);
            }
        }
        Command::UpdateSettings(new_settings) => {
            *settings = new_settings;
            settings.sanitize();
            if let Ok(mut g) = shared.lock() {
                g.set_event("Настройки применены", false);
            }
        }
    }
}

async fn finish_add(
    result: anyhow::Result<librqbit::AddTorrentResponse>,
    shared: Shared,
    source_name: Option<String>,
) {
    let mut g = shared.lock().unwrap();
    g.pending_adds = g.pending_adds.saturating_sub(1);
    match result {
        Ok(resp) => {
            let handle = resp.into_handle();
            let name = handle
                .as_ref()
                .and_then(|h| h.name())
                .or(source_name)
                .unwrap_or_else(|| "торрент".to_string());
            g.set_event(format!("Добавлен: {name}"), false);
        }
        Err(e) => {
            tracing::error!("ошибка добавления торрента: {e:#}");
            g.set_event(format!("Не удалось добавить торрент: {e:#}"), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_name_and_folder() {
        let url = "magnet:?xt=urn:btih:0123456789abcdef&dn=My%20Game%20%28v1.0%29";
        let name = magnet_display_name(url).unwrap();
        assert_eq!(name, "My Game (v1.0)");
        assert_eq!(magnet_folder_name(&name), "My Game (v1.0)");
        assert_eq!(magnet_folder_name("Game.iso"), "Game");
    }

    #[test]
    fn torrent_name_single_and_multi() {
        let single = b"d4:infod6:lengthi4e4:name8:Game.iso12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        assert_eq!(torrent_folder_name(single).as_deref(), Some("Game"));
        let multi = b"d4:infod5:filesld6:lengthi4e4:pathl4:dataeee4:name4:Game12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        assert_eq!(torrent_folder_name(multi).as_deref(), Some("Game"));
    }

    #[test]
    fn safe_folder_name() {
        assert_eq!(
            settings::sanitize_folder_name("Bad<Name>: v1?").as_deref(),
            Some("Bad_Name__ v1_")
        );
        assert!(settings::sanitize_folder_name("..").is_none());
        assert_eq!(settings::sanitize_folder_name("CON").as_deref(), Some("_CON"));
    }
}
