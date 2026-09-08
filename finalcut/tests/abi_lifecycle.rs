use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use gyroflow_finalcut::{
    GF_FRAME_GEOMETRY_VERSION, GF_GEOMETRY_SUPPORT_AFFINE_2D,
    GF_GEOMETRY_SUPPORT_PERSPECTIVE_UNVERIFIED, GF_GEOMETRY_VALIDITY_VALID,
    GF_IMAGE_ORIGIN_TOP_LEFT, GF_ROTATION_CLOCKWISE_90, GF_ROTATION_NONE, GFAffineTransform,
    GFDimensionsU32, GFError, GFFrameGeometry, GFFrameGeometryMode, GFOwnedBytes,
    GFProjectGeometry, GFRectI32, GFRenderParameters, GFStatus, gf_finalcut_error_free,
    gf_finalcut_instance_create, gf_finalcut_instance_free,
    gf_finalcut_instance_get_project_geometry, gf_finalcut_instance_get_project_render_parameters,
    gf_finalcut_instance_has_project, gf_finalcut_instance_load_project,
    gf_finalcut_instance_load_project_payload, gf_finalcut_instance_set_render_parameters,
    gf_finalcut_owned_bytes_free, gf_finalcut_project_payload_decode,
    gf_finalcut_project_payload_encode, validate_frame_geometry,
};
use sha2::{Digest, Sha256};
use std::ffi::CStr;
use std::io::{Read, Write};

fn valid_frame_geometry() -> GFFrameGeometry {
    GFFrameGeometry {
        version: GF_FRAME_GEOMETRY_VERSION,
        struct_size: std::mem::size_of::<GFFrameGeometry>() as u32,
        validity: GF_GEOMETRY_VALIDITY_VALID,
        support: GF_GEOMETRY_SUPPORT_AFFINE_2D,
        source_dimensions: GFDimensionsU32 {
            width: 1920,
            height: 1080,
        },
        oriented_dimensions: GFDimensionsU32 {
            width: 1920,
            height: 1080,
        },
        tile_dimensions: GFDimensionsU32 {
            width: 960,
            height: 540,
        },
        output_dimensions: GFDimensionsU32 {
            width: 1920,
            height: 1080,
        },
        source_rect: GFRectI32 {
            left: 0,
            bottom: 0,
            right: 960,
            top: 540,
        },
        destination_rect: GFRectI32 {
            left: 960,
            bottom: 540,
            right: 1920,
            top: 1080,
        },
        source_origin: GF_IMAGE_ORIGIN_TOP_LEFT,
        destination_origin: GF_IMAGE_ORIGIN_TOP_LEFT,
        input_rotation: GF_ROTATION_NONE,
        video_rotation: GF_ROTATION_CLOCKWISE_90,
        forward_transform: GFAffineTransform {
            values: [1.0, 0.0, 960.0, 0.0, 1.0, 540.0, 0.0, 0.0, 1.0],
        },
        inverse_transform: GFAffineTransform {
            values: [1.0, 0.0, -960.0, 0.0, 1.0, -540.0, 0.0, 0.0, 1.0],
        },
        reserved: [0; 4],
    }
}

#[test]
fn frame_geometry_layout_is_fixed_and_zero_initialization_is_legacy_unknown() {
    assert_eq!(std::mem::size_of::<GFDimensionsU32>(), 8);
    assert_eq!(std::mem::size_of::<GFRectI32>(), 16);
    assert_eq!(std::mem::size_of::<GFAffineTransform>(), 72);
    assert_eq!(std::mem::size_of::<GFFrameGeometry>(), 256);
    assert_eq!(std::mem::align_of::<GFFrameGeometry>(), 8);
    assert_eq!(std::mem::offset_of!(GFFrameGeometry, source_dimensions), 16);
    assert_eq!(std::mem::offset_of!(GFFrameGeometry, source_rect), 48);
    assert_eq!(std::mem::offset_of!(GFFrameGeometry, forward_transform), 96);
    assert_eq!(
        std::mem::offset_of!(GFFrameGeometry, inverse_transform),
        168
    );
    assert_eq!(std::mem::offset_of!(GFFrameGeometry, reserved), 240);

    assert_eq!(
        validate_frame_geometry(&GFFrameGeometry::default()),
        Ok(GFFrameGeometryMode::LegacyUnknown)
    );
    assert_eq!(
        validate_frame_geometry(&valid_frame_geometry()),
        Ok(GFFrameGeometryMode::ValidatedAffine)
    );
}

