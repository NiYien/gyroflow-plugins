use base64::{Engine as _, engine::general_purpose::STANDARD};
use gyroflow_finalcut::{
    GF_ROUTE_D_PROJECT_INPUT_AVAILABLE, GFError, GFRouteDPatchResult, GFRouteDProjectInput,
    GFStatus, GFTime, GFTimeRange, gf_finalcut_error_free, gf_finalcut_instance_create,
    gf_finalcut_instance_free, gf_finalcut_instance_load_project,
    gf_finalcut_instance_load_timing_payload, gf_finalcut_instance_resolve_source_time,
    gf_finalcut_owned_bytes_free, gf_finalcut_project_payload_encode,
    gf_finalcut_route_d_batch_patch, gf_finalcut_route_d_batch_patch_with_project_inputs,
    gf_finalcut_route_d_patch, gf_finalcut_route_d_patch_result_free, patch_fcpxml_project,
    patch_fcpxml_project_batch, patch_fcpxml_project_batch_with_media_roots,
    patch_fcpxml_project_batch_with_project_reader,
    patch_fcpxml_project_batch_with_project_reader_report_all_skipped,
};
use sha2::{Digest, Sha256};

const EFFECT_UUID: &str = "ABAD71F5-23F5-46F6-AB08-C11603168AA4";
const EFFECT_TEMPLATE_UID: &str = "~/Effects.localized/NiYien/Gyroflow/Gyroflow NiYien.moef";

fn asset_project(start: &str, duration: &str, time_map: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.14">
  <resources>
    <format id="r1" frameDuration="1001/30000s"/>
    <asset id="r2" start="3600s" duration="30s" format="r1"/>
    <effect id="fx" name="Gyroflow NiYien" uid="{EFFECT_UUID}"/>
    <effect id="other" name="Other" uid="other-effect"/>
  </resources>
  <project name="Original" uid="11111111-1111-4111-8111-111111111111">
    <sequence format="r1" duration="{duration}">
      <spine>
        <asset-clip ref="r2" offset="0s" start="{start}" duration="{duration}">
          {time_map}
          <metadata key="must-survive" value="yes"/>
          <filter-video ref="fx"><param name="Instance Identity" value="duplicate"/><param name="Timing Payload" value=""/></filter-video>
          <filter-video ref="fx"><param name="Instance Identity" value="duplicate"/><param name="Timing Payload" value=""/></filter-video>
          <filter-video ref="other" name="Other"/>
        </asset-clip>
      </spine>
    </sequence>
  </project>
</fcpxml>
"#
    )
}

fn payloads(xml: &[u8]) -> Vec<serde_json::Value> {
    let text = std::str::from_utf8(xml).unwrap();
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    document
        .descendants()
        .filter(|node| node.has_tag_name("filter-video") && node.attribute("ref") == Some("fx"))
        .map(|filter| {
            let encoded = filter
                .children()
                .find(|node| {
                    node.has_tag_name("param") && node.attribute("name") == Some("Timing Payload")
                })
                .and_then(|node| node.attribute("value"))
                .unwrap();
            serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap()
        })
        .collect()
}

fn encoded_timing_payloads(xml: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(xml).unwrap();
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    document
        .descendants()
        .filter(|node| node.has_tag_name("filter-video") && node.attribute("ref") == Some("fx"))
        .map(|filter| {
            filter
                .children()
                .find(|node| {
                    node.has_tag_name("param") && node.attribute("name") == Some("Timing Payload")
                })
                .and_then(|node| node.attribute("value"))
                .unwrap()
                .to_string()
        })
        .collect()
}

fn banked_project_parameters(bank: char, payload: &str, generation: u64) -> String {
    let (manifest_id, chunk_id) = match bank {
        'A' => (1904, 1910),
        'B' => (1905, 1930),
        _ => panic!("unknown bank"),
    };
    let manifest = serde_json::json!({
        "version": 1,
        "generation": generation,
        "chunk_count": 1,
        "encoded_length": payload.len(),
        "payload_sha256": format!("{:x}", Sha256::digest(payload.as_bytes())),
    });
    let encoded_manifest = STANDARD.encode(serde_json::to_vec(&manifest).unwrap());
    format!(
        "<param name=\"Project Payload Manifest {bank}\" key=\"9999/10013/10016/3/10036/{manifest_id}\" value=\"{encoded_manifest}\"/><param name=\"Project Payload {bank} 01\" key=\"9999/10013/10016/3/10036/{chunk_id}\" value=\"{payload}\"/>"
    )
}

fn encoded_project_payload(project: &[u8]) -> String {
    let mut payload = gyroflow_finalcut::GFOwnedBytes::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_project_payload_encode(
                project.as_ptr(),
                project.len(),
                &mut payload,
                &mut error,
            )
        },
        GFStatus::Ok
    );
    assert!(error.is_null());
    let encoded = unsafe { std::slice::from_raw_parts(payload.data, payload.len) };
    let encoded = std::str::from_utf8(encoded).unwrap().to_string();
    unsafe { gf_finalcut_owned_bytes_free(&mut payload) };
    encoded
}

fn selected_banked_payloads(xml: &[u8], effect_ref: &str) -> Vec<String> {
    let document = roxmltree::Document::parse_with_options(
        std::str::from_utf8(xml).unwrap(),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    document
        .descendants()
        .filter(|node| {
            node.has_tag_name("filter-video") && node.attribute("ref") == Some(effect_ref)
        })
        .filter_map(|filter| {
            let manifest = filter
                .children()
                .find(|node| {
                    node.has_tag_name("param")
                        && node.attribute("name") == Some("Project Payload Manifest A")
                })
                .and_then(|node| node.attribute("value"))?;
            let manifest: serde_json::Value =
                serde_json::from_slice(&STANDARD.decode(manifest).unwrap()).unwrap();
            let chunk_count = manifest["chunk_count"].as_u64().unwrap() as usize;
            Some(
                (0..chunk_count)
                    .map(|index| {
                        let name = format!("Project Payload A {:02}", index + 1);
                        filter
                            .children()
                            .find(|node| {
                                node.has_tag_name("param")
                                    && node.attribute("name") == Some(name.as_str())
                            })
                            .and_then(|node| node.attribute("value"))
                            .unwrap()
                    })
                    .collect::<String>(),
            )
        })
        .collect()
}

fn exact_sibling_batch_input(assets: &str, clips: &str) -> String {
    format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="r1" frameDuration="1/30s"/>{assets}<effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Exact Sibling" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="2s"><spine>
        {clips}</spine></sequence></project></fcpxml>"#
    )
}

fn run_route_d_output_verifier(
    original: &std::path::Path,
    output: &std::path::Path,
    project: &std::path::Path,
    route: &str,
    expected_parameters: Option<&str>,
) -> std::process::Output {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts/verify_finalcut_route_d_output.py");
    let mut command = std::process::Command::new("python3");
    command
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(script)
        .arg("--original-fcpxml")
        .arg(original)
        .arg("--fcpxml")
        .arg(output)
        .arg("--expect-occurrence")
        .arg(format!("{route}={}", project.display()));
    if let Some(parameters) = expected_parameters {
        command
            .arg("--expect-parameters")
            .arg(format!("{route}={parameters}"));
    }
    command.output().unwrap()
}

