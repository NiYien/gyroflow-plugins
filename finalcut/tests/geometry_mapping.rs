use gyroflow_finalcut::{
    GF_FRAME_GEOMETRY_VERSION, GF_GEOMETRY_SUPPORT_AFFINE_2D, GF_GEOMETRY_VALIDITY_VALID,
    GF_IMAGE_ORIGIN_BOTTOM_LEFT, GF_IMAGE_ORIGIN_TOP_LEFT, GF_ROTATION_180,
    GF_ROTATION_CLOCKWISE_90, GF_ROTATION_CLOCKWISE_270, GF_ROTATION_NONE, GFAffineTransform,
    GFDimensionsU32, GFFrameGeometry, GFFrameGeometryMode, GFProjectGeometry, GFRectI32, GFStatus,
    map_frame_geometry,
};

fn dimensions(width: u32, height: u32) -> GFDimensionsU32 {
    GFDimensionsU32 { width, height }
}

fn rect(left: i32, bottom: i32, right: i32, top: i32) -> GFRectI32 {
    GFRectI32 {
        left,
        bottom,
        right,
        top,
    }
}

fn affine(values: [f64; 9]) -> GFAffineTransform {
    GFAffineTransform { values }
}

fn identity_affine() -> GFAffineTransform {
    affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
}

fn geometry(
    source: GFDimensionsU32,
    oriented: GFDimensionsU32,
    tile: GFDimensionsU32,
    output: GFDimensionsU32,
    input_rotation: i32,
    video_rotation: i32,
) -> GFFrameGeometry {
    GFFrameGeometry {
        version: GF_FRAME_GEOMETRY_VERSION,
        struct_size: std::mem::size_of::<GFFrameGeometry>() as u32,
        validity: GF_GEOMETRY_VALIDITY_VALID,
        support: GF_GEOMETRY_SUPPORT_AFFINE_2D,
        source_dimensions: source,
        oriented_dimensions: oriented,
        tile_dimensions: tile,
        output_dimensions: output,
        source_rect: rect(0, 0, tile.width as i32, tile.height as i32),
        destination_rect: rect(0, 0, tile.width as i32, tile.height as i32),
        source_origin: GF_IMAGE_ORIGIN_TOP_LEFT,
        destination_origin: GF_IMAGE_ORIGIN_TOP_LEFT,
        input_rotation,
        video_rotation,
        forward_transform: identity_affine(),
        inverse_transform: identity_affine(),
        reserved: [0; 4],
    }
}

fn project(
    input: GFDimensionsU32,
    output: GFDimensionsU32,
    video_rotation: i32,
) -> GFProjectGeometry {
    GFProjectGeometry {
        input_dimensions: input,
        output_dimensions: output,
        video_rotation,
        reserved: 0,
    }
}

fn assert_near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.0e-5,
        "actual={actual}, expected={expected}"
    );
}

#[test]
fn geometry_v0_preserves_legacy_buffer_behavior() {
    let mapped = map_frame_geometry(
        &GFFrameGeometry::default(),
        dimensions(640, 360),
        project(dimensions(3840, 2160), dimensions(2160, 3840), 90),
    )
    .expect("zero geometry must preserve the legacy path");

    assert_eq!(mapped.mode, GFFrameGeometryMode::LegacyUnknown);
    assert_eq!(mapped.stabilization_input_dimensions, None);
    assert_eq!(mapped.stabilization_output_dimensions, None);
    assert_eq!(mapped.video_rotation, None);
    assert_eq!(mapped.input.dimensions, dimensions(640, 360));
    assert_eq!(mapped.output.dimensions, dimensions(640, 360));
    assert_eq!(mapped.input.rect, None);
    assert_eq!(mapped.output.rect, None);
    assert_eq!(mapped.input.rotation, None);
    assert!(mapped.output.post_affine.is_none());
}