#[test]
fn frame_geometry_rejects_partial_legacy_and_wrong_version_or_size() {
    let mut geometry = GFFrameGeometry::default();
    geometry.source_dimensions.width = 1920;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("legacy geometry")
    );

    geometry = valid_frame_geometry();
    geometry.version += 1;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("version")
    );

    geometry = valid_frame_geometry();
    geometry.struct_size -= 8;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("size")
    );
}

#[test]
fn frame_geometry_rejects_unknown_enums_and_unverified_perspective() {
    let mut geometry = valid_frame_geometry();
    geometry.validity = 99;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("validity")
    );

    geometry = valid_frame_geometry();
    geometry.source_origin = 99;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("origin")
    );

    geometry = valid_frame_geometry();
    geometry.input_rotation = 99;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("rotation")
    );

    geometry = valid_frame_geometry();
    geometry.support = GF_GEOMETRY_SUPPORT_PERSPECTIVE_UNVERIFIED;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("perspective")
    );
}

#[test]
fn frame_geometry_rejects_dimension_conflicts() {
    let mut geometry = valid_frame_geometry();
    geometry.source_dimensions.width = 0;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("dimensions")
    );

    geometry = valid_frame_geometry();
    geometry.destination_rect.right = geometry.destination_rect.left;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("positive extents")
    );
}

#[test]
fn frame_geometry_keeps_physical_tiles_separate_from_narrow_pasp_output() {
    let cases = [
        ("8:9 full", (720, 480), (640, 480), (640, 480)),
        ("8:9 proxy", (360, 240), (640, 480), (640, 480)),
        ("10:11 full", (704, 480), (640, 480), (640, 480)),
        ("10:11 proxy", (352, 240), (640, 480), (640, 480)),
    ];

    for (label, tile, output, logical_rect) in cases {
        let mut geometry = valid_frame_geometry();
        geometry.tile_dimensions = GFDimensionsU32 {
            width: tile.0,
            height: tile.1,
        };
        geometry.output_dimensions = GFDimensionsU32 {
            width: output.0,
            height: output.1,
        };
        geometry.source_rect = GFRectI32 {
            left: 0,
            bottom: 0,
            right: logical_rect.0,
            top: logical_rect.1,
        };
        geometry.destination_rect = geometry.source_rect;

        assert_eq!(
            validate_frame_geometry(&geometry),
            Ok(GFFrameGeometryMode::ValidatedAffine),
            "{label}"
        );
    }
}

#[test]
fn frame_geometry_rejects_nonfinite_singular_and_inconsistent_affines() {
    let mut geometry = valid_frame_geometry();
    geometry.forward_transform.values[0] = f64::NAN;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("finite")
    );

    geometry = valid_frame_geometry();
    geometry.forward_transform.values[0] = 0.0;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("singular")
    );

    geometry = valid_frame_geometry();
    geometry.inverse_transform.values[2] = 1.0;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("inverse")
    );

    geometry = valid_frame_geometry();
    geometry.forward_transform.values[6] = 0.25;
    assert!(
        validate_frame_geometry(&geometry)
            .unwrap_err()
            .contains("affine")
    );
}

#[test]
fn opaque_instance_has_an_explicit_null_safe_lifecycle() {
    let mut error: *mut GFError = std::ptr::null_mut();
    let instance = unsafe { gf_finalcut_instance_create(&mut error) };

    assert!(!instance.is_null());
    assert!(error.is_null());

    unsafe {
        gf_finalcut_instance_free(std::ptr::null_mut());
        gf_finalcut_instance_free(instance);
        gf_finalcut_error_free(std::ptr::null_mut());
    }
}

