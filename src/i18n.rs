// Language switch shared by every WinRust program (an identical copy of this file lives in each crate).
// English by default; Russian when %ProgramData%\WinRust\lang.txt contains "ru" (the installer's EN/RU button writes it).
// Programs read the file once at start; the one that flips the switch updates its own texts at once.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU8, Ordering::Relaxed},
};

static LANG: AtomicU8 = AtomicU8::new(0); // 0 = not read yet, 1 = English, 2 = Russian

fn file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("WinRust").join("lang.txt"))
}

#[allow(dead_code)]
pub fn english() -> bool {
    match LANG.load(Relaxed) {
        1 => true,
        2 => false,
        _ => {
            // unit tests keep their Russian expectations
            let ru = cfg!(test) || file().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|s| s.trim().eq_ignore_ascii_case("ru"));
            LANG.store(if ru { 2 } else { 1 }, Relaxed);
            !ru
        }
    }
}

/// Switches this process and writes the choice for the other programs (they pick it up when they start).
#[allow(dead_code)]
pub fn set_english(en: bool) {
    LANG.store(if en { 1 } else { 2 }, Relaxed);
    if let Some(f) = file() {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        let _ = std::fs::write(f, if en { "en" } else { "ru" });
    }
}

/// `t!("русский", "english")`: a text in the current language.
#[allow(unused_macros)]
macro_rules! t {
    ($ru:expr, $en:expr) => {
        if $crate::i18n::english() { $en } else { $ru }
    };
}

/// `tf!("русский {}", "english {}", args...)`: format! in the current language (both templates are checked at compile time).
#[allow(unused_macros)]
macro_rules! tf {
    ($ru:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::i18n::english() { format!($en $(, $a)*) } else { format!($ru $(, $a)*) }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn tests_run_in_russian() {
        assert_eq!(t!("да", "yes"), "да");
        assert_eq!(tf!("{} шт", "{} pcs", 3), "3 шт");
    }
}