#[test]
fn geometry_quarter_turns_keep_project_raster_and_map_input_rotation() {
    let source = dimensions(3840, 2160);
    let tile = dimensions(960, 540);
    let cases = [
        (GF_ROTATION_NONE, dimensions(3840, 2160)),
        (GF_ROTATION_CLOCKWISE_90, dimensions(2160, 3840)),
        (GF_ROTATION_180, dimensions(3840, 2160)),
        (GF_ROTATION_CLOCKWISE_270, dimensions(2160, 3840)),
    ];

    for (rotation, oriented) in cases {
        let mapped = map_frame_geometry(
            &geometry(source, oriented, tile, oriented, rotation, rotation),
            tile,
            project(source, oriented, rotation),
        )
        .unwrap_or_else(|error| panic!("rotation {rotation}: {}", error.message));

        assert_eq!(mapped.mode, GFFrameGeometryMode::ValidatedAffine);
        assert_eq!(mapped.stabilization_input_dimensions, Some(source));
        assert_eq!(mapped.stabilization_output_dimensions, Some(oriented));
        assert_eq!(mapped.video_rotation, Some(rotation));
        assert_eq!(mapped.input.dimensions, tile);
        assert_eq!(mapped.output.dimensions, tile);
        assert_eq!(mapped.input.rect, Some((0, 0, 960, 540)));
        assert_eq!(mapped.output.rect, Some((0, 0, 960, 540)));
        assert_eq!(mapped.input.rotation, Some(rotation as f32));
        assert!(mapped.output.post_affine.is_none());
    }
}

#[test]
fn geometry_true_portrait_does_not_adopt_the_landscape_final_cut_canvas() {
    let source = dimensions(1080, 1920);
    let project_output = dimensions(1080, 1920);
    let tile = dimensions(960, 540);
    let final_cut_canvas = dimensions(1920, 1080);

    let mapped = map_frame_geometry(
        &geometry(
            source,
            project_output,
            tile,
            final_cut_canvas,
            GF_ROTATION_NONE,
            GF_ROTATION_NONE,
        ),
        tile,
        project(source, project_output, GF_ROTATION_NONE),
    )
    .expect("a portrait project may render into a landscape host tile");

    assert_eq!(mapped.stabilization_input_dimensions, Some(source));
    assert_eq!(mapped.stabilization_output_dimensions, Some(project_output));
    assert_ne!(
        mapped.stabilization_output_dimensions,
        Some(final_cut_canvas)
    );
    assert_eq!(mapped.output.dimensions, tile);
    let post_affine = mapped
        .output
        .post_affine
        .expect("the host canvas is recentered only in PostAffine");
    assert_near(post_affine.zoom, 1.0);
    assert_near(post_affine.rotation_deg, 0.0);
    assert_near(post_affine.offset_norm[0], -420.0 / 1080.0);
    assert_near(post_affine.offset_norm[1], 420.0 / 1920.0);
}

#[test]
fn geometry_accepts_project_verified_non_square_pixel_and_proxy_dimensions() {
    let source = dimensions(1920, 1080);
    let project_output = dimensions(2554, 1080);
    let proxy_tile = dimensions(1277, 540);
    let value = geometry(
        source,
        project_output,
        proxy_tile,
        project_output,
        GF_ROTATION_NONE,
        GF_ROTATION_NONE,
    );

    let mapped = map_frame_geometry(
        &value,
        proxy_tile,
        project(source, project_output, GF_ROTATION_NONE),
    )
    .expect("project output geometry is authoritative for non-square pixels");

    assert_eq!(mapped.stabilization_input_dimensions, Some(source));
    assert_eq!(mapped.stabilization_output_dimensions, Some(project_output));
    assert_eq!(mapped.input.dimensions, proxy_tile);
    assert_eq!(mapped.output.dimensions, proxy_tile);
    assert!(mapped.output.post_affine.is_none());
}

#[test]
fn geometry_maps_narrow_pasp_full_and_proxy_tiles_in_logical_rect_space() {
    let cases = [
        ("8:9 full", dimensions(720, 480), dimensions(720, 480)),
        ("8:9 proxy", dimensions(720, 480), dimensions(360, 240)),
        ("10:11 full", dimensions(704, 480), dimensions(704, 480)),
        ("10:11 proxy", dimensions(704, 480), dimensions(352, 240)),
    ];
    let logical_output = dimensions(640, 480);

    for (label, physical_source, physical_tile) in cases {
        let mut value = geometry(
            physical_source,
            logical_output,
            physical_tile,
            logical_output,
            GF_ROTATION_NONE,
            GF_ROTATION_NONE,
        );
        value.source_rect = rect(0, 0, 640, 480);
        value.destination_rect = rect(0, 0, 640, 480);

        let mapped = map_frame_geometry(
            &value,
            physical_tile,
            project(physical_source, logical_output, GF_ROTATION_NONE),
        )
        .unwrap_or_else(|error| panic!("{label}: {}", error.message));

        assert_eq!(mapped.input.dimensions, physical_tile, "{label}");
        assert_eq!(mapped.output.dimensions, physical_tile, "{label}");
        assert_eq!(
            mapped.input.rect,
            Some((
                0,
                0,
                physical_tile.width as usize,
                physical_tile.height as usize
            )),
            "{label}"
        );
        assert_eq!(mapped.output.rect, mapped.input.rect, "{label}");
        assert!(mapped.output.post_affine.is_none(), "{label}");
    }
}