#[test]
fn instance_creation_does_not_require_an_error_slot() {
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    assert!(!instance.is_null());

    unsafe { gf_finalcut_instance_free(instance) };
}

#[test]
fn loaded_project_geometry_is_available_through_a_read_only_abi_getter() {
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let mut geometry = GFProjectGeometry::default();
    let missing = unsafe {
        gf_finalcut_instance_get_project_geometry(instance, &mut geometry, std::ptr::null_mut())
    };
    assert_eq!(missing, GFStatus::InvalidProject);

    let project = br#"{
        "version": 3,
        "videofile": "",
        "video_info": {
            "width": 1920,
            "height": 1080,
            "rotation": 0,
            "fps": 24.0,
            "duration_ms": 1000.0,
            "num_frames": 24
        },
        "gyro_source": {},
        "output": {"output_width": 1920, "output_height": 1080}
    }"#;
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
    let loaded = unsafe {
        gf_finalcut_instance_get_project_geometry(instance, &mut geometry, std::ptr::null_mut())
    };
    assert_eq!(loaded, GFStatus::Ok);
    assert_eq!(
        geometry.input_dimensions,
        GFDimensionsU32 {
            width: 1920,
            height: 1080
        }
    );
    assert_eq!(
        geometry.output_dimensions,
        GFDimensionsU32 {
            width: 1920,
            height: 1080
        }
    );
    assert_eq!(geometry.video_rotation, GF_ROTATION_NONE);
    assert_eq!(geometry.reserved, 0);

    let null_output = unsafe {
        gf_finalcut_instance_get_project_geometry(
            instance,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(null_output, GFStatus::NullPointer);
    unsafe { gf_finalcut_instance_free(instance) };
}

#[test]
fn invalid_project_bytes_do_not_replace_the_committed_project() {
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    assert!(!instance.is_null());
    let valid = include_bytes!("fixtures/phase0-valid.gyroflow");
    let mut error: *mut GFError = std::ptr::null_mut();

    let first_status = unsafe {
        gf_finalcut_instance_load_project(instance, valid.as_ptr(), valid.len(), &mut error)
    };
    let first_message = if error.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr((*error).message) }
            .to_string_lossy()
            .into_owned()
    };
    assert_eq!(first_status, GFStatus::Ok, "{first_message}");
    assert!(error.is_null());
    assert_eq!(unsafe { gf_finalcut_instance_has_project(instance) }, 1);

    let invalid = b"not a gyroflow project";
    let second_status = unsafe {
        gf_finalcut_instance_load_project(instance, invalid.as_ptr(), invalid.len(), &mut error)
    };
    assert_eq!(second_status, GFStatus::InvalidProject);
    assert!(!error.is_null());
    let message = unsafe { CStr::from_ptr((*error).message) }
        .to_string_lossy()
        .into_owned();
    assert!(message.contains("gyroflow project"), "{message}");
    assert_eq!(unsafe { gf_finalcut_instance_has_project(instance) }, 1);

    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_instance_free(instance);
    }
}

#[test]
fn project_load_rejects_null_instance_and_empty_bytes() {
    let valid = include_bytes!("fixtures/phase0-valid.gyroflow");
    let mut error: *mut GFError = std::ptr::null_mut();

    let null_instance_status = unsafe {
        gf_finalcut_instance_load_project(
            std::ptr::null_mut(),
            valid.as_ptr(),
            valid.len(),
            &mut error,
        )
    };
    assert_eq!(null_instance_status, GFStatus::NullPointer);
    unsafe { gf_finalcut_error_free(error) };

    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    error = std::ptr::null_mut();
    let empty_status =
        unsafe { gf_finalcut_instance_load_project(instance, std::ptr::null(), 0, &mut error) };
    assert_eq!(empty_status, GFStatus::InvalidProject);
    assert_eq!(unsafe { gf_finalcut_instance_has_project(instance) }, 0);

    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_instance_free(instance);
    }
}