fn geometry_batch_input(root: &std::path::Path, geometry: &str) -> String {
    std::fs::write(
        root.join("Geometry.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    std::fs::write(
        root.join("Control.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let media_url = format!("file://{}", root.join("Geometry.mov").display());
    let control_url = format!("file://{}", root.join("Control.mov").display());
    format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="source" frameDuration="1/30s" width="1920" height="1080" paspH="4" paspV="3"/>
        <format id="control-format" frameDuration="1/30s" width="1920" height="1080"/>
        <format id="sequence" frameDuration="1/30s" width="1080" height="1920"/>
        <asset id="a" start="0s" duration="1s" format="source"><media-rep kind="original-media" src="{media_url}"/></asset>
        <asset id="control" start="0s" duration="1s" format="control-format"><media-rep kind="original-media" src="{control_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Geometry" uid="11111111-1111-4111-8111-111111111111"><sequence format="sequence" duration="1s"><spine>
        <asset-clip name="Geometry" ref="a" offset="0s" start="0s" duration="1s">{geometry}<filter-video ref="fx"/></asset-clip>
        <asset-clip name="Control" ref="control" lane="1" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>
        </spine></sequence></project></fcpxml>"#
    )
}

fn geometry_target_report(result: &gyroflow_finalcut::BatchRouteDPatchResult) -> serde_json::Value {
    serde_json::to_value(&result.targets[0]).unwrap()
}

fn geometry_skip_detail(input: &str) -> String {
    patch_fcpxml_project_batch(input.as_bytes())
        .unwrap()
        .targets[0]
        .detail
        .clone()
}

#[test]
fn geometry_preflight_reports_runtime_live_and_does_not_bake_editorial_geometry() {
    let unique = format!(
        "gyroflow-finalcut-geometry-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let geometry = r#"<adjust-conform type="fill"/><adjust-transform position="10 -20" scale="1.25 1.25" rotation="90" anchor="0 0"><param name="position" value="10 -20"><keyframeAnimation><keyframe time="0s" value="10 -20"/><keyframe time="1s" value="30 40"/></keyframeAnimation></param><param name="scale" value="1.25 1.25"><keyframeAnimation><keyframe time="0s" value="1.25 1.25"/><keyframe time="1s" value="1.5 1.5"/></keyframeAnimation></param><param name="rotation" value="90"><keyframeAnimation><keyframe time="0s" value="90"/><keyframe time="1s" value="270"/></keyframeAnimation></param></adjust-transform><adjust-crop mode="trim"><trim-rect left="1" top="2" right="3" bottom="4"><param name="left" value="1"><keyframeAnimation><keyframe time="0s" value="1" curve="linear"/><keyframe time="1s" value="2" curve="linear"/></keyframeAnimation></param></trim-rect></adjust-crop>"#;
    let first_input = geometry_batch_input(&root, geometry);
    let first = patch_fcpxml_project_batch(first_input.as_bytes()).unwrap();
    let report = geometry_target_report(&first);

    assert_eq!(report["geometry_status"], "runtime_live");
    assert_eq!(report["geometry_reasons"], serde_json::json!([]));
    let detail = report["geometry_detail"].as_str().unwrap();
    assert!(
        detail.contains("asset_format=1920x1080 pasp=4/3"),
        "{detail}"
    );
    assert!(
        detail.contains("sequence_format=1080x1920 pasp=1/1"),
        "{detail}"
    );
    assert!(detail.contains("conform=fill"), "{detail}");
    assert!(detail.contains("transform=animated"), "{detail}");
    assert!(detail.contains("crop=trim_animated"), "{detail}");

    let first_output = std::str::from_utf8(&first.xml).unwrap();
    assert!(first_output.contains(geometry));
    let first_timing = encoded_timing_payloads(&first.xml);
    let first_projects = selected_banked_payloads(&first.xml, "fx");
    for payload in payloads(&first.xml) {
        assert!(payload.get("geometry").is_none());
        assert!(payload.get("fov").is_none());
        assert!(payload.get("zoom_mode").is_none());
    }

    let changed_geometry = geometry
        .replace("type=\"fill\"", "type=\"none\"")
        .replace("position=\"10 -20\"", "position=\"100 200\"")
        .replace("scale=\"1.25 1.25\"", "scale=\"2 2\"")
        .replace("left=\"1\"", "left=\"11\"");
    let changed_input = geometry_batch_input(&root, &changed_geometry);
    let changed = patch_fcpxml_project_batch(changed_input.as_bytes()).unwrap();

    assert_eq!(encoded_timing_payloads(&changed.xml), first_timing);
    assert_eq!(selected_banked_payloads(&changed.xml, "fx"), first_projects);
    assert_eq!(
        geometry_target_report(&changed)["geometry_status"],
        "runtime_live"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_accepts_fit_fill_none_and_crop_trim_ken_burns_forms() {
    let unique = format!(
        "gyroflow-finalcut-geometry-forms-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let cases = [
        (
            "fit",
            r#"<adjust-crop mode="crop"><crop-rect left="1" top="2" right="3" bottom="4"/></adjust-crop>"#,
            "crop=crop_static",
        ),
        (
            "fill",
            r#"<adjust-crop mode="trim"><trim-rect left="1" top="2" right="3" bottom="4"/></adjust-crop>"#,
            "crop=trim_static",
        ),
        (
            "none",
            r#"<adjust-crop mode="pan"><pan-rect left="0" top="0" right="10" bottom="10"/><pan-rect left="10" top="10" right="0" bottom="0"/></adjust-crop>"#,
            "crop=ken_burns",
        ),
    ];
    for (conform, crop, expected_crop) in cases {
        let geometry = format!(
            r#"<adjust-conform type="{conform}"/><adjust-transform position="0 0" scale="1 1" rotation="270" anchor="0 0"/>{crop}"#
        );
        let input = geometry_batch_input(&root, &geometry);
        let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
        let report = geometry_target_report(&patched);
        assert_eq!(report["geometry_status"], "runtime_live");
        let detail = report["geometry_detail"].as_str().unwrap();
        assert!(detail.contains(&format!("conform={conform}")), "{detail}");
        assert!(detail.contains(expected_crop), "{detail}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_uses_verified_simplified_chinese_names_with_opaque_keys() {
    let unique = format!(
        "gyroflow-finalcut-geometry-localized-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let geometry = r#"<adjust-transform position="0 0" scale="1 1" rotation="0" anchor="0 0"><param name="位置" key="opaque/transform/1" value="0 0"><keyframeAnimation><keyframe time="0s" value="0 0" curve="linear"/><keyframe time="1s" value="10 20" curve="linear"/></keyframeAnimation></param><param name="缩放" key="opaque/transform/2" value="1 1"><keyframeAnimation><keyframe time="0s" value="1 1" curve="linear"/><keyframe time="1s" value="1.25 1.25" curve="linear"/></keyframeAnimation></param><param name="旋转" key="opaque/transform/3" value="0"><keyframeAnimation><keyframe time="0s" value="0" curve="linear"/><keyframe time="1s" value="90" curve="linear"/></keyframeAnimation></param><param name="锚点" key="opaque/transform/4" value="0 0"/></adjust-transform><adjust-crop mode="trim"><trim-rect left="1" top="2" right="3" bottom="4"><param name="左" key="opaque/crop/1" value="1"><keyframeAnimation><keyframe time="0s" value="1" curve="linear"/><keyframe time="1s" value="2" curve="linear"/></keyframeAnimation></param><param name="上" key="opaque/crop/2" value="2"/><param name="右" key="opaque/crop/3" value="3"/><param name="下" key="opaque/crop/4" value="4"/></trim-rect></adjust-crop>"#;
    let input = geometry_batch_input(&root, geometry);
    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let report = geometry_target_report(&patched);

    assert_eq!(report["geometry_status"], "runtime_live");
    assert!(
        report["geometry_detail"]
            .as_str()
            .unwrap()
            .contains("transform=animated")
    );
    assert!(
        report["geometry_detail"]
            .as_str()
            .unwrap()
            .contains("crop=trim_animated")
    );
    assert!(
        std::str::from_utf8(&patched.xml)
            .unwrap()
            .contains(geometry)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_accepts_six_product_locale_parameter_names() {
    let unique = format!(
        "gyroflow-finalcut-geometry-six-locales-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let cases = [
        (
            "English",
            [
                "Position", "Scale", "Rotation", "Anchor", "Left", "Top", "Right", "Bottom",
            ],
        ),
        (
            "简体中文",
            ["位置", "缩放", "旋转", "锚点", "左", "上", "右", "下"],
        ),
        (
            "繁體中文",
            ["位置", "縮放", "旋轉", "錨點", "左", "上", "右", "下"],
        ),
        (
            "日本語",
            ["位置", "調整", "回転", "アンカー", "左", "上", "右", "下"],
        ),
        (
            "한국어",
            [
                "위치",
                "크기",
                "회전",
                "앵커",
                "왼쪽",
                "위",
                "오른쪽",
                "아래",
            ],
        ),
        (
            "Русский",
            [
                "Положение",
                "Масштаб",
                "Поворот",
                "Привязка",
                "Слева",
                "Сверху",
                "Справа",
                "Снизу",
            ],
        ),
    ];
    for (locale, names) in cases {
        let [position, scale, rotation, anchor, left, top, right, bottom] = names;
        let geometry = format!(
            r#"<adjust-transform><param name="{position}" key="opaque/{locale}/position" value="0 0"/><param name="{scale}" key="opaque/{locale}/scale" value="1 1"/><param name="{rotation}" key="opaque/{locale}/rotation" value="0"/><param name="{anchor}" key="opaque/{locale}/anchor" value="0 0"/></adjust-transform><adjust-crop mode="trim"><trim-rect left="1" top="2" right="3" bottom="4"><param name="{left}" key="opaque/{locale}/left" value="1"/><param name="{top}" key="opaque/{locale}/top" value="2"/><param name="{right}" key="opaque/{locale}/right" value="3"/><param name="{bottom}" key="opaque/{locale}/bottom" value="4"/></trim-rect></adjust-crop>"#
        );
        let input = geometry_batch_input(&root, &geometry);
        let patched = patch_fcpxml_project_batch(input.as_bytes())
            .unwrap_or_else(|error| panic!("{locale}: {error}"));
        assert_eq!(
            geometry_target_report(&patched)["geometry_status"],
            "runtime_live"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_blocks_opaque_key_kind_conflicts_and_unknown_names() {
    let unique = format!(
        "gyroflow-finalcut-geometry-key-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let conflict = r#"<adjust-transform><param name="position" key="opaque/shared" value="0 0"/><param name="scale" key="opaque/shared" value="1 1"/></adjust-transform>"#;
    let conflict_input = geometry_batch_input(&root, conflict);
    let detail = geometry_skip_detail(&conflict_input);
    assert!(detail.contains("opaque geometry parameter key"));
    assert!(detail.contains("conflicting kinds"));

    let unknown = r#"<adjust-transform><param name="未知位置" key="opaque/unknown" value="0 0"/></adjust-transform>"#;
    let unknown_input = geometry_batch_input(&root, unknown);
    let detail = geometry_skip_detail(&unknown_input);
    assert!(detail.contains("unknown adjust-transform parameter"));
    assert!(detail.contains("未知位置"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_ignores_inactive_and_disabled_crop_rect_semantics() {
    let unique = format!(
        "gyroflow-finalcut-geometry-inactive-crop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let cases = [
        (
            r#"<adjust-crop mode="trim"><crop-rect left="NaN" top="-100" right="Infinity" bottom="-1"><param name="Historical Unknown" key="historical/unknown" value="not-a-number"/></crop-rect><trim-rect left="1" top="2" right="3" bottom="4"/><pan-rect left="NaN"/><pan-rect right="Infinity"/></adjust-crop>"#,
            "crop=trim_static",
        ),
        (
            r#"<adjust-crop mode="trim" enabled="0"><trim-rect left="NaN" top="-100" right="Infinity" bottom="-1"><param name="Historical Unknown" key="historical/unknown" value="not-a-number"/></trim-rect></adjust-crop>"#,
            "crop=disabled",
        ),
    ];
    for (geometry, expected_detail) in cases {
        let input = geometry_batch_input(&root, geometry);
        let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
        let report = geometry_target_report(&patched);
        assert_eq!(report["geometry_status"], "runtime_live");
        assert!(
            report["geometry_detail"]
                .as_str()
                .unwrap()
                .contains(expected_detail)
        );
        assert!(
            std::str::from_utf8(&patched.xml)
                .unwrap()
                .contains(geometry)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_combines_active_linear_crop_edges_at_merged_keyframe_times() {
    let unique = format!(
        "gyroflow-finalcut-geometry-animated-crop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let geometry = r#"<adjust-crop mode="trim"><trim-rect left="0" top="0" right="0" bottom="0"><param name="top" value="0"><keyframeAnimation><keyframe time="0s" value="0" curve="linear"/><keyframe time="1s" value="60" curve="linear"/></keyframeAnimation></param><param name="bottom" value="0"><keyframeAnimation><keyframe time="0s" value="0" curve="linear"/><keyframe time="1/2s" value="75" curve="linear"/><keyframe time="1s" value="0" curve="linear"/></keyframeAnimation></param></trim-rect></adjust-crop>"#;
    let input = geometry_batch_input(&root, geometry);
    let detail = geometry_skip_detail(&input);
    assert!(detail.contains("top and bottom"), "{detail}");
    assert!(detail.contains("keyframe"), "{detail}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_blocks_unproved_smooth_crop_interpolation_and_full_static_crop() {
    let unique = format!(
        "gyroflow-finalcut-geometry-crop-boundary-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let smooth = r#"<adjust-crop mode="crop"><crop-rect><param name="top" value="0"><keyframeAnimation><keyframe time="0s" value="0" curve="smooth"/><keyframe time="1s" value="10" curve="smooth"/></keyframeAnimation></param></crop-rect></adjust-crop>"#;
    let smooth_input = geometry_batch_input(&root, smooth);
    let detail = geometry_skip_detail(&smooth_input);
    assert!(detail.contains("linear"), "{detail}");

    let static_full = r#"<adjust-crop mode="trim"><trim-rect left="100" top="0" right="100" bottom="0"/></adjust-crop>"#;
    let square_pixel_input = geometry_batch_input(&root, static_full)
        .replace("paspH=\"4\" paspV=\"3\"", "paspH=\"1\" paspV=\"1\"");
    let detail = geometry_skip_detail(&square_pixel_input);
    assert!(detail.contains("left and right"), "{detail}");

    let invalid_ken_burns_start =
        r#"<adjust-crop mode="pan"><pan-rect top="50" bottom="50"/><pan-rect/></adjust-crop>"#;
    let input = geometry_batch_input(&root, invalid_ken_burns_start);
    let detail = geometry_skip_detail(&input);
    assert!(detail.contains("Ken Burns start"), "{detail}");

    let invalid_ken_burns_end =
        r#"<adjust-crop mode="pan"><pan-rect/><pan-rect top="50" bottom="50"/></adjust-crop>"#;
    let input = geometry_batch_input(&root, invalid_ken_burns_end);
    let detail = geometry_skip_detail(&input);
    assert!(detail.contains("Ken Burns end"), "{detail}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn geometry_preflight_blocks_unsupported_singular_nonfinite_conflicting_and_unknown_forms() {
    let unique = format!(
        "gyroflow-finalcut-geometry-blocked-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let cases = [
        (
            r#"<adjust-corners botLeft="1 2" topLeft="0 0" topRight="0 0" botRight="0 0"/>"#,
            "corner",
        ),
        (r#"<adjust-transform scale="0 1"/>"#, "singular"),
        (r#"<adjust-transform position="NaN 0"/>"#, "finite"),
        (
            r#"<adjust-transform scale="1 1"/><adjust-transform scale="2 2"/>"#,
            "multiple adjust-transform",
        ),
        (
            r#"<adjust-transform scale="1 1"><unknown-animation/></adjust-transform>"#,
            "unknown",
        ),
        (
            r#"<adjust-crop mode="pan"><pan-rect/><pan-rect/><pan-rect/></adjust-crop>"#,
            "Ken Burns",
        ),
    ];
    for (geometry, expected) in cases {
        let input = geometry_batch_input(&root, geometry);
        let detail = geometry_skip_detail(&input);
        assert!(
            detail.contains(expected),
            "geometry={geometry}, detail={detail}"
        );
    }

    let partial_format = geometry_batch_input(&root, "").replace(
        "width=\"1920\" height=\"1080\" paspH=\"4\" paspV=\"3\"",
        "width=\"1920\" paspH=\"4\"",
    );
    let detail = geometry_skip_detail(&partial_format);
    assert!(detail.contains("format"), "{detail}");
    assert!(detail.contains("height"), "{detail}");

    let unsupported = geometry_batch_input(
        &root,
        r#"<adjust-corners botLeft="1 2" topLeft="0 0" topRight="0 0" botRight="0 0"/>"#,
    );
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_route_d_batch_patch(
                unsupported.as_ptr(),
                unsupported.len(),
                &mut result,
                &mut error,
            )
        },
        GFStatus::Ok
    );
    assert!(error.is_null());
    let report = unsafe { std::slice::from_raw_parts(result.report.data, result.report.len) };
    let report: serde_json::Value = serde_json::from_slice(report).unwrap();
    assert_eq!(report["skipped_count"], 1);
    assert_eq!(report["targets"][0]["skip_reason"], "blocked_geometry");
    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_route_d_patch_result_free(&mut result);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_sibling_binds_from_media_directory_when_project_media_path_differs() {
    let unique = format!(
        "gyroflow-finalcut-exact-sibling-bind-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    let other = root.join("other-directory");
    std::fs::create_dir_all(&other).unwrap();

    let project = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "exact sibling project")
        .replace(
            "\"videofile\": \"\"",
            "\"videofile\": \"D:/Camera/P1004783.MOV\"",
        );
    let other_project = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "other directory project")
        .replace(
            "\"videofile\": \"\"",
            "\"videofile\": \"/Volumes/Archive/P1004783.MOV\"",
        );
    std::fs::write(root.join("P1004783.gyroflow"), &project).unwrap();
    std::fs::write(other.join("P1004783.gyroflow"), other_project).unwrap();

    let media_url = format!("file://{}", root.join("P1004783.MOV").display());
    let assets = format!(
        r#"<asset id="a" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>"#
    );
    let input = exact_sibling_batch_input(
        &assets,
        r#"<asset-clip name="P1004783" ref="a" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>"#,
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 0);
    assert_eq!(
        selected_banked_payloads(&patched.xml, "fx"),
        vec![encoded_project_payload(project.as_bytes())]
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_sibling_path_uses_media_parent_plus_full_stem() {
    let unique = format!(
        "gyroflow-finalcut-exact-sibling-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    let other = root.join("other-directory");
    std::fs::create_dir_all(&other).unwrap();

    let valid_project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(root.join("Present.gyroflow"), valid_project).unwrap();
    let decoy_project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(root.join("Missing.gyroflow"), &decoy_project).unwrap();
    std::fs::write(other.join("Missing.take.gyroflow"), decoy_project).unwrap();

    let present_url = format!("file://{}", root.join("Present.MOV").display());
    let missing_url = format!("file://{}", root.join("Missing.take.MOV").display());
    let assets = format!(
        r#"<asset id="present" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{present_url}"/></asset>
        <asset id="missing" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{missing_url}"/></asset>"#
    );
    let input = exact_sibling_batch_input(
        &assets,
        r#"<asset-clip name="Present" ref="present" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>
        <asset-clip name="Missing" ref="missing" offset="1s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>"#,
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 1);
    let missing = patched
        .targets
        .iter()
        .find(|target| target.clip_name == "Missing")
        .unwrap();
    assert_eq!(
        missing.action,
        gyroflow_finalcut::BatchTargetAction::Skipped
    );
    assert_eq!(selected_banked_payloads(&patched.xml, "fx").len(), 1);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn media_roots_gate_every_exact_sibling_open_and_reject_symlink_escapes() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!(
        "gyroflow-authorized-roots-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let authorized = root.join("authorized");
    let outside = root.join("outside");
    std::fs::create_dir_all(&authorized).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(authorized.join("Good.gyroflow"), project).unwrap();
    std::fs::write(outside.join("Outside.gyroflow"), project).unwrap();
    std::fs::write(outside.join("Ancestor.gyroflow"), project).unwrap();
    symlink(&outside, authorized.join("escape")).unwrap();
    symlink(
        outside.join("Outside.gyroflow"),
        authorized.join("Linked.gyroflow"),
    )
    .unwrap();
    let assets = format!(
        "<asset id=\"good\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"outside\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"ancestor\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"linked\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        authorized.join("Good.mov").display(),
        outside.join("Outside.mov").display(),
        authorized.join("escape/Ancestor.mov").display(),
        authorized.join("Linked.mov").display(),
    );
    let clips = "<asset-clip name=\"Good\" ref=\"good\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Outside\" ref=\"outside\" offset=\"1s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"><param name=\"Keep\" key=\"custom/outside\" value=\"sentinel\"/></filter-video></asset-clip><asset-clip name=\"Ancestor\" ref=\"ancestor\" offset=\"2s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"><param name=\"Keep\" key=\"custom/ancestor\" value=\"sentinel\"/></filter-video></asset-clip><asset-clip name=\"Linked\" ref=\"linked\" offset=\"3s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"><param name=\"Keep\" key=\"custom/linked\" value=\"sentinel\"/></filter-video></asset-clip>";
    let input = exact_sibling_batch_input(&assets, clips);

    let patched = patch_fcpxml_project_batch_with_media_roots(
        input.as_bytes(),
        std::slice::from_ref(&authorized),
    )
    .unwrap();

    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 3);
    assert_eq!(
        patched
            .targets
            .iter()
            .skip(1)
            .map(|target| target.skip_reason.clone().unwrap())
            .collect::<Vec<_>>(),
        vec![
            gyroflow_finalcut::BatchSkipReason::PermissionDenied,
            gyroflow_finalcut::BatchSkipReason::PermissionDenied,
            gyroflow_finalcut::BatchSkipReason::IncompatibleProject,
        ]
    );
    let output = String::from_utf8(patched.xml).unwrap();
    for key in ["custom/outside", "custom/ancestor", "custom/linked"] {
        assert!(output.contains(key));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_reader_rejects_sparse_raw_project_over_256_mib_without_allocating_it() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-raw-cap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("Good.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let oversized = std::fs::File::create(root.join("Huge.gyroflow")).unwrap();
    oversized.set_len(256 * 1024 * 1024 + 1).unwrap();
    let assets = format!(
        "<asset id=\"good\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"huge\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Good.mov").display(),
        root.join("Huge.mov").display()
    );
    let clips = "<asset-clip name=\"Good\" ref=\"good\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Huge\" ref=\"huge\" offset=\"1s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"><param name=\"Keep\" key=\"custom/huge\" value=\"sentinel\"/></filter-video></asset-clip>";
    let input = exact_sibling_batch_input(&assets, clips);

    let patched =
        patch_fcpxml_project_batch_with_media_roots(input.as_bytes(), std::slice::from_ref(&root))
            .unwrap();

    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 1);
    assert_eq!(
        patched.targets[1].skip_reason,
        Some(gyroflow_finalcut::BatchSkipReason::PayloadTooLarge)
    );
    assert!(
        String::from_utf8(patched.xml)
            .unwrap()
            .contains("custom/huge")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_sibling_missing_diagnostic_contains_expected_path() {
    let unique = format!(
        "gyroflow-finalcut-exact-sibling-diagnostic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();

    let unrelated_internal_media = "E:/Archive/Missing.MOV";
    let present_project = include_str!("fixtures/phase0-valid.gyroflow").replace(
        "\"videofile\": \"\"",
        &format!("\"videofile\": \"{unrelated_internal_media}\""),
    );
    std::fs::write(root.join("Present.gyroflow"), present_project).unwrap();

    let present_url = format!("file://{}", root.join("Present.MOV").display());
    let missing_url = format!("file://{}", root.join("Missing.MOV").display());
    let assets = format!(
        r#"<asset id="present" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{present_url}"/></asset>
        <asset id="missing" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{missing_url}"/></asset>"#
    );
    let input = exact_sibling_batch_input(
        &assets,
        r#"<asset-clip name="Present" ref="present" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>
        <asset-clip name="Missing" ref="missing" offset="1s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>"#,
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let missing = patched
        .targets
        .iter()
        .find(|target| target.clip_name == "Missing")
        .unwrap();
    let expected = root.join("Missing.gyroflow");
    assert!(missing.detail.contains(&expected.display().to_string()));
    assert!(!missing.detail.contains(unrelated_internal_media));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_updates_existing_effects_only_and_preserves_project_identity() {
    let unique = format!(
        "gyroflow-finalcut-batch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let project_a = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "project A");
    let project_b = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "project B");
    std::fs::write(root.join("A.gyroflow"), project_a).unwrap();
    std::fs::write(root.join("B.gyroflow"), project_b).unwrap();
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"b\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("A.mov").display(),
        root.join("B.mov").display(),
    );
    let clips = "<asset-clip name=\"A\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"B\" ref=\"b\" offset=\"1s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Untouched\" ref=\"b\" lane=\"1\" offset=\"0s\" start=\"0s\" duration=\"1s\"/>";
    let input = exact_sibling_batch_input(&assets, clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    assert_eq!(patched.updated_project_count, 2);
    assert_eq!(patched.timing_only_count, 0);
    assert_eq!(patched.targets.len(), 2);
    let output = std::str::from_utf8(&patched.xml).unwrap();
    assert!(
        output.contains(
            "<project name=\"Exact Sibling\" uid=\"11111111-1111-4111-8111-111111111111\""
        )
    );
    assert!(!output.contains("Gyroflow Processed"));
    assert_eq!(output.matches("<filter-video ref=\"fx\"").count(), 2);
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_route_d_batch_patch(input.as_ptr(), input.len(), &mut result, &mut error)
        },
        GFStatus::Ok
    );
    assert!(error.is_null());
    let report = unsafe { std::slice::from_raw_parts(result.report.data, result.report.len) };
    let report: serde_json::Value = serde_json::from_slice(report).unwrap();
    assert_eq!(report["original_project_name"], "Exact Sibling");
    assert_eq!(report["updated_project_count"], 2);
    assert_eq!(report["timing_only_count"], 0);
    assert_eq!(report["skipped_count"], 0);
    assert!(
        report["behavior"]
            .as_str()
            .unwrap()
            .contains("internal project name")
    );
    unsafe { gf_finalcut_route_d_patch_result_free(&mut result) };

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_snapshot_abi_updates_without_opening_the_filesystem() {
    let project_path = "/Media/A.gyroflow";
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    let assets = "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file:///Media/A.mov\"/></asset>";
    let clips = "<asset-clip name=\"A\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>";
    let input = exact_sibling_batch_input(assets, clips);
    let project_input = GFRouteDProjectInput {
        path_bytes: project_path.as_ptr(),
        path_len: project_path.len(),
        project_bytes: project.as_ptr(),
        project_len: project.len(),
        status: GF_ROUTE_D_PROJECT_INPUT_AVAILABLE,
        reserved: 0,
    };
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();

    let status = unsafe {
        gf_finalcut_route_d_batch_patch_with_project_inputs(
            input.as_ptr(),
            input.len(),
            &project_input,
            1,
            &mut result,
            &mut error,
        )
    };

    assert_eq!(status, GFStatus::Ok);
    assert!(error.is_null());
    let report = unsafe { std::slice::from_raw_parts(result.report.data, result.report.len) };
    let report: serde_json::Value = serde_json::from_slice(report).unwrap();
    assert_eq!(report["updated_project_count"], 1);
    assert_eq!(report["skipped_count"], 0);
    unsafe { gf_finalcut_route_d_patch_result_free(&mut result) };
}

#[test]
fn batch_forces_exact_sibling_over_valid_copied_payload_and_writes_project_defaults() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-sibling-authority-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let sibling = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "authoritative sibling");
    let copied = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "copied payload");
    std::fs::write(root.join("Clip.gyroflow"), &sibling).unwrap();
    let copied_payload = encoded_project_payload(copied.as_bytes());
    let banks = format!(
        "{}{}",
        banked_project_parameters('A', &copied_payload, 8),
        banked_project_parameters('B', &copied_payload, 7)
    );
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Clip.mov").display()
    );
    let clips = format!(
        "<asset-clip name=\"Clip\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\">{banks}<param name=\"FOV\" key=\"9999/10013/10016/3/10036/2001\" value=\"2.5\"><keyframeAnimation><keyframe time=\"0s\" value=\"2.5\"/></keyframeAnimation></param><param name=\"Smoothness\" key=\"9999/10013/10016/3/10036/2002\" value=\"250\"/><param name=\"Lens Correction\" key=\"9999/10013/10016/3/10036/2003\" value=\"0\"/><param name=\"Horizon Lock\" key=\"9999/10013/10016/3/10036/2004\" value=\"100\"/><param name=\"Horizon Roll\" key=\"9999/10013/10016/3/10036/2005\" value=\"-90\"/><param name=\"Zoom Mode\" key=\"9999/10013/10016/3/10036/2006\" value=\"0\"/><param name=\"Stabilization Overview\" key=\"9999/10013/10016/3/10036/2007\" value=\"1\"/></filter-video></asset-clip>"
    );
    let input = exact_sibling_batch_input(&assets, &clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let output = std::str::from_utf8(&patched.xml).unwrap();
    assert_eq!(
        selected_banked_payloads(&patched.xml, "fx"),
        vec![encoded_project_payload(sibling.as_bytes())]
    );
    for id in 2001..=2007 {
        assert!(output.contains(&format!("/{}\" value=\"", id)));
    }
    for (id, value) in [
        (2001, "1"),
        (2002, "15"),
        (2003, "100"),
        (2004, "0"),
        (2005, "0"),
        (2006, "1"),
        (2007, "0"),
    ] {
        assert!(
            output.contains(&format!("/{id}\" value=\"{value}\"/>")),
            "missing /{id}={value}: {output}"
        );
    }
    assert!(output.contains("name=\"Project Display Name\" key=\"9999/10013/10016/3/10036/1906\" value=\"Clip.gyroflow\""));
    assert!(!output.contains("value=\"2.5\""));
    assert!(!output.contains("<keyframeAnimation>"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_same_hash_updates_timing_and_preserves_host_parameter_subtrees() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-same-hash-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(root.join("Clip.gyroflow"), project).unwrap();
    let payload = encoded_project_payload(project);
    let banks = format!(
        "{}{}",
        banked_project_parameters('A', &payload, 8),
        banked_project_parameters('B', &payload, 7)
    );
    let fov = "<param name=\"Host FOV\" key=\"9999/10013/10016/3/10036/2001\" value=\"1.75\"><keyframeAnimation><keyframe time=\"0s\" value=\"1.5\"/><keyframe time=\"1s\" value=\"1.75\"/></keyframeAnimation></param>";
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Clip.mov").display()
    );
    let clips = format!(
        "<asset-clip name=\"Clip\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\">{banks}{fov}<param name=\"平滑度\" key=\"9999/10013/10016/3/10036/2002\" value=\"77\"/><param name=\"Timing Payload\" key=\"9999/10013/10016/3/10036/1903\" value=\"\"/></filter-video></asset-clip>"
    );
    let input = exact_sibling_batch_input(&assets, &clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let output = std::str::from_utf8(&patched.xml).unwrap();
    assert!(output.contains(fov));
    assert!(
        output.contains(
            "<param name=\"平滑度\" key=\"9999/10013/10016/3/10036/2002\" value=\"77\"/>"
        )
    );
    assert!(output.contains("name=\"Project Display Name\" key=\"9999/10013/10016/3/10036/1906\" value=\"Clip.gyroflow\""));
    assert_ne!(encoded_timing_payloads(&patched.xml), vec![String::new()]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_uses_parameter_key_ids_not_localized_names() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-localized-parameters-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("Clip.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Clip.mov").display()
    );
    let clips = "<asset-clip name=\"Clip\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"><param name=\"视野\" key=\"9999/10013/10016/3/10036/2001\" value=\"2.5\"><keyframeAnimation><keyframe time=\"0s\" value=\"2.5\"/></keyframeAnimation></param><param name=\"平滑度\" key=\"9999/10013/10016/3/10036/2002\" value=\"250\"/><param name=\"镜头校正\" key=\"9999/10013/10016/3/10036/2003\" value=\"0\"/><param name=\"水平锁定\" key=\"9999/10013/10016/3/10036/2004\" value=\"100\"/><param name=\"水平滚转\" key=\"9999/10013/10016/3/10036/2005\" value=\"-90\"/><param name=\"缩放模式\" key=\"9999/10013/10016/3/10036/2006\" value=\"0\"/><param name=\"稳定概览\" key=\"9999/10013/10016/3/10036/2007\" value=\"1\"/></filter-video></asset-clip>";
    let input = exact_sibling_batch_input(&assets, clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let output = std::str::from_utf8(&patched.xml).unwrap();
    for name in [
        "视野",
        "平滑度",
        "镜头校正",
        "水平锁定",
        "水平滚转",
        "缩放模式",
        "稳定概览",
    ] {
        assert!(output.contains(&format!("name=\"{name}\"")));
    }
    assert!(!output.contains("<keyframeAnimation>"));
    assert_eq!(patched.updated_project_count, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_skips_missing_invalid_permission_over_capacity_and_geometry_targets_independently() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-independent-skips-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Invalid.gyroflow"), b"not a project").unwrap();
    std::fs::write(
        root.join("Permission.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    std::fs::write(
        root.join("Geometry.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    std::fs::write(
        root.join("Good.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let mut random = String::with_capacity(5_500_000);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..5_500_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        random.push(char::from(b'!' + (state % 90) as u8));
    }
    std::fs::write(
        root.join("Oversized.gyroflow"),
        format!(
            "{{\"title\":\"oversized\",\"version\":3,\"videofile\":\"\",\"gyro_source\":{{}},\"padding\":{}}}",
            serde_json::to_string(&random).unwrap()
        ),
    )
    .unwrap();
    let assets = format!(
        "<asset id=\"missing\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"invalid\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"permission\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"oversized\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"geometry\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"good\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Missing.mov").display(),
        root.join("Invalid.mov").display(),
        root.join("Permission.mov").display(),
        root.join("Oversized.mov").display(),
        root.join("Geometry.mov").display(),
        root.join("Good.mov").display()
    );
    let clips = "<asset-clip name=\"Missing\" ref=\"missing\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Invalid\" ref=\"invalid\" offset=\"1s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Permission\" ref=\"permission\" offset=\"2s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Oversized\" ref=\"oversized\" offset=\"3s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Geometry\" ref=\"geometry\" offset=\"4s\" start=\"0s\" duration=\"1s\"><adjust-corners botLeft=\"1 2\"/><filter-video ref=\"fx\"/></asset-clip><asset-clip name=\"Good\" ref=\"good\" lane=\"1\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>";
    let input = exact_sibling_batch_input(&assets, clips);

    let reader = |path: &std::path::Path| {
        if path.file_name().and_then(|name| name.to_str()) == Some("Permission.gyroflow") {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected permission denial",
            ))
        } else {
            std::fs::read(path)
        }
    };
    let patched =
        patch_fcpxml_project_batch_with_project_reader(input.as_bytes(), &reader).unwrap();
    assert_eq!(patched.skipped_count, 5);
    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.targets.len(), 6);
    assert!(
        patched
            .targets
            .iter()
            .all(|target| target.expected_project_path.is_some())
    );
    let reasons: Vec<_> = patched
        .targets
        .iter()
        .filter_map(|target| target.skip_reason.clone())
        .collect();
    assert_eq!(
        reasons,
        vec![
            gyroflow_finalcut::BatchSkipReason::MissingProject,
            gyroflow_finalcut::BatchSkipReason::InvalidProject,
            gyroflow_finalcut::BatchSkipReason::PermissionDenied,
            gyroflow_finalcut::BatchSkipReason::PayloadTooLarge,
            gyroflow_finalcut::BatchSkipReason::BlockedGeometry,
        ]
    );
    assert_ne!(patched.xml, input.as_bytes());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_without_existing_effects_returns_no_updateable_targets() {
    let unique = format!(
        "gyroflow-finalcut-resource-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("Leaf.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let media_url = format!("file://{}", root.join("Leaf.mov").display());
    let input = format!(
        r#"<fcpxml version="1.14"><resources><format id="r1" frameDuration="1/30s"/>
        <asset id="r2" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/></resources><project name="P" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="1s"><spine>
        <asset-clip name="Leaf" ref="r2" offset="0s" start="0s" duration="1s"/>
        </spine></sequence></project></fcpxml>"#
    );

    let error = patch_fcpxml_project_batch(input.as_bytes()).unwrap_err();
    assert!(error.to_string().contains("no updateable existing"));
    assert_eq!(input.matches("<filter-video").count(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_global_xml_ambiguity_returns_no_output() {
    let input = format!(
        "<fcpxml version=\"1.14\"><resources><format id=\"r1\" frameDuration=\"1/30s\"/><effect id=\"fx1\" uid=\"{EFFECT_UUID}\"/><effect id=\"fx2\" uid=\"{EFFECT_TEMPLATE_UID}\"/></resources><project name=\"Ambiguous\" uid=\"11111111-1111-4111-8111-111111111111\"><sequence format=\"r1\" duration=\"1s\"><spine/></sequence></project></fcpxml>"
    );
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();

    assert_eq!(
        unsafe {
            gf_finalcut_route_d_batch_patch(input.as_ptr(), input.len(), &mut result, &mut error)
        },
        GFStatus::RouteDUnsafeStructure
    );
    assert!(result.xml.data.is_null());
    assert_eq!(result.xml.len, 0);
    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_route_d_patch_result_free(&mut result);
    }
}

#[test]
fn batch_fails_globally_before_mutation_for_any_duplicate_resource_id() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-global-resource-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("Good.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let assets = format!(
        "<asset id=\"good\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"duplicate\"/><format id=\"duplicate\" frameDuration=\"1/30s\"/>",
        root.join("Good.mov").display()
    );
    let input = exact_sibling_batch_input(
        &assets,
        "<asset-clip name=\"Good\" ref=\"good\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>",
    );

    let error = patch_fcpxml_project_batch(input.as_bytes()).unwrap_err();

    assert!(error.to_string().contains("duplicate global resource id"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_fails_globally_for_duplicate_exact_reserved_parameter_key() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-global-reserved-key-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("Good.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let assets = format!(
        "<asset id=\"missing\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset><asset id=\"good\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Missing.mov").display(),
        root.join("Good.mov").display()
    );
    let key = "9999/10013/10016/3/10036/1903";
    let clips = format!(
        "<asset-clip name=\"Bad\" ref=\"missing\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"><param name=\"One\" key=\"{key}\" value=\"\"/><param name=\"Two\" key=\"{key}\" value=\"\"/></filter-video></asset-clip><asset-clip name=\"Good\" ref=\"good\" offset=\"1s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>"
    );
    let input = exact_sibling_batch_input(&assets, &clips);

    let error = patch_fcpxml_project_batch(input.as_bytes()).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("duplicate reserved parameter key")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reserved_parameter_names_are_display_only_and_wrong_prefix_banks_are_replaced() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-reserved-display-only-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(root.join("Clip.gyroflow"), project).unwrap();
    let payload = encoded_project_payload(project);
    let wrong_prefix_banks = format!(
        "{}{}",
        banked_project_parameters('A', &payload, 8),
        banked_project_parameters('B', &payload, 7)
    )
    .replace("9999/10013/10016/3/10036", "untrusted/prefix");
    let custom =
        "<param name=\"Project Payload Manifest A\" key=\"custom/setting\" value=\"opaque\"/>";
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Clip.mov").display()
    );
    let clips = format!(
        "<asset-clip name=\"Clip\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\">{wrong_prefix_banks}{custom}</filter-video></asset-clip>"
    );
    let input = exact_sibling_batch_input(&assets, &clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let output = String::from_utf8(patched.xml).unwrap();

    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.timing_only_count, 0);
    assert!(output.contains(custom));
    assert!(output.contains("untrusted/prefix/1904"));
    assert!(output.contains("9999/10013/10016/3/10036/1904"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_hash_fills_only_an_empty_project_display_value() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-empty-project-display-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(root.join("Clip.gyroflow"), project).unwrap();
    let payload = encoded_project_payload(project);
    let banks = format!(
        "{}{}",
        banked_project_parameters('A', &payload, 8),
        banked_project_parameters('B', &payload, 7)
    );
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Clip.mov").display()
    );
    let clips = format!(
        "<asset-clip name=\"Clip\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\">{banks}<param name=\"Project Display Name\" key=\"9999/10013/10016/3/10036/1906\" value=\"\"/></filter-video></asset-clip>"
    );
    let input = exact_sibling_batch_input(&assets, &clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let output = String::from_utf8(patched.xml).unwrap();

    assert_eq!(patched.timing_only_count, 1);
    assert!(output.contains("key=\"9999/10013/10016/3/10036/1906\" value=\"Clip.gyroflow\""));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_accepts_only_exact_noop_conform_rate() {
    let unique = format!(
        "gyroflow-finalcut-noop-conform-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("SameRate.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    std::fs::write(root.join("SameRate.mov"), b"video bytes must not be read").unwrap();
    let media_url = format!("file://{}", root.join("SameRate.mov").display());
    let input = format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="source" frameDuration="1001/60000s"/>
        <format id="different" frameDuration="1/30s"/>
        <asset id="a" start="0s" duration="1001/10000s" format="source"><media-rep kind="original-media" src="{media_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Same Rate" uid="11111111-1111-4111-8111-111111111111"><sequence format="source" duration="1001/20000s"><spine>
        <asset-clip name="Same Rate" ref="a" offset="0s" start="0s" duration="1001/20000s"><conform-rate srcFrameRate="59.94"/><filter-video ref="fx"><param name="Timing Payload" key="9999/10013/10016/3/10036/1903" value=""/></filter-video></asset-clip>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    assert_eq!(patched.updated_project_count, 1);

    let different_output_rate = input.replace(
        "<sequence format=\"source\"",
        "<sequence format=\"different\"",
    );
    assert!(
        patch_fcpxml_project_batch(different_output_rate.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("no updateable targets")
    );

    let wrong_label = input.replace("srcFrameRate=\"59.94\"", "srcFrameRate=\"30\"");
    assert!(
        patch_fcpxml_project_batch(wrong_label.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("no updateable targets")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_linear_clip_bounds_ignore_only_trailing_out_of_range_time_points() {
    let exported = asset_project(
        "3600s",
        "3s",
        r#"<timeMap>
        <timept time="3600s" value="3603s" interp="linear"/>
        <timept time="3603s" value="3600s" interp="linear"/>
        <timept time="3610s" value="3612s" interp="smooth2"/>
        </timeMap>"#,
    );
    let patched = patch_fcpxml_project(exported.as_bytes(), None).unwrap();
    for payload in payloads(&patched.xml) {
        assert_eq!(payload["mapping"][0]["local"], "0/1");
        assert_eq!(payload["mapping"][0]["source"], "3/1");
        assert_eq!(payload["mapping"][1]["local"], "3/1");
        assert_eq!(payload["mapping"][1]["source"], "0/1");
    }

    let non_linear_boundary = exported.replacen(
        "time=\"3603s\" value=\"3600s\" interp=\"linear\"",
        "time=\"3603s\" value=\"3600s\" interp=\"smooth2\"",
        1,
    );
    assert!(patch_fcpxml_project(non_linear_boundary.as_bytes(), None).is_err());

    let in_range_mixed = exported.replacen("time=\"3610s\"", "time=\"3602s\"", 1);
    assert!(patch_fcpxml_project(in_range_mixed.as_bytes(), None).is_err());
}

#[test]
fn batch_inserts_into_exact_single_video_clip_wrapper_only() {
    let unique = format!(
        "gyroflow-finalcut-video-wrapper-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("Retime.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let media_url = format!("file://{}", root.join("Retime.mov").display());
    let input = format!(
        r#"<fcpxml version="1.14"><resources><format id="r1" frameDuration="1/30s"/>
        <asset id="a" start="0s" duration="2s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/></resources>
        <project name="Wrapper" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="1s"><spine>
        <clip name="Retime Wrapper" offset="0s" start="0s" duration="1s"><timeMap>
        <timept time="0s" value="0s" interp="linear"/><timept time="1s" value="2s" interp="linear"/>
        </timeMap><video ref="a" offset="0s" start="0s" duration="2s"/><filter-video ref="fx"/>
        <metadata><md key="keep" value="yes"/></metadata></clip>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.occurrence_count, 1);
    let output = std::str::from_utf8(&patched.xml).unwrap();
    let video = output.find("<video ref=\"a\"").unwrap();
    let filter = output.find("<filter-video ref=\"fx\"").unwrap();
    let metadata = output.find("<metadata><md key=\"keep\"").unwrap();
    assert!(video < filter && filter < metadata);
    let timing = &payloads(&patched.xml)[0];
    assert_eq!(timing["mapping"][0]["source"], "0/1");
    assert_eq!(timing["mapping"][1]["source"], "2/1");

    let ambiguous = input.replace(
        "<video ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"2s\"/>",
        "<video ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"2s\"/><video ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"2s\"/>",
    );
    assert!(
        patch_fcpxml_project_batch(ambiguous.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("no updateable targets")
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_matrix_preserves_banks_and_ignores_effectless_complex_clips() {
    let unique = format!(
        "gyroflow-finalcut-matrix-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(root.join("Shared.gyroflow"), project).unwrap();
    let media_url = format!("file://{}", root.join("Shared.mov").display());
    let payload = encoded_project_payload(project);
    let valid_banks = format!(
        "{}{}",
        banked_project_parameters('A', &payload, 8),
        banked_project_parameters('B', &payload, 7),
    );
    let input = format!(
        r#"<fcpxml version="1.14"><resources><format id="r1" frameDuration="1/30s"/>
        <asset id="a" start="0s" duration="2s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/><effect id="other" uid="other"/>
        </resources><project name="Matrix" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="4s"><spine>
        <asset-clip name="Banked" ref="a" offset="0s" start="0s" duration="1s"><filter-video ref="fx">{valid_banks}<param name="Timing Payload" key="9999/10013/10016/3/10036/1903" value=""/></filter-video></asset-clip>
        <asset-clip name="Duplicate Source" ref="a" offset="1s" start="1s" duration="1s"><filter-video ref="other" name="Other"/></asset-clip>
        <sync-clip name="Unsupported Sync" offset="2s" duration="1s"><asset-clip name="Nested" ref="a" offset="0s" start="0s" duration="1s"></asset-clip></sync-clip>
        <ref-clip name="Unsupported Compound" ref="compound" offset="3s" duration="1s"/>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let output = std::str::from_utf8(&patched.xml).unwrap();
    assert_eq!(output.matches(&valid_banks).count(), 1);
    assert_eq!(patched.timing_only_count, 1);
    assert_eq!(patched.updated_project_count, 0);
    assert_eq!(patched.skipped_count, 0);
    assert_eq!(output.matches("<filter-video ref=\"fx\"").count(), 1);
    assert_eq!(patched.targets.len(), 1);

    let ambiguous_existing = input.replace(
        &format!("{valid_banks}<param name=\"Timing Payload\""),
        "<param name=\"Timing Payload\" key=\"9999/10013/10016/3/10036/1903\" value=\"\"/><param name=\"Timing Payload\"",
    );
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_route_d_batch_patch(
                ambiguous_existing.as_ptr(),
                ambiguous_existing.len(),
                &mut result,
                &mut error,
            )
        },
        GFStatus::RouteDUnsafeStructure
    );
    assert!(!error.is_null());
    assert!(result.xml.data.is_null());
    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_route_d_patch_result_free(&mut result);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_skips_existing_effects_below_unproved_complex_ancestors_without_mutating_them() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-complex-existing-effects-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    for name in ["Good", "Sync", "Multi", "Reference", "Audition", "Mystery"] {
        std::fs::write(
            root.join(format!("{name}.gyroflow")),
            include_bytes!("fixtures/phase0-valid.gyroflow"),
        )
        .unwrap();
    }
    let assets = ["Good", "Sync", "Multi", "Reference", "Audition", "Mystery"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            format!(
                "<asset id=\"a{index}\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
                root.join(format!("{name}.mov")).display()
            )
        })
        .collect::<String>();
    let sync_filter = "<filter-video ref=\"fx\"><param name=\"Sentinel\" key=\"host/sync\" value=\"keep-sync\"/></filter-video>";
    let multi_filter = "<filter-video ref=\"fx\"><param name=\"Sentinel\" key=\"host/multi\" value=\"keep-multi\"/></filter-video>";
    let reference_filter = "<filter-video ref=\"fx\"><param name=\"Sentinel\" key=\"host/reference\" value=\"keep-reference\"/></filter-video>";
    let audition_filter = "<filter-video ref=\"fx\"><param name=\"Sentinel\" key=\"host/audition\" value=\"keep-audition\"/></filter-video>";
    let mystery_filter = "<filter-video ref=\"fx\"><param name=\"Sentinel\" key=\"host/mystery\" value=\"keep-mystery\"/></filter-video>";
    let clips = format!(
        "<asset-clip name=\"Good\" ref=\"a0\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip><sync-clip name=\"Sync Container\" offset=\"1s\" duration=\"1s\"><asset-clip name=\"Sync\" ref=\"a1\" offset=\"0s\" start=\"0s\" duration=\"1s\">{sync_filter}</asset-clip></sync-clip><mc-clip name=\"Multi Container\" offset=\"2s\" duration=\"1s\"><asset-clip name=\"Multi\" ref=\"a2\" offset=\"0s\" start=\"0s\" duration=\"1s\">{multi_filter}</asset-clip></mc-clip><ref-clip name=\"Reference Container\" offset=\"3s\" duration=\"1s\"><asset-clip name=\"Reference\" ref=\"a3\" offset=\"0s\" start=\"0s\" duration=\"1s\">{reference_filter}</asset-clip></ref-clip><audition name=\"Audition Container\"><asset-clip name=\"Audition\" ref=\"a4\" offset=\"0s\" start=\"0s\" duration=\"1s\">{audition_filter}</asset-clip></audition><project-wrapper name=\"Unknown Container\"><asset-clip name=\"Mystery\" ref=\"a5\" offset=\"0s\" start=\"0s\" duration=\"1s\">{mystery_filter}</asset-clip></project-wrapper>"
    );
    let input = exact_sibling_batch_input(&assets, &clips);

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();

    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 5);
    assert_eq!(patched.targets.len(), 6);
    for target in patched.targets.iter().skip(1) {
        assert_eq!(
            target.skip_reason,
            Some(gyroflow_finalcut::BatchSkipReason::UnsupportedStructure)
        );
    }
    let output = std::str::from_utf8(&patched.xml).unwrap();
    assert!(output.contains(sync_filter));
    assert!(output.contains(multi_filter));
    assert!(output.contains(reference_filter));
    assert!(output.contains(audition_filter));
    assert!(output.contains(mystery_filter));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_preserves_verified_single_compound_reference_support() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-batch-compound-reference-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("Compound.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let media_url = format!("file://{}", root.join("Compound.mov").display());
    let input = format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="r1" frameDuration="1/30s"/>
        <asset id="asset" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>
        <media id="compound"><sequence format="r1" duration="1s"><spine>
        <asset-clip name="Compound" ref="asset" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>
        </spine></sequence></media><effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Compound" uid="22222222-2222-4222-8222-222222222222"><sequence format="r1" duration="1s"><spine>
        <ref-clip ref="compound" offset="0s" duration="1s"/>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();

    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 0);
    assert_eq!(patched.targets.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_compound_legacy_patch_output_passes_action_semantic_verifier() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-verifier-compound-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let sibling = root.join("Compound.gyroflow");
    let sibling_bytes = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "verifier authoritative sibling");
    let old_bytes = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "verifier legacy payload");
    std::fs::write(&sibling, &sibling_bytes).unwrap();
    let legacy = encoded_project_payload(old_bytes.as_bytes());
    let media_url = format!("file://{}", root.join("Compound.mov").display());
    let original_xml = format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="r1" frameDuration="1/30s"/><format id="untouched" frameDuration="1/60s"/>
        <asset id="asset" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>
        <media id="compound"><sequence format="r1" duration="1s"><spine>
        <asset-clip name="Compound" ref="asset" offset="0s" start="0s" duration="1s"><filter-video ref="fx"><param name="Legacy Payload" key="9999/10013/10016/3/10036/1902" value="{legacy}"/><param name="Old Identity" key="9999/10013/10016/3/10036/1901" value="11111111-2222-4333-8444-555555555555"/><param name="Host FOV" key="9999/10013/10016/3/10036/2001" value="2.5"><keyframeAnimation><keyframe time="0s" value="2.5"/></keyframeAnimation></param><param name="Custom" key="custom/sentinel" value="keep"/></filter-video></asset-clip>
        </spine></sequence></media><effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Compound" uid="22222222-2222-4222-8222-222222222222"><sequence format="r1" duration="1s"><spine>
        <ref-clip ref="compound" offset="0s" duration="1s"/>
        </spine></sequence></project></fcpxml>"#
    );
    let patched = patch_fcpxml_project_batch_with_media_roots(
        original_xml.as_bytes(),
        std::slice::from_ref(&root),
    )
    .unwrap();
    assert_eq!(patched.updated_project_count, 1);
    let output_text = std::str::from_utf8(&patched.xml).unwrap();
    assert!(!output_text.contains("/1902\""));
    assert!(!output_text.contains("11111111-2222-4333-8444-555555555555"));
    assert!(output_text.contains("key=\"custom/sentinel\" value=\"keep\""));

    let original = root.join("Original.fcpxml");
    let output = root.join("Replacement.fcpxml");
    std::fs::write(&original, original_xml).unwrap();
    std::fs::write(&output, &patched.xml).unwrap();
    let route =
        "/fcpxml[1]/resources[1]/media[1]/sequence[1]/spine[1]/asset-clip[1]/filter-video[1]";
    let verified = run_route_d_output_verifier(
        &original,
        &output,
        &sibling,
        route,
        Some("1,15,100,0,0,1,0"),
    );
    assert!(
        verified.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );

    let corrupted = root.join("Corrupted.fcpxml");
    std::fs::write(
        &corrupted,
        output_text.replace(
            "<format id=\"untouched\" frameDuration=\"1/60s\"/>",
            "<format id=\"untouched\" frameDuration=\"1/24s\"/>",
        ),
    )
    .unwrap();
    let rejected = run_route_d_output_verifier(
        &original,
        &corrupted,
        &sibling,
        route,
        Some("1,15,100,0,0,1,0"),
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("resources"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_same_hash_patch_output_preserves_host_subtrees_for_verifier() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-verifier-same-hash-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let sibling = root.join("Clip.gyroflow");
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    std::fs::write(&sibling, project).unwrap();
    let payload = encoded_project_payload(project);
    let banks = format!(
        "{}{}",
        banked_project_parameters('A', &payload, 8),
        banked_project_parameters('B', &payload, 7)
    );
    let media_url = format!("file://{}", root.join("Clip.mov").display());
    let original_xml = format!(
        r#"<fcpxml version="1.14"><resources><format id="r1" frameDuration="1/30s"/><asset id="a" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset><effect id="fx" uid="{EFFECT_UUID}"/></resources><project name="Same Hash" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="1s"><spine><asset-clip name="Clip" ref="a" offset="0s" start="0s" duration="1s"><filter-video ref="fx"><param name="Host Identity" key="9999/10013/10016/3/10036/1901" value="11111111-2222-4333-8444-555555555555"/>{banks}<param name="Host Display" key="9999/10013/10016/3/10036/1906" value="Keep This Display"><metadata key="display-sentinel" value="keep"/></param><param name="Host FOV" key="9999/10013/10016/3/10036/2001" value="1.75"><keyframeAnimation><keyframe time="0s" value="1.5"/><keyframe time="1s" value="1.75"/></keyframeAnimation></param><param name="Timing Payload" key="9999/10013/10016/3/10036/1903" value=""/></filter-video></asset-clip></spine></sequence></project></fcpxml>"#
    );
    let patched = patch_fcpxml_project_batch_with_media_roots(
        original_xml.as_bytes(),
        std::slice::from_ref(&root),
    )
    .unwrap();
    assert_eq!(patched.timing_only_count, 1);

    let original = root.join("Original.fcpxml");
    let output = root.join("Replacement.fcpxml");
    std::fs::write(&original, original_xml).unwrap();
    std::fs::write(&output, &patched.xml).unwrap();
    let route = "/fcpxml[1]/project[1]/sequence[1]/spine[1]/asset-clip[1]/filter-video[1]";
    let verified = run_route_d_output_verifier(&original, &output, &sibling, route, None);
    assert!(
        verified.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_all_skipped_returns_no_updateable_targets() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-all-skipped-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let assets = format!(
        "<asset id=\"missing\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Missing.mov").display()
    );
    let input = exact_sibling_batch_input(
        &assets,
        "<asset-clip name=\"Missing\" ref=\"missing\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>",
    );

    let error = patch_fcpxml_project_batch(input.as_bytes()).unwrap_err();

    assert!(
        error.to_string().contains("no updateable targets"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_snapshot_batch_reports_all_skipped_targets_without_output_changes() {
    let assets = "<asset id=\"missing\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file:///Media/Missing.mov\"/></asset>";
    let input = exact_sibling_batch_input(
        assets,
        "<asset-clip name=\"Missing\" ref=\"missing\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>",
    );
    let reader = |_path: &std::path::Path| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "fixture denied",
        ))
    };

    let reported = patch_fcpxml_project_batch_with_project_reader_report_all_skipped(
        input.as_bytes(),
        &reader,
    )
    .unwrap();

    assert_eq!(reported.xml, input.as_bytes());
    assert_eq!(reported.updated_project_count, 0);
    assert_eq!(reported.skipped_count, 1);
    assert_eq!(
        reported.targets[0].skip_reason,
        Some(gyroflow_finalcut::BatchSkipReason::PermissionDenied)
    );
}

#[test]
fn batch_rejects_byte_identical_timing_only_output() {
    let root = std::env::temp_dir().join(format!(
        "gyroflow-identical-timing-only-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("Clip.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let assets = format!(
        "<asset id=\"a\" start=\"0s\" duration=\"1s\" format=\"r1\"><media-rep kind=\"original-media\" src=\"file://{}\"/></asset>",
        root.join("Clip.mov").display()
    );
    let input = exact_sibling_batch_input(
        &assets,
        "<asset-clip name=\"Clip\" ref=\"a\" offset=\"0s\" start=\"0s\" duration=\"1s\"><filter-video ref=\"fx\"/></asset-clip>",
    );
    let first = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    assert_eq!(first.updated_project_count, 1);

    let error = patch_fcpxml_project_batch(&first.xml).unwrap_err();

    assert!(error.to_string().contains("no XML changes"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_target_failures_are_skipped_but_resource_conflicts_fail_closed() {
    let unique = format!(
        "gyroflow-finalcut-failures-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    std::fs::write(
        root.join("Good.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    std::fs::write(root.join("Corrupt.gyroflow"), b"not a gyroflow project").unwrap();

    let good_url = format!("file://{}", root.join("Good.mov").display());
    let missing_url = format!("file://{}", root.join("Missing.mov").display());
    let corrupt_url = format!("file://{}", root.join("Corrupt.mov").display());
    let base = |assets: &str, clips: &str, effects: &str| {
        format!(
            r#"<fcpxml version="1.14"><resources><format id="r1" frameDuration="1/30s"/>{assets}{effects}</resources>
            <project name="Failures" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="10s"><spine>{clips}</spine></sequence></project></fcpxml>"#
        )
    };
    let effects = format!(r#"<effect id="fx" uid="{EFFECT_UUID}"/>"#);
    let good_asset = format!(
        r#"<asset id="good" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{good_url}"/></asset>"#
    );
    let missing_asset = format!(
        r#"<asset id="missing" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{missing_url}"/></asset>"#
    );
    let corrupt_asset = format!(
        r#"<asset id="corrupt" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{corrupt_url}"/></asset>"#
    );

    let missing_is_skipped = base(
        &format!("{good_asset}{missing_asset}"),
        r#"<asset-clip name="Good" ref="good" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>
        <asset-clip name="Missing" ref="missing" offset="1s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>"#,
        &effects,
    );
    let patched = patch_fcpxml_project_batch(missing_is_skipped.as_bytes()).unwrap();
    assert_eq!(patched.updated_project_count, 1);
    assert_eq!(patched.skipped_count, 1);

    let corrupt_target = base(
        &corrupt_asset,
        r#"<asset-clip name="Corrupt" ref="corrupt" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>"#,
        &effects,
    );
    assert!(
        patch_fcpxml_project_batch(corrupt_target.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("no updateable targets")
    );

    let ambiguous_asset = format!(
        r#"<asset id="ambiguous" start="0s" duration="1s" format="r1">
        <media-rep kind="original-media" src="{good_url}"/><media-rep kind="original-media" src="{missing_url}"/></asset>"#
    );
    let ambiguous_existing = base(
        &ambiguous_asset,
        r#"<asset-clip name="Ambiguous" ref="ambiguous" offset="0s" start="0s" duration="1s"><filter-video ref="fx"><param name="Timing Payload" key="9999/10013/10016/3/10036/1903" value=""/></filter-video></asset-clip>"#,
        &effects,
    );
    assert!(
        patch_fcpxml_project_batch(ambiguous_existing.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("no updateable targets")
    );

    let duplicate_resources = base(
        &good_asset,
        r#"<asset-clip name="Good" ref="good" offset="0s" start="0s" duration="1s"></asset-clip>"#,
        &format!(
            r#"<effect id="fx" uid="{EFFECT_UUID}"/><effect id="fx2" uid="{EFFECT_TEMPLATE_UID}"/>"#
        ),
    );
    assert!(
        patch_fcpxml_project_batch(duplicate_resources.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("exactly one")
    );

    let mut random = String::with_capacity(5_500_000);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..5_500_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        random.push(char::from(b'!' + (state % 90) as u8));
    }
    let oversized_project = format!(
        "{{\"title\":\"oversized\",\"version\":3,\"videofile\":\"\",\"gyro_source\":{{}},\"padding\":{}}}",
        serde_json::to_string(&random).unwrap()
    );
    std::fs::write(root.join("Oversized.gyroflow"), oversized_project).unwrap();
    let oversized_url = format!("file://{}", root.join("Oversized.mov").display());
    let oversized_asset = format!(
        r#"<asset id="oversized" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{oversized_url}"/></asset>"#
    );
    let oversized_target = base(
        &oversized_asset,
        r#"<asset-clip name="Oversized" ref="oversized" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>"#,
        &effects,
    );
    assert!(
        patch_fcpxml_project_batch(oversized_target.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("no updateable targets")
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_uses_ten_416_kib_chunks_per_bank() {
    let unique = format!(
        "gyroflow-finalcut-bank-geometry-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir(&root).unwrap();
    let mut random = String::with_capacity(500_000);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for _ in 0..500_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        random.push(char::from(b'!' + (state % 90) as u8));
    }
    let project = format!(
        "{{\"title\":\"bank geometry\",\"version\":3,\"videofile\":\"\",\"gyro_source\":{{}},\"padding\":{}}}",
        serde_json::to_string(&random).unwrap()
    );
    let encoded = encoded_project_payload(project.as_bytes());
    assert!(encoded.len() > 416 * 1024);
    assert!(encoded.len() <= 2 * 416 * 1024);
    std::fs::write(root.join("Geometry.gyroflow"), project).unwrap();
    let media_url = format!("file://{}", root.join("Geometry.mov").display());
    let input = format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="r1" frameDuration="1/30s"/>
        <asset id="a" start="0s" duration="1s" format="r1"><media-rep kind="original-media" src="{media_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Geometry" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="1s"><spine>
        <asset-clip name="Geometry" ref="a" offset="0s" start="0s" duration="1s"><filter-video ref="fx"/></asset-clip>
        </spine></sequence></project></fcpxml>"#
    );
    let patched = patch_fcpxml_project_batch(input.as_bytes()).unwrap();
    let document = roxmltree::Document::parse_with_options(
        std::str::from_utf8(&patched.xml).unwrap(),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    let filter = document
        .descendants()
        .find(|node| node.has_tag_name("filter-video") && node.attribute("ref") == Some("fx"))
        .unwrap();
    let manifest = filter
        .children()
        .find(|node| {
            node.has_tag_name("param")
                && node.attribute("name") == Some("Project Payload Manifest A")
        })
        .and_then(|node| node.attribute("value"))
        .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(manifest).unwrap()).unwrap();
    assert_eq!(manifest["chunk_count"], 2);
    let first_chunk = filter
        .children()
        .find(|node| {
            node.has_tag_name("param") && node.attribute("name") == Some("Project Payload A 01")
        })
        .and_then(|node| node.attribute("value"))
        .unwrap();
    assert_eq!(first_chunk.len(), 416 * 1024);
    assert_eq!(
        filter
            .children()
            .find(|node| {
                node.has_tag_name("param")
                    && node.attribute("name") == Some("Project Payload Manifest A")
            })
            .and_then(|node| node.attribute("key")),
        Some("9999/10013/10016/3/10036/1904")
    );
    assert_eq!(
        filter
            .children()
            .find(|node| {
                node.has_tag_name("param") && node.attribute("name") == Some("Project Payload A 01")
            })
            .and_then(|node| node.attribute("key")),
        Some("9999/10013/10016/3/10036/1910")
    );
    assert!(filter.children().all(|node| {
        node.attribute("name") != Some("Project Payload A 11")
            && node.attribute("name") != Some("Project Payload B 11")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_asset_clip_matrix_maps_local_to_source_time_per_occurrence() {
    let cases = [
        ("3600s", "30s", "", "0/1", "0/1", "30/1", "30/1"),
        ("3605s", "25s", "", "0/1", "5/1", "25/1", "30/1"),
        (
            "7200s",
            "60s",
            r#"<timeMap><timept time="7200s" value="3600s"/><timept time="7260s" value="3630s"/></timeMap>"#,
            "0/1",
            "0/1",
            "60/1",
            "30/1",
        ),
        (
            "1800s",
            "15s",
            r#"<timeMap><timept time="1800s" value="3600s"/><timept time="1815s" value="3630s"/></timeMap>"#,
            "0/1",
            "0/1",
            "15/1",
            "30/1",
        ),
        (
            "3600s",
            "30s",
            r#"<timeMap><timept time="3600s" value="3630s"/><timept time="3630s" value="3600s"/></timeMap>"#,
            "0/1",
            "30/1",
            "30/1",
            "0/1",
        ),
    ];

    for (start, duration, time_map, local0, source0, local1, source1) in cases {
        let input = asset_project(start, duration, time_map);
        let patched = patch_fcpxml_project(input.as_bytes(), Some("Processed")).unwrap();
        let payloads = payloads(&patched.xml);

        assert_eq!(patched.original_project_name, "Original");
        assert_eq!(patched.processed_project_name, "Processed");
        assert_eq!(patched.occurrence_count, 2);
        assert!(
            std::str::from_utf8(&patched.xml)
                .unwrap()
                .contains("must-survive")
        );
        assert_eq!(payloads[0]["occurrence"], 1);
        assert_eq!(payloads[1]["occurrence"], 2);
        for payload in payloads {
            assert_eq!(payload["mapping"][0]["local"], local0);
            assert_eq!(payload["mapping"][0]["source"], source0);
            assert_eq!(payload["mapping"][1]["local"], local1);
            assert_eq!(payload["mapping"][1]["source"], source1);
            assert_eq!(payload["effect_bounds"]["start"], start.replace('s', "/1"));
            assert_eq!(payload["input_bounds"]["start"], start.replace('s', "/1"));
            assert_eq!(
                payload["effect_bounds"]["duration"],
                duration.replace('s', "/1")
            );
            assert_eq!(payload["fcpxml_version"], "1.14");
            assert_eq!(payload["structure_sha256"].as_str().unwrap().len(), 64);
        }
    }
}

#[test]
fn piecewise_linear_ramp_keeps_exact_rational_control_points() {
    let time_map = r#"<timeMap>
      <timept time="3600s" value="3600s" interp="linear"/>
      <timept time="111052287/30720s" value="3613110416666/1000000000s" interp="linear"/>
      <timept time="55756287/15360s" value="3622474999999/1000000000s" interp="linear"/>
      <timept time="3660s" value="3630s" interp="linear"/>
    </timeMap>"#;
    let input = asset_project("3600s", "60s", time_map);
    let patched = patch_fcpxml_project(input.as_bytes(), None).unwrap();
    let payload = &payloads(&patched.xml)[0];

    assert_eq!(payload["mapping"][1]["local"], "153429/10240");
    assert_eq!(payload["mapping"][1]["source"], "6555208333/500000000");
    assert_eq!(payload["mapping"][2]["local"], "153429/5120");
    assert_eq!(payload["mapping"][2]["source"], "22474999999/1000000000");
}

#[test]
fn smooth2_ramp_expands_to_the_verified_per_frame_floor_mapping() {
    let time_map = r#"<timeMap>
      <timept time="3600s" value="3600s" interp="smooth2"/>
      <timept time="111052287/30720s" value="3613110416666/1000000000s" interp="smooth2" inTime="1s" outTime="1s"/>
      <timept time="55756287/15360s" value="3622474999999/1000000000s" interp="smooth2" inTime="1s" outTime="1s"/>
      <timept time="111972861/30720s" value="3628093749999/1000000000s" interp="smooth2" inTime="1s" outTime="1s"/>
      <timept time="56216575/15360s" value="3629966666666/1000000000s" interp="smooth2" inTime="1s" outTime="16666667/1000000000s"/>
      <timept time="1405439975/384000s" value="3629966666666/1000000000s" interp="smooth2" inTime="16666667/1000000000s" outTime="8333333/1000000000s"/>
      <timept time="1405452775/384000s" value="3630s" interp="smooth2"/>
    </timeMap>"#;
    let input = asset_project("3600s", "60s", time_map).replace("1001/30000s", "100/3000s");

    let patched = patch_fcpxml_project(input.as_bytes(), None).unwrap();
    let payload = &payloads(&patched.xml)[0];
    let mapping = payload["mapping"].as_array().unwrap();

    assert_eq!(mapping.len(), 1801);
    let source_frames: Vec<i128> = mapping
        .iter()
        .map(|point| {
            let source = point["source"].as_str().unwrap();
            let (numerator, denominator) = source.split_once('/').unwrap();
            let source_frame = numerator.parse::<i128>().unwrap() * 30;
            let denominator = denominator.parse::<i128>().unwrap();
            assert_eq!(source_frame % denominator, 0);
            source_frame / denominator
        })
        .collect();
    let digest_input = source_frames
        .iter()
        .map(i128::to_string)
        .collect::<Vec<_>>()
        .join(",");

    assert_eq!(
        format!("{:x}", Sha256::digest(digest_input.as_bytes())),
        "078f77a20b7fbdb9b1aa328edbb7f54776ff6641313e7f5bf1b76dafdbbe75f0"
    );
    for (frame, source_frame) in [
        (0, 0),
        (1, 0),
        (2, 1),
        (424, 370),
        (449, 391),
        (925, 684),
        (1349, 841),
        (1374, 845),
        (1782, 896),
        (1799, 899),
        (1800, 899),
    ] {
        assert_eq!(source_frames[frame], source_frame);
    }
}

#[test]
fn deprecated_smooth_time_map_fails_closed() {
    let time_map = r#"<timeMap>
      <timept time="3600s" value="3600s" interp="smooth"/>
      <timept time="3615s" value="3610s" interp="smooth" inTime="1s" outTime="1s"/>
      <timept time="3630s" value="3630s" interp="smooth"/>
    </timeMap>"#;
    let input = asset_project("3600s", "30s", time_map);

    let error = patch_fcpxml_project(input.as_bytes(), None).unwrap_err();

    assert!(error.to_string().contains("smooth timeMap interpolation"));
}

#[test]
fn unverified_smooth2_variants_fail_closed() {
    let smooth2 = r#"<timeMap frameSampling="nearest-neighbor">
      <timept time="3600s" value="3600s" interp="smooth2"/>
      <timept time="3615s" value="3610s" interp="smooth2" inTime="1s" outTime="1s"/>
      <timept time="3630s" value="3630s" interp="smooth2"/>
    </timeMap>"#;
    let input = asset_project("3600s", "30s", smooth2);
    assert!(
        patch_fcpxml_project(input.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("floor frameSampling")
    );

    let older_version = input.replace("version=\"1.14\"", "version=\"1.13\"");
    assert!(
        patch_fcpxml_project(older_version.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("verified only for FCPXML 1.14")
    );

    let linear_with_handles = smooth2
        .replace(
            "frameSampling=\"nearest-neighbor\"",
            "frameSampling=\"floor\"",
        )
        .replace("smooth2", "linear");
    assert!(
        patch_fcpxml_project(
            asset_project("3600s", "30s", &linear_with_handles).as_bytes(),
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("mixed timeMap interpolation")
    );
}

#[test]
fn omitted_default_timing_parameter_is_inserted_from_owned_project_key() {
    let input = asset_project("3600s", "30s", "").replace(
        "<param name=\"Instance Identity\" value=\"duplicate\"/><param name=\"Timing Payload\" value=\"\"/>",
        "<param name=\"Project Payload\" key=\"9999/10013/10016/3/10036/1902\" value=\"payload\"/>",
    );

    let patched = patch_fcpxml_project(input.as_bytes(), Some("Processed")).unwrap();
    let document = roxmltree::Document::parse_with_options(
        std::str::from_utf8(&patched.xml).unwrap(),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    let timing_keys: Vec<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("param") && node.attribute("name") == Some("Timing Payload")
        })
        .map(|node| node.attribute("key").unwrap())
        .collect();

    assert_eq!(patched.occurrence_count, 2);
    assert_eq!(payloads(&patched.xml).len(), 2);
    assert_eq!(
        timing_keys,
        vec![
            "9999/10013/10016/3/10036/1903",
            "9999/10013/10016/3/10036/1903",
        ]
    );
    assert_eq!(
        document
            .descendants()
            .filter(|node| {
                node.has_tag_name("param")
                    && node.attribute("name") == Some("Project Payload")
                    && node.attribute("value") == Some("payload")
            })
            .count(),
        2
    );
}

#[test]
fn omitted_timing_is_inserted_from_verified_banked_payload_without_rewriting_chunks() {
    let bank = banked_project_parameters('A', "cGF5bG9hZA==", 7);
    let input = asset_project("3600s", "30s", "").replace(
        "<param name=\"Instance Identity\" value=\"duplicate\"/><param name=\"Timing Payload\" value=\"\"/>",
        &bank,
    );

    let patched = patch_fcpxml_project(input.as_bytes(), Some("Processed")).unwrap();
    let patched_text = std::str::from_utf8(&patched.xml).unwrap();
    let document = roxmltree::Document::parse_with_options(
        patched_text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    let timing_keys: Vec<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("param") && node.attribute("name") == Some("Timing Payload")
        })
        .map(|node| node.attribute("key").unwrap())
        .collect();

    assert_eq!(patched_text.matches(&bank).count(), 2);
    assert_eq!(timing_keys.len(), 2);
    assert!(
        timing_keys
            .iter()
            .all(|key| *key == "9999/10013/10016/3/10036/1903")
    );
}

#[test]
fn banked_payload_corruption_conflict_and_fallback_are_fail_closed() {
    let valid_a = banked_project_parameters('A', "QUFBQQ==", 1);
    let corrupt_a = valid_a.replace("value=\"QUFBQQ==\"", "value=\"corrupt\"");
    let only_corrupt = asset_project("3600s", "30s", "").replace(
        "<param name=\"Instance Identity\" value=\"duplicate\"/><param name=\"Timing Payload\" value=\"\"/>",
        &corrupt_a,
    );
    assert!(
        patch_fcpxml_project(only_corrupt.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("bank A")
    );

    let valid_b = banked_project_parameters('B', "QkJCQg==", 1);
    let corrupt_newer_a = banked_project_parameters('A', "Q0NDQw==", 2)
        .replace("value=\"Q0NDQw==\"", "value=\"corrupt\"");
    let fallback_params = format!("{corrupt_newer_a}{valid_b}");
    let fallback = asset_project("3600s", "30s", "").replace(
        "<param name=\"Instance Identity\" value=\"duplicate\"/><param name=\"Timing Payload\" value=\"\"/>",
        &fallback_params,
    );
    assert!(patch_fcpxml_project(fallback.as_bytes(), None).is_ok());

    let conflicting = format!(
        "{}{}",
        banked_project_parameters('A', "QUFBQQ==", 9),
        banked_project_parameters('B', "QkJCQg==", 9),
    );
    let conflict = asset_project("3600s", "30s", "").replace(
        "<param name=\"Instance Identity\" value=\"duplicate\"/><param name=\"Timing Payload\" value=\"\"/>",
        &conflicting,
    );
    assert!(
        patch_fcpxml_project(conflict.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("same generation")
    );
}

#[test]
fn single_library_wrapped_project_is_processed() {
    let input = asset_project("3600s", "30s", "")
        .replacen("  <project", "  <library><event><project", 1)
        .replacen(
            "  </project>\n</fcpxml>",
            "  </project></event></library>\n</fcpxml>",
            1,
        );

    let patched = patch_fcpxml_project(input.as_bytes(), Some("Processed")).unwrap();

    assert_eq!(patched.original_project_name, "Original");
    assert_eq!(patched.processed_project_name, "Processed");
    assert_eq!(patched.occurrence_count, 2);
    assert!(
        std::str::from_utf8(&patched.xml)
            .unwrap()
            .contains("<library><event><project name=\"Processed\"")
    );
}

#[test]
fn exact_owned_motion_template_uid_identifies_the_production_effect() {
    let input = asset_project("3600s", "30s", "").replace(EFFECT_UUID, EFFECT_TEMPLATE_UID);

    let patched = patch_fcpxml_project(input.as_bytes(), Some("Processed")).unwrap();

    assert_eq!(patched.occurrence_count, 2);

    let spoofed = input.replace(
        EFFECT_TEMPLATE_UID,
        "~/Effects.localized/Other/Gyroflow NiYien.moef",
    );
    assert!(patch_fcpxml_project(spoofed.as_bytes(), Some("Processed")).is_err());
}

#[test]
fn single_compound_reference_resolves_inner_asset_occurrences() {
    let input = format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="r1" frameDuration="1/30s"/>
        <media id="compound"><sequence format="r1" duration="30s"><spine>
          <asset-clip ref="asset" offset="0s" start="3600s" duration="30s">
            <filter-video ref="fx"><param name="Timing Payload" value=""/></filter-video>
          </asset-clip>
        </spine></sequence></media>
        <asset id="asset" start="3600s" duration="30s"/>
        <effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Compound" uid="22222222-2222-4222-8222-222222222222"><sequence format="r1" duration="30s"><spine>
          <ref-clip ref="compound" offset="0s" duration="30s"/>
        </spine></sequence></project></fcpxml>"#
    );
    let patched = patch_fcpxml_project(input.as_bytes(), None).unwrap();
    let payload = &payloads(&patched.xml)[0];
    assert_eq!(payload["mapping"][0]["source"], "0/1");
    assert_eq!(payload["mapping"][1]["source"], "30/1");
}

#[test]
fn unsupported_ambiguous_or_partial_inputs_fail_before_output() {
    let unsupported = asset_project("3600s", "30s", "").replace("1.14", "1.99");
    assert!(patch_fcpxml_project(unsupported.as_bytes(), None).is_err());

    let two_projects = asset_project("3600s", "30s", "").replace(
        "</fcpxml>",
        "<project name=\"Second\"><sequence duration=\"1s\"/></project></fcpxml>",
    );
    assert!(patch_fcpxml_project(two_projects.as_bytes(), None).is_err());

    let missing_timing = asset_project("3600s", "30s", "").replacen(
        "<param name=\"Timing Payload\" value=\"\"/>",
        "",
        1,
    );
    assert!(patch_fcpxml_project(missing_timing.as_bytes(), None).is_err());

    let compound = format!(
        r#"<fcpxml version="1.14"><resources><media id="c"><sequence duration="1s"><spine>
        <asset-clip ref="a" start="0s" duration="1s"><filter-video ref="fx"><param name="Timing Payload" value=""/></filter-video></asset-clip>
        </spine></sequence></media><asset id="a" start="0s" duration="1s"/><effect id="fx" uid="{EFFECT_UUID}"/></resources>
        <project name="P"><sequence duration="2s"><spine><ref-clip ref="c" duration="1s"/><ref-clip ref="c" offset="1s" duration="1s"/></spine></sequence></project></fcpxml>"#
    );
    assert!(patch_fcpxml_project(compound.as_bytes(), None).is_err());
}

#[test]
fn c_bridge_returns_patch_and_report_with_explicit_release_contract() {
    let input = asset_project("3600s", "30s", "");
    let name = b"Processed by C ABI";
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    let status = unsafe {
        gf_finalcut_route_d_patch(
            input.as_ptr(),
            input.len(),
            name.as_ptr(),
            name.len(),
            &mut result,
            &mut error,
        )
    };
    assert_eq!(status, GFStatus::Ok);
    assert!(error.is_null());
    let xml = unsafe { std::slice::from_raw_parts(result.xml.data, result.xml.len) };
    let report = unsafe { std::slice::from_raw_parts(result.report.data, result.report.len) };
    assert!(
        std::str::from_utf8(xml)
            .unwrap()
            .contains("Processed by C ABI")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(report).unwrap()["occurrence_count"],
        2
    );
    unsafe { gf_finalcut_route_d_patch_result_free(&mut result) };
    assert!(result.xml.data.is_null());
    assert!(result.report.data.is_null());
}

#[test]
fn timing_payload_resolves_exact_source_time_and_stale_bounds_fail_closed() {
    let input = asset_project(
        "7200s",
        "60s",
        r#"<timeMap><timept time="7200s" value="3600s"/><timept time="7260s" value="3630s"/></timeMap>"#,
    );
    let patched = patch_fcpxml_project(input.as_bytes(), None).unwrap();
    let timing = encoded_timing_payloads(&patched.xml).remove(0);
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    assert_eq!(
        unsafe {
            gf_finalcut_instance_load_project(
                instance,
                project.as_ptr(),
                project.len(),
                std::ptr::null_mut(),
            )
        },
        GFStatus::Ok
    );
    let bounds = GFTimeRange {
        start: GFTime {
            numerator: 7200,
            denominator: 1,
        },
        duration: GFTime {
            numerator: 60,
            denominator: 1,
        },
    };
    let mut error: *mut GFError = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_instance_load_timing_payload(
                instance,
                timing.as_ptr(),
                timing.len(),
                &bounds,
                &bounds,
                &mut error,
            )
        },
        GFStatus::Ok
    );
    assert!(error.is_null());
    let mut source = GFTime {
        numerator: 0,
        denominator: 1,
    };
    assert_eq!(
        unsafe {
            gf_finalcut_instance_resolve_source_time(
                instance,
                GFTime {
                    numerator: 15,
                    denominator: 1,
                },
                &mut source,
                &mut error,
            )
        },
        GFStatus::Ok
    );
    assert_eq!(
        source,
        GFTime {
            numerator: 15,
            denominator: 2
        }
    );

    let stale_bounds = GFTimeRange {
        start: GFTime {
            numerator: 7200,
            denominator: 1,
        },
        duration: GFTime {
            numerator: 59,
            denominator: 1,
        },
    };
    assert_eq!(
        unsafe {
            gf_finalcut_instance_load_timing_payload(
                instance,
                timing.as_ptr(),
                timing.len(),
                &stale_bounds,
                &bounds,
                &mut error,
            )
        },
        GFStatus::StaleTiming
    );
    unsafe { gf_finalcut_error_free(error) };
    error = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_instance_resolve_source_time(
                instance,
                GFTime {
                    numerator: 15,
                    denominator: 1,
                },
                &mut source,
                &mut error,
            )
        },
        GFStatus::MissingTiming
    );

    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_instance_free(instance);
    }
}