#[test]
fn geometry_keeps_logical_ideal_rects_separate_from_physical_proxy_texture_size() {
    let project_size = dimensions(3840, 2160);
    let proxy_tile = dimensions(960, 540);
    let canvas = dimensions(1920, 1080);
    let mut value = geometry(
        project_size,
        project_size,
        proxy_tile,
        canvas,
        GF_ROTATION_NONE,
        GF_ROTATION_NONE,
    );
    value.source_rect = rect(0, 0, 3840, 2160);
    value.destination_rect = rect(0, 0, 1920, 1080);
    value.forward_transform = affine([0.5, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 1.0]);
    value.inverse_transform = affine([2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0]);

    let mapped = map_frame_geometry(
        &value,
        proxy_tile,
        project(project_size, project_size, GF_ROTATION_NONE),
    )
    .expect("logical document rects must not be forced to proxy texture extents");

    assert_eq!(mapped.input.dimensions, proxy_tile);
    assert_eq!(mapped.output.dimensions, proxy_tile);
    let post_affine = mapped.output.post_affine.expect("canvas scale");
    assert_near(post_affine.zoom, 0.5);
    assert_near(post_affine.offset_norm[0], 0.0);
    assert_near(post_affine.offset_norm[1], 0.0);
}

#[test]
fn geometry_rebases_nonzero_top_left_rects_and_decomposes_post_affine() {
    let tile = dimensions(400, 600);
    let mut value = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
    value.source_rect = rect(100, 200, 500, 800);
    value.destination_rect = rect(1000, -200, 1400, 400);
    value.source_origin = GF_IMAGE_ORIGIN_TOP_LEFT;
    value.destination_origin = GF_IMAGE_ORIGIN_TOP_LEFT;
    value.forward_transform = affine([0.0, 2.0, 440.0, -2.0, 0.0, 620.0, 0.0, 0.0, 1.0]);
    value.inverse_transform = affine([0.0, -0.5, 310.0, 0.5, 0.0, -220.0, 0.0, 0.0, 1.0]);

    let mapped = map_frame_geometry(&value, tile, project(tile, tile, GF_ROTATION_NONE))
        .expect("verified similarity transform");

    assert_eq!(mapped.input.rect, Some((0, 0, 400, 600)));
    assert_eq!(mapped.output.rect, Some((0, 0, 400, 600)));
    let post_affine = mapped.output.post_affine.expect("non-identity affine");
    assert_near(post_affine.zoom, 2.0);
    assert_eq!(post_affine.scale_xy, [1.0, 1.0]);
    assert_near(post_affine.rotation_deg, 90.0);
    assert_near(post_affine.offset_norm[0], 0.1);
    assert_near(post_affine.offset_norm[1], -0.2);
}

#[test]
fn geometry_bottom_left_rect_origins_are_canonicalized_without_spurious_translation() {
    let tile = dimensions(320, 180);
    let mut value = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
    value.source_rect = rect(-50, 75, 270, 255);
    value.destination_rect = rect(950, -425, 1270, -245);
    value.source_origin = GF_IMAGE_ORIGIN_BOTTOM_LEFT;
    value.destination_origin = GF_IMAGE_ORIGIN_BOTTOM_LEFT;
    value.forward_transform = affine([1.0, 0.0, 1000.0, 0.0, 1.0, -500.0, 0.0, 0.0, 1.0]);
    value.inverse_transform = affine([1.0, 0.0, -1000.0, 0.0, 1.0, 500.0, 0.0, 0.0, 1.0]);

    let mapped = map_frame_geometry(&value, tile, project(tile, tile, GF_ROTATION_NONE))
        .expect("absolute rect origins must rebase to the tile");

    assert_eq!(mapped.input.rect, Some((0, 0, 320, 180)));
    assert_eq!(mapped.output.rect, Some((0, 0, 320, 180)));
    assert!(mapped.output.post_affine.is_none());
}