fn encoded_payload(project: &[u8]) -> Vec<u8> {
    let mut payload = GFOwnedBytes::default();
    let mut error: *mut GFError = std::ptr::null_mut();
    let status = unsafe {
        gf_finalcut_project_payload_encode(
            project.as_ptr(),
            project.len(),
            &mut payload,
            &mut error,
        )
    };
    assert_eq!(status, GFStatus::Ok);
    assert!(error.is_null());
    let copied = unsafe { std::slice::from_raw_parts(payload.data, payload.len) }.to_vec();
    unsafe { gf_finalcut_owned_bytes_free(&mut payload) };
    copied
}

#[test]
fn project_payload_envelope_round_trips_version_content_and_hash() {
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    let payload = encoded_payload(project);
    let envelope_bytes = STANDARD.decode(&payload).expect("outer Base64");
    assert_eq!(&envelope_bytes[..7], b"GFPRJ2\0");
    assert_eq!(
        u64::from_be_bytes(envelope_bytes[7..15].try_into().unwrap()),
        project.len() as u64
    );
    assert_eq!(&envelope_bytes[15..47], Sha256::digest(project).as_slice());
    let compressed = &envelope_bytes[47..];
    let mut decoded = Vec::new();
    ZlibDecoder::new(compressed)
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, project);

    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let mut error: *mut GFError = std::ptr::null_mut();
    let status = unsafe {
        gf_finalcut_instance_load_project_payload(
            instance,
            payload.as_ptr(),
            payload.len(),
            &mut error,
        )
    };
    assert_eq!(status, GFStatus::Ok);
    assert!(error.is_null());
    assert_eq!(unsafe { gf_finalcut_instance_has_project(instance) }, 1);
    unsafe { gf_finalcut_instance_free(instance) };
}

#[test]
fn legacy_v1_project_payload_remains_loadable() {
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(project).unwrap();
    let compressed = encoder.finish().unwrap();
    let envelope = serde_json::json!({
        "version": 1,
        "codec": "zlib",
        "uncompressed_len": project.len(),
        "content_sha256": format!("{:x}", Sha256::digest(project)),
        "project_base64": STANDARD.encode(compressed),
    });
    let payload = STANDARD.encode(serde_json::to_vec(&envelope).unwrap());
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

#[test]
fn project_payload_decode_round_trips_v1_and_v2_payloads() {
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    let v2_payload = encoded_payload(project);

    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(project).unwrap();
    let compressed = encoder.finish().unwrap();
    let v1_payload = STANDARD
        .encode(
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "codec": "zlib",
                "uncompressed_len": project.len(),
                "content_sha256": format!("{:x}", Sha256::digest(project)),
                "project_base64": STANDARD.encode(compressed),
            }))
            .unwrap(),
        )
        .into_bytes();

    for payload in [&v1_payload, &v2_payload] {
        let mut decoded = GFOwnedBytes::default();
        let mut error: *mut GFError = std::ptr::null_mut();
        let status = unsafe {
            gf_finalcut_project_payload_decode(
                payload.as_ptr(),
                payload.len(),
                &mut decoded,
                &mut error,
            )
        };

        assert_eq!(status, GFStatus::Ok);
        assert!(error.is_null());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(decoded.data, decoded.len) },
            project
        );
        unsafe { gf_finalcut_owned_bytes_free(&mut decoded) };
    }
}

