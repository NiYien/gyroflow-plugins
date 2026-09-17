// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn load_through_plugin(path: &std::path::Path) -> Arc<StabilizationManager> {
    let mut instance = GyroflowPluginBaseInstance {
        reload_values_from_project: true,
        ..Default::default()
    };
    let mut params = TestParams::default();
    params.set_string(Params::ProjectPath, &path.to_string_lossy()).unwrap();
    params.set_string(Params::InstanceId, "project-compatibility").unwrap();
    let cache = Mutex::new(LruCache::new(std::num::NonZeroUsize::new(1).unwrap()));
    instance.stab_manager(&mut params, &cache, (0, 0), false).unwrap()
}

#[test]
fn embedded_gyro_survives_scalar_and_dual_axis_lens_metadata() {
    let dir = seq_tmp();
    // An accessible external file must not replace the embedded, rebased samples.
    std::fs::write(dir.join("recording.bin"), b"external gyro must not be read").unwrap();
    for (name, data, expected_focal) in [
        ("scalar", include_str!("../../tests/fixtures/embedded-gyro-scalar.gyroflow"), (500.0, 500.0)),
        ("dual-axis", include_str!("../../tests/fixtures/embedded-gyro-dual-axis.gyroflow"), (500.0, 550.0)),
    ] {
        let path = dir.join(format!("{name}.gyroflow"));
        std::fs::write(&path, data).unwrap();
        let stab = load_through_plugin(&path);
        let gyro = stab.gyro.read();
        let metadata = gyro.file_metadata.read();
        assert_eq!(metadata.raw_imu.len(), 65, "{name}");
        assert_eq!(metadata.raw_imu.first().unwrap().timestamp_ms, 0.0);
        assert_eq!(metadata.raw_imu.last().unwrap().timestamp_ms, 640.0);
        assert_eq!(metadata.lens_params[&0].pixel_focal_length, Some(expected_focal));
        assert_eq!(gyro.quaternions.len(), 65);
        assert_eq!(gyro.get_offsets(), &BTreeMap::from([(200_000, -100.0)]));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires GYROFLOW_COMPAT_FIXTURE pointing to an original feedback project"]
fn feedback_project_preserves_embedded_samples_and_sync_offsets() {
    let path = std::path::PathBuf::from(std::env::var("GYROFLOW_COMPAT_FIXTURE").unwrap());
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let expected: gyroflow_core::gyro_source::FileMetadata = gyroflow_core::util::decompress_from_base91_cbor(
        json["gyro_source"]["file_metadata"].as_str().unwrap(),
    ).unwrap();
    let offsets: BTreeMap<i64, f64> = serde_json::from_value(json["offsets"].clone()).unwrap();
    assert!(!expected.raw_imu.is_empty());
    let stab = load_through_plugin(&path);
    let gyro = stab.gyro.read();
    let metadata = gyro.file_metadata.read();
    assert_eq!(metadata.raw_imu.len(), expected.raw_imu.len());
    for (actual, expected) in metadata.raw_imu.iter().zip(&expected.raw_imu) {
        assert_eq!(actual.timestamp_ms, expected.timestamp_ms);
        assert_eq!(actual.gyro, expected.gyro);
        assert_eq!(actual.accl, expected.accl);
    }
    assert_eq!(metadata.lens_params.len(), expected.lens_params.len());
    assert_eq!(gyro.get_offsets(), &offsets);
    assert_eq!(gyro.quaternions.len(), expected.raw_imu.len());
    eprintln!("plugin feedback import: samples={} lens_params={} offsets={:?}", metadata.raw_imu.len(), metadata.lens_params.len(), gyro.get_offsets());
}