#[test]
fn geometry_adapter_affine_reaches_core_nonuniform_inverse_without_loss() {
    let tile = dimensions(400, 240);
    let project_geometry = project(tile, tile, GF_ROTATION_NONE);
    let mut value = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
    value.source_origin = GF_IMAGE_ORIGIN_BOTTOM_LEFT;
    value.destination_origin = GF_IMAGE_ORIGIN_BOTTOM_LEFT;
    value.forward_transform = affine([0.0, -1.5, 380.0, 2.0, 0.0, -280.0, 0.0, 0.0, 1.0]);
    value.inverse_transform =
        affine([0.0, 0.5, 140.0, -2.0 / 3.0, 0.0, 760.0 / 3.0, 0.0, 0.0, 1.0]);

    let mapped = map_frame_geometry(&value, tile, project_geometry)
        .expect("orthogonal non-uniform ScaleTranslate geometry");
    let post_affine = mapped.output.post_affine.expect("non-uniform affine");
    assert_near(post_affine.rotation_deg, 90.0);
    assert_near(post_affine.zoom, 1.0);
    assert_near(post_affine.scale_xy[0], 2.0);
    assert_near(post_affine.scale_xy[1], 1.5);
    assert_near(post_affine.offset_norm[0], 0.0);
    assert_near(post_affine.offset_norm[1], 0.0);

    let source_point = post_affine.inverse_map_point([170.0, 140.0], [400.0, 240.0]);
    assert_near(source_point[0], 210.0);
    assert_near(source_point[1], 140.0);
}