#[test]
fn project_payload_decode_rejects_unknown_versions_and_hash_mismatches() {
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    let unknown = STANDARD.encode(
        serde_json::to_vec(&serde_json::json!({
            "version": 99,
            "codec": "zlib",
            "uncompressed_len": project.len(),
            "content_sha256": format!("{:x}", Sha256::digest(project)),
            "project_base64": STANDARD.encode(project),
        }))
        .unwrap(),
    );
    let mut v2 = STANDARD.decode(encoded_payload(project)).unwrap();
    v2[15] ^= 0xff;
    let hash_mismatch = STANDARD.encode(v2);

    for (payload, expected_status) in [
        (unknown.as_bytes(), GFStatus::UnknownPayloadVersion),
        (hash_mismatch.as_bytes(), GFStatus::InvalidProject),
    ] {
        let mut decoded = GFOwnedBytes {
            data: std::ptr::dangling_mut(),
            len: 17,
            capacity: 23,
        };
        let mut error: *mut GFError = std::ptr::null_mut();
        let status = unsafe {
            gf_finalcut_project_payload_decode(
                payload.as_ptr(),
                payload.len(),
                &mut decoded,
                &mut error,
            )
        };

        assert_eq!(status, expected_status);
        assert_eq!(decoded.data, std::ptr::null_mut());
        assert_eq!(decoded.len, 0);
        assert_eq!(decoded.capacity, 0);
        unsafe { gf_finalcut_error_free(error) };
    }
}

