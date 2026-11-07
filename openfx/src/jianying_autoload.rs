// SPDX-License-Identifier: GPL-3.0-or-later

use gyroflow_plugin_base::{filesystem, serde_json};
use serde_json::Value;
use std::{collections::HashMap, fs, path::{Path, PathBuf}, time::UNIX_EPOCH};

#[derive(Default)]
pub(crate) struct State {
    stopped: bool,
    candidates: HashMap<String, Option<String>>,
}

impl State {
    pub(crate) fn candidate(&mut self, input: &str) -> Option<&str> {
        if self.stopped || input.is_empty() { return None; }
        self.candidates.entry(input.to_owned()).or_insert_with(|| resolve(input)).as_deref()
    }

    pub(crate) fn needs_source_bootstrap(&self) -> bool {
        !self.stopped && self.candidates.is_empty()
    }

    pub(crate) fn stop(&mut self) {
        self.stopped = true;
        self.candidates.clear();
    }
}

fn local_path(value: &str) -> Option<PathBuf> {
    let path = PathBuf::from(filesystem::url_to_path(value));
    path.is_absolute().then_some(path)
}

fn same_path(left: &Path, right: &Path) -> Option<bool> {
    Some(fs::canonicalize(left).ok()? == fs::canonicalize(right).ok()?)
}

fn canonical_json(mut value: Value) -> Option<Vec<u8>> {
    // The workspace may enable serde_json's preserve_order feature through another crate.
    value.sort_all_objects();
    serde_json::to_vec(&value).ok()
}