#[test]
fn geometry_rejects_shear_reflection_and_project_conflicts() {
    let tile = dimensions(400, 240);
    let project_geometry = project(tile, tile, GF_ROTATION_NONE);

    let mut shear = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
    shear.forward_transform = affine([1.0, 0.25, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    shear.inverse_transform = affine([1.0, -0.25, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    let error = map_frame_geometry(&shear, tile, project_geometry).unwrap_err();
    assert_eq!(error.status, GFStatus::InvalidArgument);
    assert!(error.message.contains("shear"), "{}", error.message);

    let mut reflection = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
    reflection.forward_transform = affine([-1.0, 0.0, 400.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    reflection.inverse_transform = reflection.forward_transform;
    let error = map_frame_geometry(&reflection, tile, project_geometry).unwrap_err();
    assert_eq!(error.status, GFStatus::InvalidArgument);
    assert!(error.message.contains("reflection"), "{}", error.message);

    let valid = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
    let error = map_frame_geometry(
        &valid,
        tile,
        project(dimensions(401, 240), tile, GF_ROTATION_NONE),
    )
    .unwrap_err();
    assert_eq!(error.status, GFStatus::InvalidArgument);
    assert!(error.message.contains("project input"), "{}", error.message);

    let mut direction_conflict = valid;
    direction_conflict.input_rotation = GF_ROTATION_CLOCKWISE_90;
    let error = map_frame_geometry(&direction_conflict, tile, project_geometry).unwrap_err();
    assert_eq!(error.status, GFStatus::InvalidArgument);
    assert!(
        error.message.contains("input rotation"),
        "{}",
        error.message
    );
}

#[test]
fn geometry_direct_and_route_d_use_the_identical_pure_mapper() {
    let source = dimensions(720, 1280);
    let tile = dimensions(360, 640);
    let value = geometry(
        source,
        source,
        tile,
        dimensions(1080, 1920),
        GF_ROTATION_NONE,
        GF_ROTATION_NONE,
    );
    let project_geometry = project(source, source, GF_ROTATION_NONE);

    let direct = map_frame_geometry(&value, tile, project_geometry).expect("Direct geometry");
    let route_d = map_frame_geometry(&value, tile, project_geometry).expect("Route D geometry");

    assert_eq!(direct, route_d);
}

#[test]
fn geometry_canvas_and_delivery_matrix_keeps_project_and_document_scales_separate() {
    let project_output = dimensions(1920, 1080);
    let square_tile = dimensions(540, 540);
    let canvas_cases = [
        ("16:9", dimensions(1920, 1080), None),
        ("9:16", dimensions(1080, 1920), Some([0.21875, -0.3888889])),
        ("1:1", dimensions(1080, 1080), Some([0.21875, 0.0])),
    ];
    for (label, canvas, expected_offset) in canvas_cases {
        let mapped = map_frame_geometry(
            &geometry(
                project_output,
                project_output,
                square_tile,
                canvas,
                GF_ROTATION_NONE,
                GF_ROTATION_NONE,
            ),
            square_tile,
            project(project_output, project_output, GF_ROTATION_NONE),
        )
        .unwrap_or_else(|error| panic!("canvas {label}: {}", error.message));

        assert_eq!(mapped.stabilization_input_dimensions, Some(project_output));
        assert_eq!(mapped.stabilization_output_dimensions, Some(project_output));
        match expected_offset {
            None => assert!(mapped.output.post_affine.is_none()),
            Some(expected) => {
                let affine = mapped.output.post_affine.expect("canvas translation");
                assert_near(affine.zoom, 1.0);
                assert_near(affine.rotation_deg, 0.0);
                assert_near(affine.offset_norm[0], expected[0]);
                assert_near(affine.offset_norm[1], expected[1]);
            }
        }
    }

    let source = dimensions(3840, 2160);
    let canvas = dimensions(1920, 1080);
    let delivery_cases = [
        ("full", dimensions(1920, 1080)),
        ("proxy", dimensions(960, 540)),
        ("better-performance", dimensions(480, 270)),
    ];
    for (label, physical_tile) in delivery_cases {
        let mut value = geometry(
            source,
            source,
            physical_tile,
            canvas,
            GF_ROTATION_NONE,
            GF_ROTATION_NONE,
        );
        value.source_rect = rect(100, 200, 3940, 2360);
        value.destination_rect = rect(700, -100, 2620, 980);
        value.forward_transform = affine([0.5, 0.0, 650.0, 0.0, 0.5, -200.0, 0.0, 0.0, 1.0]);
        value.inverse_transform = affine([2.0, 0.0, -1300.0, 0.0, 2.0, 400.0, 0.0, 0.0, 1.0]);

        let mapped = map_frame_geometry(
            &value,
            physical_tile,
            project(source, source, GF_ROTATION_NONE),
        )
        .unwrap_or_else(|error| panic!("delivery {label}: {}", error.message));
        let affine = mapped.output.post_affine.expect("document scale");
        assert_eq!(mapped.input.dimensions, physical_tile);
        assert_eq!(mapped.output.dimensions, physical_tile);
        assert_near(affine.zoom, 0.5);
        assert_near(affine.offset_norm[0], 0.0);
        assert_near(affine.offset_norm[1], 0.0);
    }
}

#[test]
fn geometry_current_frame_affines_do_not_leak_across_frames_or_instances() {
    let tile = dimensions(1000, 500);
    let project_geometry = project(tile, tile, GF_ROTATION_NONE);
    let frame = |forward, inverse| {
        let mut value = geometry(tile, tile, tile, tile, GF_ROTATION_NONE, GF_ROTATION_NONE);
        value.source_origin = GF_IMAGE_ORIGIN_BOTTOM_LEFT;
        value.destination_origin = GF_IMAGE_ORIGIN_BOTTOM_LEFT;
        value.forward_transform = affine(forward);
        value.inverse_transform = affine(inverse);
        map_frame_geometry(&value, tile, project_geometry).expect("supported live affine")
    };

    let instance_a_start = frame(
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    );
    let instance_b_position = frame(
        [1.0, 0.0, 100.0, 0.0, 1.0, -50.0, 0.0, 0.0, 1.0],
        [1.0, 0.0, -100.0, 0.0, 1.0, 50.0, 0.0, 0.0, 1.0],
    );
    let instance_a_scale = frame(
        [1.25, 0.0, -125.0, 0.0, 1.25, -62.5, 0.0, 0.0, 1.0],
        [0.8, 0.0, 100.0, 0.0, 0.8, 50.0, 0.0, 0.0, 1.0],
    );
    let instance_b_rotation = frame(
        [0.0, -1.0, 750.0, 1.0, 0.0, -250.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 250.0, -1.0, 0.0, 750.0, 0.0, 0.0, 1.0],
    );
    let instance_a_rebuilt = frame(
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    );

    assert!(instance_a_start.output.post_affine.is_none());
    let position = instance_b_position.output.post_affine.unwrap();
    assert_near(position.zoom, 1.0);
    assert_near(position.offset_norm[0], 0.1);
    assert_near(position.offset_norm[1], -0.1);
    let scale = instance_a_scale.output.post_affine.unwrap();
    assert_near(scale.zoom, 1.25);
    assert_near(scale.offset_norm[0], 0.0);
    assert_near(scale.offset_norm[1], 0.0);
    let rotation = instance_b_rotation.output.post_affine.unwrap();
    assert_near(rotation.rotation_deg, 90.0);
    assert_near(rotation.offset_norm[0], 0.0);
    assert_near(rotation.offset_norm[1], 0.0);
    assert_eq!(instance_a_rebuilt, instance_a_start);
}
