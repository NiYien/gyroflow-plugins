// common/src/i18n.rs

use std::collections::HashMap;
use std::sync::OnceLock;

#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::t($key)
    };
}

/// Map a raw OS / env locale string to one of the supported locale codes.
/// Falls back to "en" for any unrecognized locale.
pub fn normalize_locale(raw: &str) -> &'static str {
    let lower = raw.to_ascii_lowercase();
    let primary = lower
        .split(|c: char| c == '-' || c == '_')
        .next()
        .unwrap_or("");

    match primary {
        "zh" => {
            if lower.contains("hant")
                || lower.contains("-tw") || lower.contains("_tw")
                || lower.contains("-hk") || lower.contains("_hk")
                || lower.contains("-mo") || lower.contains("_mo")
            {
                "zh-TW"
            } else {
                "zh-CN"
            }
        }
        "ko" => "ko",
        "ja" => "ja",
        "ru" => "ru",
        "en" => "en",
        _ => "en",
    }
}

struct I18nState {
    locale: &'static str,
    table: HashMap<&'static str, &'static str>,
}

static STATE: OnceLock<I18nState> = OnceLock::new();

/// Initialize the i18n table. Idempotent — safe to call multiple times,
/// only the first call has effect.
pub fn init() {
    STATE.get_or_init(|| {
        let raw = std::env::var("GYROFLOW_PLUGIN_LANG")
            .ok()
            .or_else(sys_locale::get_locale)
            .unwrap_or_default();
        let locale = normalize_locale(&raw);

        let json = match locale {
            "en"    => include_str!("../locales/en.json"),
            "zh-CN" => include_str!("../locales/zh-CN.json"),
            "zh-TW" => include_str!("../locales/zh-TW.json"),
            "ja"    => include_str!("../locales/ja.json"),
            "ko"    => include_str!("../locales/ko.json"),
            "ru"    => include_str!("../locales/ru.json"),
            _       => include_str!("../locales/en.json"),
        };

        let parsed: HashMap<String, String> = serde_json::from_str(json)
            .expect("locale JSON parse failed (build.rs should have caught this)");

        let table = parsed
            .into_iter()
            .map(|(k, v)| {
                let k_static: &'static str = Box::leak(k.into_boxed_str());
                let v_static: &'static str = Box::leak(v.into_boxed_str());
                (k_static, v_static)
            })
            .collect();

        I18nState { locale, table }
    });
}

/// Look up a translation key. Returns the translated string for the active
/// locale. Panics on missing key (build.rs prevents this from happening
/// in any committed state).
pub fn t(key: &str) -> &'static str {
    init();
    let state = STATE.get().expect("STATE init");
    state.table.get(key).copied().unwrap_or_else(|| {
        panic!("i18n: missing key {key:?} in locale {:?}", state.locale)
    })
}

fn substitute_placeholders(template: &str, args: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (name, value) in args {
        let placeholder = format!("{{{name}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

/// Look up a translation key and substitute `{name}` placeholders.
/// Returns an owned String (since the result is dynamic).
pub fn tf(key: &str, args: &[(&str, &str)]) -> String {
    substitute_placeholders(t(key), args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_variants() {
        assert_eq!(normalize_locale("en"), "en");
        assert_eq!(normalize_locale("en-US"), "en");
        assert_eq!(normalize_locale("en_GB"), "en");
        assert_eq!(normalize_locale("EN"), "en");
    }

    #[test]
    fn simplified_chinese() {
        assert_eq!(normalize_locale("zh"), "zh-CN");
        assert_eq!(normalize_locale("zh-CN"), "zh-CN");
        assert_eq!(normalize_locale("zh_CN"), "zh-CN");
        assert_eq!(normalize_locale("zh-Hans"), "zh-CN");
        assert_eq!(normalize_locale("zh-Hans-CN"), "zh-CN");
        assert_eq!(normalize_locale("zh-SG"), "zh-CN");
    }

    #[test]
    fn traditional_chinese() {
        assert_eq!(normalize_locale("zh-TW"), "zh-TW");
        assert_eq!(normalize_locale("zh_TW"), "zh-TW");
        assert_eq!(normalize_locale("zh-Hant"), "zh-TW");
        assert_eq!(normalize_locale("zh-Hant-TW"), "zh-TW");
        assert_eq!(normalize_locale("zh-HK"), "zh-TW");
        assert_eq!(normalize_locale("zh-MO"), "zh-TW");
    }

    #[test]
    fn other_supported() {
        assert_eq!(normalize_locale("ja"), "ja");
        assert_eq!(normalize_locale("ja-JP"), "ja");
        assert_eq!(normalize_locale("ko"), "ko");
        assert_eq!(normalize_locale("ko-KR"), "ko");
        assert_eq!(normalize_locale("ru"), "ru");
        assert_eq!(normalize_locale("ru-RU"), "ru");
        assert_eq!(normalize_locale("ru-BY"), "ru");
    }

    #[test]
    fn fallback_to_english() {
        assert_eq!(normalize_locale(""), "en");
        assert_eq!(normalize_locale("fr"), "en");
        assert_eq!(normalize_locale("de-DE"), "en");
        assert_eq!(normalize_locale("xyz"), "en");
    }

    /// init() and t() share state via OnceLock; this test relies on
    /// being the only test that touches that state, so we set the env
    /// var before any other code can call init().
    /// Place it in this single combined test rather than splitting.
    #[test]
    fn init_and_lookup() {
        // Note: env var must be set before init() is first called anywhere
        // in the test binary. Cargo runs each #[test] in the same process,
        // so this works only because no other test invokes init().
        // SAFETY: single-threaded test context; no other thread reads env vars here.
        unsafe { std::env::set_var("GYROFLOW_PLUGIN_LANG", "en"); }
        init();
        assert_eq!(t("status.ok"), "OK");
        assert!(!t("label.smoothness").is_empty());
    }

    #[test]
    fn substitute_simple() {
        assert_eq!(
            substitute_placeholders("Hello {name}", &[("name", "world")]),
            "Hello world"
        );
    }

    #[test]
    fn substitute_multiple() {
        assert_eq!(
            substitute_placeholders("{a} and {b}", &[("a", "1"), ("b", "2")]),
            "1 and 2"
        );
    }

    #[test]
    fn substitute_missing_arg_keeps_placeholder() {
        // If caller forgets to pass an argument, the placeholder stays
        // visible rather than panicking.
        assert_eq!(
            substitute_placeholders("Hello {name}", &[]),
            "Hello {name}"
        );
    }

    #[test]
    fn substitute_no_placeholders() {
        assert_eq!(substitute_placeholders("plain text", &[]), "plain text");
    }
}
