//! Щадящий режим: клиент не отбирает процессор у игр, Discord и звука.
//!
//! Скорость загрузки от этого не страдает: сеть и диск — не процессор,
//! а приоритет ниже обычного лишь означает, что при конкуренции за ядра
//! система сначала обслуживает другие программы (в том числе аудиопотоки).

use std::sync::atomic::{AtomicU8, Ordering};

/// Приоритет процесса.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Обычный (щадящий режим выключен).
    Normal,
    /// Ниже обычного: активное окно, но другие программы важнее.
    Below,
    /// Самый низкий: окно свёрнуто, клиент работает в фоне.
    Idle,
}

static APPLIED: AtomicU8 = AtomicU8::new(UNKNOWN);

const UNKNOWN: u8 = u8::MAX;
const NORMAL: u8 = 0;
const BELOW: u8 = 1;
const IDLE: u8 = 2;

impl Level {
    fn code(self) -> u8 {
        match self {
            Level::Normal => NORMAL,
            Level::Below => BELOW,
            Level::Idle => IDLE,
        }
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn class(self) -> u32 {
        match self {
            Level::Normal => 0x0000_0020,      // NORMAL_PRIORITY_CLASS
            Level::Below => 0x0000_4000,       // BELOW_NORMAL_PRIORITY_CLASS
            Level::Idle => 0x0000_0040,        // IDLE_PRIORITY_CLASS
        }
    }
}

/// Применить уровень (повторные вызовы с тем же уровнем ничего не делают).
pub fn apply(level: Level) {
    let code = level.code();
    if APPLIED.load(Ordering::Relaxed) == code {
        return;
    }
    APPLIED.store(code, Ordering::Relaxed);
    set_process_class(level.class());
}

#[cfg(windows)]
fn set_process_class(class: u32) {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn SetPriorityClass(process: *mut c_void, class: u32) -> i32;
    }

    // Не критично, если система откажет (например, из-за политик) —
    // клиент просто продолжит работать с обычным приоритетом.
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), class);
    }
}

#[cfg(not(windows))]
fn set_process_class(_class: u32) {}

/// Уровень, который нужен сейчас.
pub fn desired(enabled: bool, minimized: bool) -> Level {
    if !enabled {
        Level::Normal
    } else if minimized {
        Level::Idle
    } else {
        Level::Below
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_have_distinct_codes() {
        assert_ne!(Level::Normal.code(), Level::Below.code());
        assert_ne!(Level::Below.code(), Level::Idle.code());
        assert_ne!(Level::Normal.code(), Level::Idle.code());
    }

    #[test]
    fn desired_levels() {
        assert!(matches!(desired(false, false), Level::Normal));
        assert!(matches!(desired(true, false), Level::Below));
        assert!(matches!(desired(true, true), Level::Idle));
    }
}
