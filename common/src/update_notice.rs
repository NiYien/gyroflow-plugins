// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    attempted: Option<Instant>,
    in_flight: bool,
    available: Option<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

// Background work only publishes a result; host UI APIs stay on host callbacks.
pub fn available() -> Option<bool> {
    let state = STATE.get_or_init(|| Mutex::new(State::default()));
    let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
    let result = guard.available;
    if !guard.in_flight && guard.attempted.is_none_or(|t| t.elapsed() >= Duration::from_secs(300)) {
        guard.in_flight = true;
        guard.attempted = Some(Instant::now());
        if let Some(previous) = guard.worker.take() { let _ = previous.join(); }
        match std::thread::Builder::new().name("plugin-update-check".into()).spawn(move || {
            let result = fetch();
            let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
            guard.in_flight = false;
            match result {
                Ok(result) => {
                    log::info!("Plugin update check: current_ref={} available={result}", env!("NIYIEN_PLUGIN_SOURCE_REF"));
                    guard.available = Some(result);
                }
                Err(error) => log::debug!("Plugin update check failed: {error}"),
            }
        }) {
            Ok(worker) => guard.worker = Some(worker),
            Err(_) => guard.in_flight = false,
        }
    }
    result
}

pub fn shutdown() {
    let worker = STATE.get().and_then(|state| state.lock().unwrap_or_else(|e| e.into_inner()).worker.take());
    if let Some(worker) = worker { let _ = worker.join(); }
}

fn fetch() -> Result<bool, Box<dyn std::error::Error>> {
    let platform = match std::env::consts::OS { "macos" => "macos", "windows" => "windows", _ => "linux" };
    let endpoint = gyroflow_core::distribution::manifest_api();
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    let url = format!("{endpoint}{separator}platform={platform}&arch={}", std::env::consts::ARCH);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build().into();
    let body = agent.get(&url).call()?.into_body().with_config().limit(1024 * 1024).read_to_string()?;
    let manifest: serde_json::Value = serde_json::from_str(&body)?;
    Ok(is_newer(&manifest, env!("NIYIEN_PLUGIN_SOURCE_REF"), env!("NIYIEN_PLUGIN_VERSION")))
}

fn numeric_version(value: &str) -> Option<Vec<u64>> {
    let mut parts = value.trim().trim_start_matches('v').split('.')
        .map(str::parse::<u64>).collect::<Result<Vec<_>, _>>().ok()?;
    if parts.is_empty() { return None; }
    while parts.last() == Some(&0) { parts.pop(); }
    Some(parts)
}

fn is_newer(manifest: &serde_json::Value, current_ref: &str, current_version: &str) -> bool {
    let latest_ref = manifest["plugins_source_ref"].as_str().unwrap_or_default().trim();
    let tag = manifest["plugins_source_tag"].as_str().unwrap_or_default().trim();
    if current_ref == "dev" {
        return latest_ref.strip_prefix("actions-run-").and_then(|id| id.parse::<u64>().ok()).is_some_and(|id| id > 0)
            || numeric_version(tag).is_some() || numeric_version(latest_ref).is_some();
    }
    if !latest_ref.is_empty() && latest_ref == current_ref { return false; }
    if let (Some(latest), Some(current)) = (latest_ref.strip_prefix("actions-run-"), current_ref.strip_prefix("actions-run-")) {
        return matches!((latest.parse::<u64>(), current.parse::<u64>()), (Ok(a), Ok(b)) if a > b);
    }
    let latest_version = if numeric_version(tag).is_some() { tag } else { latest_ref };
    matches!((numeric_version(latest_version), numeric_version(current_version)), (Some(a), Some(b)) if a > b)
}

pub fn close_host_message(host: &str) -> String {
    crate::i18n::tf("update.close_host", &[("host", host)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn release_versions_and_builds() {
        for (latest, current, expected) in [("v2.1.3", "2.1.2.54", true), ("v2.1.2.55", "2.1.2.54", true), ("v2.1.2.0", "2.1.2", false), ("v2.1.1", "2.1.2", false), ("garbage", "2.1.2", false)] {
            assert_eq!(is_newer(&json!({"plugins_source_ref": latest}), "", current), expected);
        }
    }

    #[test]
    fn artifact_identity_and_missing_metadata() {
        for (latest, current, expected) in [("actions-run-101", "actions-run-100", true), ("actions-run-100", "actions-run-100", false), ("actions-run-99", "actions-run-100", false), ("actions-run-101", "", false)] {
            assert_eq!(is_newer(&json!({"plugins_source_ref": latest}), current, "2.1.2"), expected);
        }
        assert!(!is_newer(&json!({}), "", "2.1.2"));
        assert!(is_newer(&json!({"plugins_source_ref": "actions-run-101", "plugins_source_tag": "v2.1.3"}), "v2.1.2", "2.1.2"));
    }

    #[test]
    fn local_dev_offers_a_valid_published_plugin() {
        for manifest in [json!({"plugins_source_ref": "actions-run-101"}), json!({"plugins_source_ref": "v2.1.2"}), json!({"plugins_source_tag": "v2.1.2"})] {
            assert!(is_newer(&manifest, "dev", "2.1.2"));
        }
        for manifest in [json!({}), json!({"plugins_source_ref": "actions-run-0"}), json!({"plugins_source_ref": "actions-run-invalid"}), json!({"plugins_source_tag": "artifact-name"})] {
            assert!(!is_newer(&manifest, "dev", "2.1.2"));
        }
    }
}
