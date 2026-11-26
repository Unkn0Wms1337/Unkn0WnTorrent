use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const APP_NAME: &str = "Unkn0WnTorrent";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join(APP_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn logs_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_download_dir() -> PathBuf {
    home_dir().join("Downloads").join(APP_NAME)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub download_dir: String,
    pub listen_port: u16,
    pub upnp: bool,
    pub utp: bool,
    pub max_peers_per_torrent: u32,
    /// 0 = качать все торренты одновременно.
    pub max_active_downloads: u32,
    /// Добавлять публичные трекеры к каждому торренту — заметно больше пиров.
    pub add_public_trackers: bool,
    /// Ограничение отдачи при активной загрузке, КБ/с. 0 — без лимита.
    /// Без него отдача забивает канал и загрузка проседает.
    pub upload_limit_kbps: u32,
    pub notify_on_finish: bool,
    pub notify_sound: bool,
    /// Щадящий режим: низкий приоритет процесса, чтобы не лагал звук/Discord.
    pub low_impact: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir().to_string_lossy().to_string(),
            listen_port: 42413,
            upnp: true,
            utp: true,
            max_peers_per_torrent: 500,
            max_active_downloads: 0,
            add_public_trackers: true,
            upload_limit_kbps: 1024,
            notify_on_finish: true,
            notify_sound: true,
            low_impact: true,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        data_dir().join("settings.json")
    }

    pub fn load() -> Self {
        let mut s = match std::fs::read_to_string(Self::path()) {
            Ok(data) => serde_json::from_str::<Settings>(&data).unwrap_or_default(),
            Err(_) => Settings::default(),
        };
        s.sanitize();
        s
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let mut copy = self.clone();
        copy.sanitize();
        let data = serde_json::to_string_pretty(&copy)?;
        std::fs::write(Self::path(), data)?;
        Ok(())
    }

    pub fn sanitize(&mut self) {
        if self.download_dir.trim().is_empty() {
            self.download_dir = default_download_dir().to_string_lossy().to_string();
        }
        self.max_peers_per_torrent = self.max_peers_per_torrent.clamp(20, 2000);
        // Прежняя версия по умолчанию ставила 300 — поднимаем до нового максимума.
        if self.max_peers_per_torrent == 300 {
            self.max_peers_per_torrent = 500;
        }
        self.max_active_downloads = self.max_active_downloads.min(1000);
        self.upload_limit_kbps = self.upload_limit_kbps.min(1_000_000);
        let _ = std::fs::create_dir_all(&self.download_dir);
    }

    pub fn peers_per_torrent(&self) -> usize {
        self.max_peers_per_torrent as usize
    }

    /// Публичные трекеры для быстрого поиска пиров.
    pub fn public_trackers(&self) -> Vec<String> {
        if !self.add_public_trackers {
            return Vec::new();
        }
        [
            "udp://tracker.opentrackr.org:1337/announce",
            "udp://open.demonii.com:1337/announce",
            "udp://open.stealth.si:80/announce",
            "udp://tracker.torrent.eu.org:451/announce",
            "udp://exodus.desync.com:6969/announce",
            "udp://tracker.openbittorrent.com:6969/announce",
            "http://tracker.openbittorrent.com:80/announce",
            "udp://tracker.tiny-vps.com:6969/announce",
            "udp://explodie.org:6969/announce",
            "udp://tracker.dler.org:6969/announce",
            "udp://tracker.qu.ax:6969/announce",
            "udp://tracker.ololosh.space:6969/announce",
            "udp://tracker.moeking.me:6969/announce",
            "udp://opentracker.i2p.rocks:6969/announce",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

/// Безопасное имя подпапки для торрента (ограничения Windows).
pub fn sanitize_folder_name(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        let ch = match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        };
        out.push(ch);
    }
    // Windows не любит точки и пробелы в конце имени.
    let mut out = out.trim().trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() || out == "." || out == ".." {
        return None;
    }
    // Зарезервированные имена устройств.
    let stem = out
        .split('.')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    let truncated: String = out.chars().take(120).collect();
    let truncated = truncated.trim_end_matches(['.', ' ']).trim().to_string();
    if truncated.is_empty() {
        None
    } else {
        Some(truncated)
    }
}

/// Уведомление Windows "Загрузка завершена".
pub fn notify_finished(name: &str, folder: &str, sound: bool) -> anyhow::Result<()> {
    use tauri_winrt_notification::{Duration, Sound, Toast};

    let mut toast = Toast::new(Toast::POWERSHELL_APP_ID)
        .title("Загрузка завершена")
        .text1(name)
        .text2(folder)
        .duration(Duration::Long);
    toast = toast.sound(if sound {
        Some(Sound::Default)
    } else {
        None
    });
    toast
        .show()
        .map_err(|e| anyhow::anyhow!("не удалось показать уведомление: {e:?}"))
}

/// Уведомление-проверка из настроек.
pub fn notify_test(sound: bool) -> anyhow::Result<()> {
    use tauri_winrt_notification::{Duration, Sound, Toast};
    let mut toast = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(APP_NAME)
        .text1("Уведомления работают")
        .text2("Когда загрузка завершится, вы увидите такое сообщение.")
        .duration(Duration::Short);
    toast = toast.sound(if sound {
        Some(Sound::Default)
    } else {
        None
    });
    toast
        .show()
        .map_err(|e| anyhow::anyhow!("не удалось показать уведомление: {e:?}"))
}
