//! Language selection: English by default, Chinese when the system asks for
//! it, overridable from the top ribbon. Every user-facing string is wrapped
//! in [`tr`] with its English and Chinese forms; the active language picks one
//! at display time, so the same binary serves both.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set(l: Lang) {
    LANG.store(l as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 {
        Lang::Zh
    } else {
        Lang::En
    }
}

/// The English or Chinese form of a string, for the current language.
pub fn tr<'a>(en: &'a str, zh: &'a str) -> &'a str {
    match get() {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

/// Like [`tr`], but for format templates: both sides are formatted with the
/// same arguments and the active language's result is returned (owned).
/// Rust's `format!`/`bail!`/`println!` macros require a literal format
/// string, so use this inside them as `format!("{}", trfmt!("...{x}...", "...{x}...", x))`.
#[macro_export]
macro_rules! trfmt {
    ($en:literal, $zh:literal $(, $arg:expr)* $(,)?) => {{
        match $crate::lang::get() {
            $crate::lang::Lang::Zh => format!($zh $(, $arg)*),
            $crate::lang::Lang::En => format!($en $(, $arg)*),
        }
    }};
}

/// A sensible default: Chinese on a Simplified-Chinese Windows, English
/// otherwise.
pub fn system_default() -> Lang {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
        // The primary language id is the low 10 bits of the LANGID; 0x04 is
        // Chinese (both Simplified and Traditional land there, and Simplified
        // is by far the common case for this tool's users).
        if (unsafe { GetUserDefaultUILanguage() } & 0x3FF) == 0x04 {
            return Lang::Zh;
        }
    }
    Lang::En
}
