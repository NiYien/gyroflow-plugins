use std::collections::BTreeSet;

const LOCALES: &[&str] = &["en", "zh-CN", "zh-TW", "ja", "ko", "ru"];

fn main() {
    println!("cargo:rerun-if-changed=locales");

    let read = |name: &str| -> serde_json::Map<String, serde_json::Value> {
        let path = format!("locales/{name}.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let value: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("parse {path}: {e}"));
        match value {
            serde_json::Value::Object(m) => m,
            _ => panic!("{path}: expected JSON object at top level"),
        }
    };

    let en = read("en");
    let en_keys: BTreeSet<&str> = en.keys().map(String::as_str).collect();

    for &lang in LOCALES.iter().filter(|&&l| l != "en") {
        let other = read(lang);
        let other_keys: BTreeSet<&str> = other.keys().map(String::as_str).collect();

        let missing: Vec<&&str> = en_keys.difference(&other_keys).collect();
        let extra: Vec<&&str> = other_keys.difference(&en_keys).collect();
        if !missing.is_empty() || !extra.is_empty() {
            panic!(
                "locale {lang} key mismatch with en\n  missing in {lang}: {missing:?}\n  extra in {lang}: {extra:?}"
            );
        }

        for (k, v_en) in en.iter() {
            let v_en_str = v_en
                .as_str()
                .unwrap_or_else(|| panic!("en.json {k:?}: value not a string"));
            let v_other_str = other[k]
                .as_str()
                .unwrap_or_else(|| panic!("{lang}.json {k:?}: value not a string"));
            for ph in extract_placeholders(v_en_str) {
                let token = format!("{{{ph}}}");
                if !v_other_str.contains(&token) {
                    panic!(
                        "locale {lang} key {k:?}: missing placeholder {token}\n  en: {v_en_str:?}\n  {lang}: {v_other_str:?}"
                    );
                }
            }
        }
    }
}

fn extract_placeholders(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j < bytes.len() {
                let name = std::str::from_utf8(&bytes[i + 1..j])
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    result.push(name);
                }
                i = j + 1;
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
    result
}