#[test]
fn project_payload_decode_requires_input_and_output_pointers() {
    let payload = encoded_payload(include_bytes!("fixtures/phase0-valid.gyroflow"));
    let mut decoded = GFOwnedBytes::default();

    assert_eq!(
        unsafe {
            gf_finalcut_project_payload_decode(
                std::ptr::null(),
                payload.len(),
                &mut decoded,
                std::ptr::null_mut(),
            )
        },
        GFStatus::NullPointer
    );
    assert_eq!(
        unsafe {
            gf_finalcut_project_payload_decode(
                payload.as_ptr(),
                payload.len(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        GFStatus::NullPointer
    );
}

#[test]
fn project_render_parameters_getter_requires_a_loaded_project_and_valid_pointers() {
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let mut parameters = GFRenderParameters {
        fov: -1.0,
        smoothness: -1.0,
        lens_correction: -1.0,
        horizon_lock_amount: -1.0,
        horizon_lock_roll: -1.0,
        zoom_mode: -1,
        overview: 2,
        reserved: [9; 3],
    };
    let mut error: *mut GFError = std::ptr::null_mut();

    assert_eq!(
        unsafe {
            gf_finalcut_instance_get_project_render_parameters(
                instance,
                &mut parameters,
                &mut error,
            )
        },
        GFStatus::InvalidProject
    );
    assert_eq!(
        parameters,
        GFRenderParameters {
            fov: 0.0,
            smoothness: 0.0,
            lens_correction: 0.0,
            horizon_lock_amount: 0.0,
            horizon_lock_roll: 0.0,
            zoom_mode: 0,
            overview: 0,
            reserved: [0; 3],
        }
    );
    unsafe { gf_finalcut_error_free(error) };
    assert_eq!(
        unsafe {
            gf_finalcut_instance_get_project_render_parameters(
                std::ptr::null(),
                &mut parameters,
                std::ptr::null_mut(),
            )
        },
        GFStatus::NullPointer
    );
    assert_eq!(
        unsafe {
            gf_finalcut_instance_get_project_render_parameters(
                instance,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        GFStatus::NullPointer
    );

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
    let expected = representative_parameters();
    assert_eq!(
        unsafe {
            gf_finalcut_instance_set_render_parameters(instance, &expected, std::ptr::null_mut())
        },
        GFStatus::Ok
    );
    assert_eq!(
        unsafe {
            gf_finalcut_instance_get_project_render_parameters(
                instance,
                &mut parameters,
                std::ptr::null_mut(),
            )
        },
        GFStatus::Ok
    );
    assert_eq!(parameters, expected);

    unsafe { gf_finalcut_instance_free(instance) };
}

#[test]
fn raw_project_cap_is_checked_before_reading_caller_memory() {
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let unreadable = std::ptr::dangling::<u8>();
    let oversized = 256 * 1024 * 1024 + 1;
    let mut encoded = GFOwnedBytes {
        data: std::ptr::dangling_mut(),
        len: 17,
        capacity: 23,
    };

    assert_eq!(
        unsafe {
            gf_finalcut_instance_load_project(instance, unreadable, oversized, std::ptr::null_mut())
        },
        GFStatus::InvalidProject
    );
    assert_eq!(
        unsafe {
            gf_finalcut_project_payload_encode(
                unreadable,
                oversized,
                &mut encoded,
                std::ptr::null_mut(),
            )
        },
        GFStatus::InvalidProject
    );
    assert!(encoded.data.is_null());
    assert_eq!(encoded.len, 0);
    assert_eq!(encoded.capacity, 0);

    unsafe { gf_finalcut_instance_free(instance) };
}

#[test]
fn unknown_or_tampered_payload_fails_without_replacing_project() {
    let project = include_bytes!("fixtures/phase0-valid.gyroflow");
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let initial = unsafe {
        gf_finalcut_instance_load_project(
            instance,
            project.as_ptr(),
            project.len(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(initial, GFStatus::Ok);

    let legacy = serde_json::json!({
        "version": 99,
        "codec": "zlib",
        "uncompressed_len": project.len(),
        "content_sha256": format!("{:x}", Sha256::digest(project)),
        "project_base64": STANDARD.encode(project),
    });
    let unknown = STANDARD.encode(serde_json::to_vec(&legacy).unwrap());
    let mut error: *mut GFError = std::ptr::null_mut();
    let unknown_status = unsafe {
        gf_finalcut_instance_load_project_payload(
            instance,
            unknown.as_ptr(),
            unknown.len(),
            &mut error,
        )
    };
    assert_eq!(unknown_status, GFStatus::UnknownPayloadVersion);
    unsafe { gf_finalcut_error_free(error) };

    let payload = encoded_payload(project);
    let mut envelope = STANDARD.decode(&payload).unwrap();
    envelope[15] ^= 0xff;
    let tampered = STANDARD.encode(envelope);
    error = std::ptr::null_mut();
    let tampered_status = unsafe {
        gf_finalcut_instance_load_project_payload(
            instance,
            tampered.as_ptr(),
            tampered.len(),
            &mut error,
        )
    };
    assert_eq!(tampered_status, GFStatus::InvalidProject);
    unsafe { gf_finalcut_error_free(error) };

    error = std::ptr::null_mut();
    let corrupt = b"not base64";
    let corrupt_status = unsafe {
        gf_finalcut_instance_load_project_payload(
            instance,
            corrupt.as_ptr(),
            corrupt.len(),
            &mut error,
        )
    };
    assert_eq!(corrupt_status, GFStatus::InvalidProject);
    assert_eq!(unsafe { gf_finalcut_instance_has_project(instance) }, 1);

    unsafe {
        gf_finalcut_error_free(error);
        gf_finalcut_instance_free(instance);
    }
}

fn representative_parameters() -> GFRenderParameters {
    GFRenderParameters {
        fov: 1.25,
        smoothness: 42.0,
        lens_correction: 80.0,
        horizon_lock_amount: 30.0,
        horizon_lock_roll: 5.0,
        zoom_mode: 2,
        overview: 1,
        reserved: [0; 3],
    }
}

#[test]
fn render_parameters_require_a_project_and_reject_non_finite_values() {
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let mut parameters = representative_parameters();
    let mut error: *mut GFError = std::ptr::null_mut();
    let missing_project =
        unsafe { gf_finalcut_instance_set_render_parameters(instance, &parameters, &mut error) };
    assert_eq!(missing_project, GFStatus::InvalidProject);
    unsafe { gf_finalcut_error_free(error) };

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
    parameters.fov = f64::NAN;
    error = std::ptr::null_mut();
    let invalid =
        unsafe { gf_finalcut_instance_set_render_parameters(instance, &parameters, &mut error) };
    assert_eq!(invalid, GFStatus::InvalidArgument);
    unsafe { gf_finalcut_error_free(error) };

    parameters = representative_parameters();
    let valid = unsafe {
        gf_finalcut_instance_set_render_parameters(instance, &parameters, std::ptr::null_mut())
    };
    assert_eq!(valid, GFStatus::Ok);
    unsafe { gf_finalcut_instance_free(instance) };
}