#[cfg(target_os = "windows")]
fn md5_hex(bytes: &[u8]) -> Option<String> {
    use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_MD5_ALG_HANDLE};
    let mut digest = [0u8; 16];
    unsafe { BCryptHash(BCRYPT_MD5_ALG_HANDLE, None, bytes, &mut digest) }.ok().ok()?;
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(target_os = "macos")]
fn md5_hex(bytes: &[u8]) -> Option<String> {
    #[link(name = "System")]
    unsafe extern "C" {
        fn CC_MD5(data: *const std::ffi::c_void, length: u32, digest: *mut u8) -> *mut u8;
    }
    let length = u32::try_from(bytes.len()).ok()?;
    let mut digest = [0u8; 16];
    if unsafe { CC_MD5(bytes.as_ptr().cast(), length, digest.as_mut_ptr()) }.is_null() { return None; }
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn md5_hex(_bytes: &[u8]) -> Option<String> { None }

fn metadata_matches(metadata: &fs::Metadata, sidecar: &Value, size: &str, modified: &str) -> Option<bool> {
    Some(metadata.is_file()
        && metadata.len() == sidecar.get(size)?.as_u64()?
        && metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() == sidecar.get(modified)?.as_u64()?)
}

fn resolve(input: &str) -> Option<String> {
    let input_path = local_path(input)?;
    let input_metadata = fs::metadata(&input_path).ok()?;
    if !input_metadata.is_file() { return None; }
    let agency_path = input_path.with_extension("agency");
    let source = if agency_path.try_exists().ok()? {
        let sidecar: Value = serde_json::from_slice(&fs::read(&agency_path).ok()?).ok()?;
        let data = sidecar.get("data")?;
        data.as_object()?;
        if md5_hex(&canonical_json(data.clone())?)? != sidecar.get("verify")?.as_str()? { return None; }
        let target = agency_path.parent()?.join(filesystem::url_to_path(data.get("target_path")?.as_str()?));
        if !same_path(&target, &input_path)? { return None; }
        let source = local_path(data.get("source_path")?.as_str()?)?;
        let source_metadata = fs::metadata(&source).ok()?;
        if !metadata_matches(&source_metadata, &sidecar, "source_size", "source_modifyTime")?
            || !metadata_matches(&input_metadata, &sidecar, "target_size", "target_modifyTime")? {
            return None;
        }
        source
    } else {
        input_path
    };
    let project_path = source.with_extension("gyroflow");
    let project: Value = serde_json::from_slice(&fs::read(&project_path).ok()?).ok()?;
    project.as_object()?;
    // Use the shared OFX lookup after validating the exact-named sidecar. Its saved
    // media path may refer to an old location after the video and project were copied.
    gyroflow_plugin_base::GyroflowPluginBase::get_project_path(source.to_str()?)
}

#[cfg(all(test, any(target_os = "windows", target_os = "macos")))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/jianying-autoload-tests")
            .join(format!("{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&folder).unwrap();
        fs::canonicalize(folder).unwrap()
    }

    fn project_for(source: &Path, videofile: &str) -> PathBuf {
        let project = source.with_extension("gyroflow");
        fs::write(&project, serde_json::to_vec(&serde_json::json!({"videofile": videofile})).unwrap()).unwrap();
        project
    }

    fn sidecar_for(input: &Path, source: &Path) -> Value {
        let data = serde_json::json!({"target_path": input.file_name().unwrap().to_str().unwrap(), "source_path": source.to_str().unwrap()});
        let source_metadata = fs::metadata(source).unwrap();
        let target_metadata = fs::metadata(input).unwrap();
        serde_json::json!({
            "verify": md5_hex(&canonical_json(data.clone()).unwrap()).unwrap(), "data": data,
            "source_size": source_metadata.len(), "target_size": target_metadata.len(),
            "source_modifyTime": source_metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs(),
            "target_modifyTime": target_metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs()
        })
    }

    #[test]
    fn jianying_autoload_canonical_data_and_known_md5_vectors() {
        assert_eq!(md5_hex(b"").as_deref(), Some("d41d8cd98f00b204e9800998ecf8427e"));
        assert_eq!(md5_hex(b"abc").as_deref(), Some("900150983cd24fb0d6963f7d28e17f72"));
        let data = serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":"中文"}"#).unwrap();
        let bytes = canonical_json(data).unwrap();
        assert_eq!(bytes, "{\"a\":\"中文\",\"z\":{\"a\":1,\"b\":2}}".as_bytes());
        // This digest was independently calculated from the fixed UTF-8 bytes with Python hashlib.
        assert_eq!(md5_hex(&bytes).as_deref(), Some("85a642c381409f0c7bd1a2b6b0f83b97"));
    }

    #[test]
    fn jianying_autoload_direct_file_url_and_empty_retry() {
        let folder = fixture("direct-url");
        let source = folder.join("原片 空格.MP4");
        fs::write(&source, b"source").unwrap();
        let project = project_for(&source, &filesystem::path_to_url(source.to_str().unwrap()));
        let mut state = State::default();
        assert!(state.candidate("").is_none());
        let source_url = filesystem::path_to_url(source.to_str().unwrap());
        assert!(source_url.starts_with("file:///"));
        assert!(source_url.contains("%20"));
        let candidate = state.candidate(&source_url).expect("The exact project for the file URL must resolve");
        // A file URL preserves the source identity across native path representations.
        assert_eq!(fs::canonicalize(candidate).unwrap(), fs::canonicalize(project).unwrap());
    }

    #[test]
    fn jianying_autoload_requires_exact_readable_project() {
        let folder = fixture("exact-only");
        let source = folder.join("clip.mp4");
        let other = folder.join("other.mp4");
        fs::write(&source, b"source").unwrap();
        fs::write(&other, b"other").unwrap();
        fs::write(folder.join("clip-final.gyroflow"), serde_json::to_vec(&serde_json::json!({"videofile": source})).unwrap()).unwrap();
        assert!(State::default().candidate(source.to_str().unwrap()).is_none());
        let project = project_for(&source, other.to_str().unwrap());
        assert_eq!(State::default().candidate(source.to_str().unwrap()), project.to_str());
        for invalid in [b"invalid JSON".as_slice(), b"null", b"[]"] {
            fs::write(&project, invalid).unwrap();
            assert!(State::default().candidate(source.to_str().unwrap()).is_none());
        }
    }

    #[test]
    fn jianying_autoload_same_name_project_survives_media_move() {
        let folder = fixture("moved-media");
        let source = folder.join("原片 空格.MOV");
        fs::write(&source, b"moved source").unwrap();
        let missing = folder.join("old location").join(source.file_name().unwrap());
        assert!(!missing.exists());
        let project = project_for(&source, &filesystem::path_to_url(missing.to_str().unwrap()));
        let before = fs::read(&project).unwrap();
        let source_url = filesystem::path_to_url(source.to_str().unwrap());
        let mut state = State::default();
        let candidate = state.candidate(&source_url).expect("The moved media's exact project must resolve");
        assert_eq!(fs::canonicalize(candidate).unwrap(), fs::canonicalize(&project).unwrap());
        assert_eq!(gyroflow_plugin_base::GyroflowPluginBase::get_project_path(source.to_str().unwrap()),
                   Some(project.to_str().unwrap().to_owned()));
        assert_eq!(fs::read(&project).unwrap(), before);
    }

    #[test]
    fn jianying_autoload_caches_each_nonempty_source_and_stops() {
        let folder = fixture("cache");
        let first = folder.join("first.mp4");
        let second = folder.join("second.mp4");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let mut state = State::default();
        assert!(state.candidate(first.to_str().unwrap()).is_none());
        project_for(&first, first.to_str().unwrap());
        let second_project = project_for(&second, second.to_str().unwrap());
        assert_eq!(state.candidate(second.to_str().unwrap()), second_project.to_str());
        assert!(state.candidate(first.to_str().unwrap()).is_none());
        fs::write(&second_project, b"now invalid").unwrap();
        assert_eq!(state.candidate(second.to_str().unwrap()), second_project.to_str());
        state.stop();
        assert!(state.candidate(second.to_str().unwrap()).is_none());
        assert!(state.candidates.is_empty());
    }

    #[test]
    fn jianying_autoload_validates_proxy_sidecar_and_metadata() {
        let folder = fixture("agency");
        let source = folder.join("原片.MP4");
        let input = folder.join("proxy.mp4");
        fs::write(&source, b"original source").unwrap();
        fs::write(&input, b"proxy").unwrap();
        let project = project_for(&source, source.to_str().unwrap());
        let sidecar = sidecar_for(&input, &source);
        let sidecar_path = input.with_extension("agency");
        fs::write(&sidecar_path, serde_json::to_vec(&sidecar).unwrap()).unwrap();
        assert_eq!(State::default().candidate(input.to_str().unwrap()), project.to_str());
        for field in ["source_size", "target_size", "source_modifyTime", "target_modifyTime"] {
            let mut changed = sidecar.clone();
            changed[field] = (changed[field].as_u64().unwrap() + 1).into();
            fs::write(&sidecar_path, serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(State::default().candidate(input.to_str().unwrap()).is_none(), "{field}");
        }
        let mut changed = sidecar.clone();
        changed["verify"] = "00000000000000000000000000000000".into();
        fs::write(&sidecar_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(State::default().candidate(input.to_str().unwrap()).is_none());
        let other = folder.join("other-proxy.mp4");
        fs::write(&other, b"proxy").unwrap();
        let mut changed = sidecar;
        changed["data"]["target_path"] = "other-proxy.mp4".into();
        changed["verify"] = md5_hex(&canonical_json(changed["data"].clone()).unwrap()).unwrap().into();
        fs::write(&sidecar_path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(State::default().candidate(input.to_str().unwrap()).is_none());
    }

    #[test]
    fn jianying_autoload_present_invalid_sidecar_prevents_direct_fallback() {
        let folder = fixture("invalid-agency");
        let source = folder.join("clip.mp4");
        fs::write(&source, b"source").unwrap();
        project_for(&source, source.to_str().unwrap());
        fs::write(source.with_extension("agency"), b"invalid JSON").unwrap();
        assert!(State::default().candidate(source.to_str().unwrap()).is_none());
    }
}
