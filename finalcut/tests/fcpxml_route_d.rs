use base64::{Engine as _, engine::general_purpose::STANDARD};
use gyroflow_finalcut::{
    GFError, GFRouteDPatchResult, GFStatus, GFTime, GFTimeRange, gf_finalcut_error_free,
    gf_finalcut_instance_create, gf_finalcut_instance_free, gf_finalcut_instance_load_project,
    gf_finalcut_instance_load_project_payload, gf_finalcut_instance_load_timing_payload,
    gf_finalcut_instance_resolve_source_time, gf_finalcut_owned_bytes_free,
    gf_finalcut_project_payload_encode, gf_finalcut_route_d_batch_patch, gf_finalcut_route_d_patch,
    gf_finalcut_route_d_patch_result_free, patch_fcpxml_project, patch_fcpxml_project_batch,
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
        .map(|filter| {
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
            let chunk_count = manifest["chunk_count"].as_u64().unwrap() as usize;
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
                .collect::<String>()
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

fn geometry_batch_input(root: &std::path::Path, geometry: &str) -> String {
    std::fs::write(
        root.join("Geometry.gyroflow"),
        include_bytes!("fixtures/phase0-valid.gyroflow"),
    )
    .unwrap();
    let media_url = format!("file://{}", root.join("Geometry.mov").display());
    format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="source" frameDuration="1/30s" width="1920" height="1080" paspH="4" paspV="3"/>
        <format id="sequence" frameDuration="1/30s" width="1080" height="1920"/>
        <asset id="a" start="0s" duration="1s" format="source"><media-rep kind="original-media" src="{media_url}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/>
        </resources><project name="Geometry" uid="11111111-1111-4111-8111-111111111111"><sequence format="sequence" duration="1s"><spine>
        <asset-clip name="Geometry" ref="a" offset="0s" start="0s" duration="1s">{geometry}</asset-clip>
        </spine></sequence></project></fcpxml>"#
    )
}

fn geometry_target_report(result: &gyroflow_finalcut::BatchRouteDPatchResult) -> serde_json::Value {
    serde_json::to_value(&result.targets[0]).unwrap()
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
    let first = patch_fcpxml_project_batch(first_input.as_bytes(), None).unwrap();
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
    let changed = patch_fcpxml_project_batch(changed_input.as_bytes(), None).unwrap();

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
        let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
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
    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
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
    let error = patch_fcpxml_project_batch(conflict_input.as_bytes(), None).unwrap_err();
    assert!(error.to_string().contains("opaque geometry parameter key"));
    assert!(error.to_string().contains("conflicting kinds"));

    let unknown = r#"<adjust-transform><param name="未知位置" key="opaque/unknown" value="0 0"/></adjust-transform>"#;
    let unknown_input = geometry_batch_input(&root, unknown);
    let error = patch_fcpxml_project_batch(unknown_input.as_bytes(), None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown adjust-transform parameter")
    );
    assert!(error.to_string().contains("未知位置"));
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
        let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
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
    let error = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap_err();

    assert!(error.to_string().contains("top and bottom"), "{error}");
    assert!(error.to_string().contains("keyframe"), "{error}");
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
    let error = patch_fcpxml_project_batch(smooth_input.as_bytes(), None).unwrap_err();
    assert!(error.to_string().contains("linear"), "{error}");

    let static_full = r#"<adjust-crop mode="trim"><trim-rect left="100" top="0" right="100" bottom="0"/></adjust-crop>"#;
    let square_pixel_input = geometry_batch_input(&root, static_full)
        .replace("paspH=\"4\" paspV=\"3\"", "paspH=\"1\" paspV=\"1\"");
    let error = patch_fcpxml_project_batch(square_pixel_input.as_bytes(), None).unwrap_err();
    assert!(error.to_string().contains("left and right"), "{error}");

    let invalid_ken_burns_start =
        r#"<adjust-crop mode="pan"><pan-rect top="50" bottom="50"/><pan-rect/></adjust-crop>"#;
    let input = geometry_batch_input(&root, invalid_ken_burns_start);
    let error = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap_err();
    assert!(error.to_string().contains("Ken Burns start"), "{error}");

    let invalid_ken_burns_end =
        r#"<adjust-crop mode="pan"><pan-rect/><pan-rect top="50" bottom="50"/></adjust-crop>"#;
    let input = geometry_batch_input(&root, invalid_ken_burns_end);
    let error = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap_err();
    assert!(error.to_string().contains("Ken Burns end"), "{error}");
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
        let error = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "geometry={geometry}, error={error}"
        );
    }

    let partial_format = geometry_batch_input(&root, "").replace(
        "width=\"1920\" height=\"1080\" paspH=\"4\" paspV=\"3\"",
        "width=\"1920\" paspH=\"4\"",
    );
    let error = patch_fcpxml_project_batch(partial_format.as_bytes(), None).unwrap_err();
    assert!(error.to_string().contains("format"), "{error}");
    assert!(error.to_string().contains("height"), "{error}");

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
                std::ptr::null(),
                0,
                &mut result,
                &mut error,
            )
        },
        GFStatus::InvalidArgument
    );
    assert!(result.xml.data.is_null());
    assert_eq!(result.xml.len, 0);
    let message = unsafe { std::ffi::CStr::from_ptr((*error).message) }
        .to_string_lossy()
        .into_owned();
    assert!(message.contains("corner"), "{message}");
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
        r#"<asset-clip name="P1004783" ref="a" offset="0s" start="0s" duration="1s"/>"#,
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
    assert_eq!(patched.inserted_count, 1);
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
        r#"<asset-clip name="Present" ref="present" offset="0s" start="0s" duration="1s"/>
        <asset-clip name="Missing" ref="missing" offset="1s" start="0s" duration="1s"/>"#,
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
    assert_eq!(patched.inserted_count, 1);
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
        r#"<asset-clip name="Present" ref="present" offset="0s" start="0s" duration="1s"/>
        <asset-clip name="Missing" ref="missing" offset="1s" start="0s" duration="1s"/>"#,
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
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
fn batch_binds_distinct_sibling_projects_and_inserts_supported_leaf() {
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
        .replace("managed-media candidate", "batch project A");
    let project_b = include_str!("fixtures/phase0-valid.gyroflow")
        .replace("managed-media candidate", "batch project B");
    std::fs::write(root.join("A001.gyroflow"), project_a).unwrap();
    std::fs::write(root.join("B002.gyroflow"), project_b).unwrap();
    std::fs::write(root.join("A001.mov"), b"video bytes must not be read").unwrap();
    std::fs::write(root.join("B002.mov"), b"video bytes must not be read").unwrap();
    let media_a = format!("file://{}", root.join("A001.mov").display());
    let media_b = format!("file://{}", root.join("B002.mov").display());
    let input = format!(
        r#"<fcpxml version="1.14"><resources>
        <format id="r1" frameDuration="1/30s"/>
        <asset id="a" start="0s" duration="10s" format="r1"><media-rep kind="original-media" src="{media_a}"/></asset>
        <asset id="b" start="0s" duration="10s" format="r1"><media-rep kind="original-media" src="{media_b}"/></asset>
        <effect id="fx" uid="{EFFECT_UUID}"/><effect id="other" uid="other"/>
        </resources><project name="Batch" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="20s"><spine>
        <asset-clip name="Existing" ref="a" offset="0s" start="0s" duration="10s"><filter-video-mask><mask-isolation/><filter-video ref="fx" name="Gyroflow NiYien"/></filter-video-mask></asset-clip>
        <asset-clip name="Inserted" ref="b" offset="10s" start="0s" duration="10s"><metadata key="before" value="yes"/><marker start="1s" value="before-filters"/><filter-video ref="other" name="Other"/></asset-clip>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes(), Some("Batch Processed")).unwrap();
    let project_payloads = selected_banked_payloads(&patched.xml, "fx");
    assert_eq!(patched.inserted_count, 1);
    assert_eq!(patched.updated_count, 1);
    assert_eq!(patched.skipped_count, 0);
    assert_eq!(project_payloads.len(), 2);
    assert_ne!(project_payloads[0], project_payloads[1]);
    for payload in project_payloads {
        let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
        assert_eq!(
            unsafe {
                gf_finalcut_instance_load_project_payload(
                    instance,
                    payload.as_ptr(),
                    payload.len(),
                    std::ptr::null_mut(),
                )
            },
            GFStatus::Ok
        );
        unsafe { gf_finalcut_instance_free(instance) };
    }
    let output = std::str::from_utf8(&patched.xml).unwrap();
    let marker = output
        .find("<marker start=\"1s\" value=\"before-filters\"/>")
        .unwrap();
    let other = output
        .find("<filter-video ref=\"other\" name=\"Other\"/>")
        .unwrap();
    let inserted = output[other..]
        .find("<filter-video ref=\"fx\"")
        .map(|offset| other + offset)
        .unwrap();
    assert!(marker < other && other < inserted);
    assert_eq!(payloads(&patched.xml).len(), 2);
    assert!(output.contains("<metadata key=\"before\" value=\"yes\"/>"));
    assert!(output.contains(
        "<filter-video ref=\"fx\" name=\"Gyroflow NiYien\"><param name=\"Instance Identity\""
    ));
    assert_eq!(output.matches("<filter-video ref=\"fx\"").count(), 2);
    let document = roxmltree::Document::parse_with_options(
        output,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    let identities: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("filter-video") && node.attribute("ref") == Some("fx"))
        .map(|filter| {
            let identity = filter
                .children()
                .find(|node| {
                    node.has_tag_name("param")
                        && node.attribute("name") == Some("Instance Identity")
                })
                .unwrap();
            assert_eq!(
                identity.attribute("key"),
                Some("9999/10013/10016/3/10036/1901")
            );
            identity.attribute("value").unwrap()
        })
        .collect();
    assert_eq!(identities.len(), 2);
    assert!(identities.iter().all(|identity| identity.len() == 36));
    assert_ne!(identities[0], identities[1]);

    let name = b"Batch C ABI";
    let mut result = GFRouteDPatchResult::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_route_d_batch_patch(
                input.as_ptr(),
                input.len(),
                name.as_ptr(),
                name.len(),
                &mut result,
                &mut error,
            )
        },
        GFStatus::Ok
    );
    assert!(error.is_null());
    let report = unsafe { std::slice::from_raw_parts(result.report.data, result.report.len) };
    let report: serde_json::Value = serde_json::from_slice(report).unwrap();
    assert_eq!(report["inserted_count"], 1);
    assert_eq!(report["updated_count"], 1);
    assert_eq!(report["failed_count"], 0);
    assert_eq!(report["targets"].as_array().unwrap().len(), 2);
    unsafe { gf_finalcut_route_d_patch_result_free(&mut result) };

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_inserts_unique_production_effect_resource_when_absent() {
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
        </resources><project name="P" uid="11111111-1111-4111-8111-111111111111"><sequence format="r1" duration="1s"><spine>
        <asset-clip name="Leaf" ref="r2" offset="0s" start="0s" duration="1s"/>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
    let document = roxmltree::Document::parse_with_options(
        std::str::from_utf8(&patched.xml).unwrap(),
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .unwrap();
    let effects: Vec<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("effect") && node.attribute("uid") == Some(EFFECT_TEMPLATE_UID)
        })
        .collect();
    assert_eq!(effects.len(), 1);
    let effect_ref = effects[0].attribute("id").unwrap();
    assert_eq!(
        document
            .descendants()
            .filter(|node| {
                node.has_tag_name("filter-video") && node.attribute("ref") == Some(effect_ref)
            })
            .count(),
        1
    );
    assert_eq!(patched.inserted_count, 1);
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

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
    assert_eq!(patched.updated_count, 1);

    let different_output_rate = input.replace(
        "<sequence format=\"source\"",
        "<sequence format=\"different\"",
    );
    assert!(
        patch_fcpxml_project_batch(different_output_rate.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("conform-rate")
    );

    let wrong_label = input.replace("srcFrameRate=\"59.94\"", "srcFrameRate=\"30\"");
    assert!(
        patch_fcpxml_project_batch(wrong_label.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("conform-rate")
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
        </timeMap><video ref="a" offset="0s" start="0s" duration="2s"/>
        <metadata><md key="keep" value="yes"/></metadata></clip>
        </spine></sequence></project></fcpxml>"#
    );

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
    assert_eq!(patched.inserted_count, 1);
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
    assert!(patch_fcpxml_project_batch(ambiguous.as_bytes(), None).is_err());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_matrix_preserves_banks_skips_complex_and_fails_without_partial_output() {
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

    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
    let output = std::str::from_utf8(&patched.xml).unwrap();
    assert_eq!(output.matches(&valid_banks).count(), 1);
    assert_eq!(patched.updated_count, 1);
    assert_eq!(patched.inserted_count, 1);
    assert_eq!(patched.skipped_count, 2);
    assert_eq!(
        output
            .matches("<filter-video ref=\"fx\" name=\"Gyroflow NiYien\">")
            .count(),
        1
    );
    assert!(patched.targets.iter().any(|target| {
        target.clip_name == "Unsupported Sync"
            && target.action == gyroflow_finalcut::BatchTargetAction::Skipped
    }));
    assert!(patched.targets.iter().any(|target| {
        target.clip_name == "Unsupported Compound"
            && target.action == gyroflow_finalcut::BatchTargetAction::Skipped
    }));

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
                std::ptr::null(),
                0,
                &mut result,
                &mut error,
            )
        },
        GFStatus::InvalidArgument
    );
    assert!(result.xml.data.is_null());
    assert_eq!(result.xml.len, 0);
    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_route_d_patch_result_free(&mut result);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn batch_missing_corrupt_ambiguous_over_capacity_and_resource_conflicts_fail_closed() {
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
        r#"<asset-clip name="Good" ref="good" offset="0s" start="0s" duration="1s"></asset-clip>
        <asset-clip name="Missing" ref="missing" offset="1s" start="0s" duration="1s"></asset-clip>"#,
        &effects,
    );
    let patched = patch_fcpxml_project_batch(missing_is_skipped.as_bytes(), None).unwrap();
    assert_eq!(patched.inserted_count, 1);
    assert_eq!(patched.skipped_count, 1);

    let corrupt_target = base(
        &corrupt_asset,
        r#"<asset-clip name="Corrupt" ref="corrupt" offset="0s" start="0s" duration="1s"></asset-clip>"#,
        &effects,
    );
    assert!(
        patch_fcpxml_project_batch(corrupt_target.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("is invalid")
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
        patch_fcpxml_project_batch(ambiguous_existing.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("exactly one original-media")
    );

    let duplicate_resources = base(
        &good_asset,
        r#"<asset-clip name="Good" ref="good" offset="0s" start="0s" duration="1s"></asset-clip>"#,
        &format!(
            r#"<effect id="fx" uid="{EFFECT_UUID}"/><effect id="fx2" uid="{EFFECT_TEMPLATE_UID}"/>"#
        ),
    );
    assert!(
        patch_fcpxml_project_batch(duplicate_resources.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("at most one")
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
        r#"<asset-clip name="Oversized" ref="oversized" offset="0s" start="0s" duration="1s"></asset-clip>"#,
        &effects,
    );
    assert!(
        patch_fcpxml_project_batch(oversized_target.as_bytes(), None)
            .unwrap_err()
            .to_string()
            .contains("4 MiB")
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
        <asset-clip name="Geometry" ref="a" offset="0s" start="0s" duration="1s"/>
        </spine></sequence></project></fcpxml>"#
    );
    let patched = patch_fcpxml_project_batch(input.as_bytes(), None).unwrap();
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
