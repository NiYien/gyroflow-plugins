// SPDX-License-Identifier: GPL-3.0-or-later
use super::host_geometry::*;
use super::{GFAffineTransform, GFDimensionsU32, GFProjectGeometry};
use gyroflow_plugin_base::gyroflow_core::{
    gpu::{BufferDescription, BufferSource, Buffers, wgpu::WgpuWrapper},
    stabilization::{
        FrameTransform, KernelParams, PixelType, RGBAf, Stabilization,
        distortion_models::DistortionModel,
    },
};
fn image(w: u32, h: u32) -> GFHostImageV2 {
    GFHostImageV2 {
        image_rect: [0.0, 0.0, w as f64, h as f64],
        tile_rect: [0.0, 0.0, w as f64, h as f64],
        pixel_to_ideal: GFAffineTransform {
            values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        texture: GFDimensionsU32 {
            width: w,
            height: h,
        },
        origin: 2,
        reserved: 0,
    }
}
fn rgba_bytes(values: impl Iterator<Item = [f32; 4]>) -> Vec<u8> {
    values
        .flat_map(|p| p.into_iter().flat_map(f32::to_ne_bytes))
        .collect()
}
fn render(
    input: &mut [u8],
    size: (usize, usize),
    output: (usize, usize),
    p: KernelParams,
    gpu: bool,
) -> Vec<f32> {
    let mut result = vec![0; output.0 * output.1 * 16];
    let mut buffers = Buffers {
        input: BufferDescription {
            size: (size.0, size.1, size.0 * 16),
            data: BufferSource::Cpu { buffer: input },
            ..Default::default()
        },
        output: BufferDescription {
            size: (output.0, output.1, output.0 * 16),
            data: BufferSource::Cpu {
                buffer: &mut result,
            },
            ..Default::default()
        },
    };
    let model = DistortionModel::from_name("poly3");
    // Zero lens coefficients and this projection give an independently checkable identity warp.
    let matrices = vec![[
        1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]];
    if gpu {
        let backend = WgpuWrapper::new(&p, RGBAf::wgpu_format().unwrap(), model, None, &buffers, 0)
            .expect("GPU required for this explicit test");
        assert!(backend.undistort_image(
            &mut buffers,
            &FrameTransform {
                matrices,
                kernel_params: p,
                ..Default::default()
            },
            &[]
        ));
    } else {
        if p.interpolation > 8 {
            assert!(Stabilization::undistort_image_cpu::<13, RGBAf>(
                &mut buffers,
                &p,
                &model,
                None,
                &matrices,
                &[],
                &[]
            ));
        } else {
            assert!(Stabilization::undistort_image_cpu::<2, RGBAf>(
                &mut buffers,
                &p,
                &model,
                None,
                &matrices,
                &[],
                &[]
            ));
        }
    }
    result
        .chunks_exact(4)
        .map(|v| f32::from_ne_bytes(v.try_into().unwrap()))
        .collect()
}
fn params(input: (usize, usize), output: (usize, usize), mapping: &HostMapping) -> KernelParams {
    let mut p = KernelParams {
        width: 64,
        height: 48,
        output_width: 64,
        output_height: 48,
        stride: input.0 as i32 * 16,
        output_stride: output.0 as i32 * 16,
        matrix_count: 1,
        interpolation: 2,
        flags: 32 | 64,
        bytes_per_pixel: 16,
        pix_element_count: 4,
        f: [1.0, 1.0],
        lens_correction_amount: 1.0,
        source_rect: [
            mapping.input_rect.0 as i32,
            mapping.input_rect.1 as i32,
            mapping.input_rect.2 as i32,
            mapping.input_rect.3 as i32,
        ],
        output_rect: [
            mapping.output_rect.0 as i32,
            mapping.output_rect.1 as i32,
            mapping.output_rect.2 as i32,
            mapping.output_rect.3 as i32,
        ],
        input_rotation: mapping.input_rotation,
        max_pixel_value: 1.0,
        pixel_value_limit: 1.0,
        light_refraction_coefficient: 1.0,
        safe_area_rect: [0.0, 0.0, 64.0, 48.0],
        ..Default::default()
    };
    if let Some(a) = mapping.output_affine {
        p.post_rotation = a.rotation_deg;
        p.post_zoom = a.zoom;
        p.post_scale = a.scale_xy;
        p.post_offset = a.offset_norm;
    }
    p
}
fn project(rotation: i32) -> GFProjectGeometry {
    GFProjectGeometry {
        input_dimensions: GFDimensionsU32 {
            width: 64,
            height: 48,
        },
        output_dimensions: GFDimensionsU32 {
            width: 64,
            height: 48,
        },
        video_rotation: rotation,
        reserved: 0,
    }
}
fn check_images(gpu: bool) {
    for rotation in [0, 90, 180, 270] {
        for native in [(32, 24), (64, 48), (128, 96)] {
            let size = if rotation % 180 == 0 {
                native
            } else {
                (native.1, native.0)
            };
            let mut input = rgba_bytes((0..size.0 * size.1).map(|n| {
                let (x, y) = (n % size.0, n / size.0);
                let (nx, ny) = match rotation {
                    90 => (y, native.1 - 1 - x),
                    180 => (native.0 - 1 - x, native.1 - 1 - y),
                    270 => (native.0 - 1 - y, x),
                    _ => (x, y),
                };
                [
                    ((nx as f32 + 0.5) * 64.0 / native.0 as f32 - 0.5) / 64.0,
                    ((ny as f32 + 0.5) * 48.0 / native.1 as f32 - 0.5) / 48.0,
                    0.25,
                    1.0,
                ]
            }));
            for output in [(64, 48), (128, 96), (32, 24), (96, 96), (48, 64), (83, 47)] {
                for sizing in [1, 2, 3] {
                    let mapping = build_mapping(
                        image(size.0 as u32, size.1 as u32),
                        image(output.0 as u32, output.1 as u32),
                        project(rotation),
                        GFHostOptions {
                            input_orientation: 0,
                            sizing,
                        },
                    )
                    .unwrap();
                    assert!(mapping.crop.is_none());
                    let p = params(size, output, &mapping);
                    let pixels = render(&mut input, size, output, p, gpu);
                    let (rx, ry, rw, rh) = mapping.output_rect;
                    let affine = mapping.output_affine.unwrap_or_default();
                    // The existing core rotates boundary coordinates, with the established
                    // one-pixel phase on flipped axes. Preserve that Resolve convention.
                    let sx = 64.0 / native.0 as f32;
                    let sy = 48.0 / native.1 as f32;
                    let dx = if matches!(rotation, 180 | 270) {
                        -(sx + 1.0) * 0.5
                    } else {
                        (sx - 1.0) * 0.5
                    };
                    let dy = if matches!(rotation, 90 | 180) {
                        -(sy + 1.0) * 0.5
                    } else {
                        (sy - 1.0) * 0.5
                    };
                    for y in 0..output.1 {
                        for x in 0..output.0 {
                            let pixel = &pixels[(y * output.0 + x) * 4..][..4];
                            if x + 1 < rx || y + 1 < ry || x > rx + rw + 1 || y > ry + rh + 1 {
                                assert_eq!(pixel, [0.0; 4]);
                                continue;
                            }
                            let phase = if gpu { 0.5 } else { 0.0 };
                            let nx = ((x as f32 + phase - rx as f32) * 64.0 / rw as f32 - 32.0)
                                / affine.scale_xy[0]
                                + 32.0
                                + dx;
                            let ny = ((y as f32 + phase - ry as f32) * 48.0 / rh as f32 - 24.0)
                                / affine.scale_xy[1]
                                + 24.0
                                + dy;
                            if nx > 3.0 && nx < 60.0 && ny > 3.0 && ny < 44.0 {
                                for (a, b) in pixel.iter().zip([nx / 64.0, ny / 48.0, 0.25, 1.0]) {
                                    assert!(
                                        (a - b).abs() < 0.001,
                                        "gpu={gpu} rot={rotation} in={size:?} out={output:?} mode={sizing} at={x},{y}: {pixel:?} expected={nx},{ny}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn existing_core_processes_scaled_rotated_aspect_images_cpu() {
    check_images(false);
}
#[test]
#[ignore = "Requires a real GPU"]
fn existing_core_processes_scaled_rotated_aspect_images_gpu() {
    check_images(true);
}
#[test]
fn padding_and_fractional_host_scale_are_fcp_rectangles() {
    let mut source = image(70, 54);
    source.image_rect = [2.0, 4.0, 66.0, 52.0];
    source.pixel_to_ideal.values = [0.5, 0.0, 25.0, 0.0, 0.5, -10.0, 0.0, 0.0, 1.0];
    let mapping =
        build_mapping(source, image(128, 96), project(0), GFHostOptions::default()).unwrap();
    assert_eq!(mapping.input_rect, (2, 2, 64, 48));
    assert_eq!(mapping.output_rect, (0, 0, 128, 96));
}
#[test]
fn fill_crop_restores_the_original_camera_and_does_not_accumulate() {
    use gyroflow_plugin_base::StabilizationManager;
    use gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions;
    let manager = StabilizationManager::default();
    {
        let mut p = manager.params.write();
        p.size = (64, 48);
        p.output_size = (64, 48);
        p.fps = 30.0;
        p.frame_count = 30;
        p.duration_ms = 1000.0;
    }
    {
        let mut lens = manager.lens.write();
        lens.calib_dimension = Dimensions { w: 128, h: 96 };
        lens.fisheye_params.camera_matrix =
            vec![[100.0, 0.0, 64.0], [0.0, 100.0, 48.0], [0.0, 0.0, 1.0]];
    }
    let mut state = HostGeometryState::new(&manager, project(0));
    let mapping = build_mapping(
        image(64, 64),
        image(96, 96),
        project(0),
        GFHostOptions {
            input_orientation: 0,
            sizing: 2,
        },
    )
    .unwrap();
    assert_eq!(mapping.crop.unwrap().size, (48, 48));
    state.apply(&manager, mapping.crop);
    assert_eq!(manager.params.read().size, (48, 48));
    assert_eq!(manager.lens.read().calib_dimension.w, 96);
    assert_eq!(manager.lens.read().fisheye_params.camera_matrix[0][2], 48.0);
    state.apply(&manager, mapping.crop);
    assert_eq!(manager.lens.read().fisheye_params.camera_matrix[0][2], 48.0);
    state.apply(&manager, None);
    assert_eq!(manager.params.read().size, (64, 48));
    assert_eq!(manager.params.read().output_size, (64, 48));
    assert_eq!(manager.lens.read().calib_dimension.w, 128);
    assert_eq!(manager.lens.read().fisheye_params.camera_matrix[0][2], 64.0);
}
#[cfg(target_os = "macos")]
#[test]
#[ignore = "Requires Metal; exercises the production RGBA16Float bridge"]
fn versioned_bridge_processes_half_float_at_different_source_and_output_sizes() {
    use super::*;
    use gyroflow_plugin_base::RGBAf16;
    use objc2::rc::Retained;
    use objc2_metal::*;
    use std::{ffi::c_void, ptr::NonNull};
    let device = MTLCreateSystemDefaultDevice().unwrap();
    let queue = device.newCommandQueue().unwrap();
    let d = MTLTextureDescriptor::new();
    unsafe {
        d.setWidth(64);
        d.setHeight(48);
    }
    d.setPixelFormat(MTLPixelFormat::RGBA16Float);
    d.setStorageMode(MTLStorageMode::Shared);
    d.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    let source = device.newTextureWithDescriptor(&d).unwrap();
    unsafe {
        d.setWidth(96);
        d.setHeight(96);
    }
    let output = device.newTextureWithDescriptor(&d).unwrap();
    let full = rgba_bytes(
        (0..64 * 48).map(|n| [(n % 64) as f32 / 64.0, (n / 64) as f32 / 48.0, 0.25, 1.0]),
    );
    let mut half: Vec<u8> = full
        .chunks_exact(16)
        .flat_map(|p| {
            let pixel = RGBAf16::from_float_glam(RGBAf::to_float_glam(p));
            // PixelType guarantees a packed POD representation.
            unsafe { std::mem::transmute::<RGBAf16, [u8; 8]>(pixel) }
        })
        .collect();
    let region = |w, h| MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: w,
            height: h,
            depth: 1,
        },
    };
    unsafe {
        source.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            region(64, 48),
            0,
            NonNull::new(half.as_mut_ptr().cast()).unwrap(),
            64 * 8,
        );
    }
    let bytes = serde_json::to_vec(&serde_json::json!({
        "version":3,"videofile":"fixture.mov",
        "video_info":{"width":640,"height":480,"fps":60.0,"num_frames":60,"duration_ms":1000.0},
        "gyro_source":{}
    }))
    .unwrap();
    let instance = unsafe { gf_finalcut_instance_create(std::ptr::null_mut()) };
    let mut error = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            gf_finalcut_instance_load_project(instance, bytes.as_ptr(), bytes.len(), &mut error)
        },
        GFStatus::Ok
    );
    let bounds = GFTimeRange {
        start: GFTime {
            numerator: 0,
            denominator: 1,
        },
        duration: GFTime {
            numerator: 1,
            denominator: 1,
        },
    };
    let request = GFMetalRenderRequestV2 {
        version: 2,
        struct_size: std::mem::size_of::<GFMetalRenderRequestV2>() as u32,
        input_texture: Retained::as_ptr(&source) as *mut c_void,
        output_texture: Retained::as_ptr(&output) as *mut c_void,
        command_queue: Retained::as_ptr(&queue) as *mut c_void,
        device_registry_id: device.registryID(),
        source: image(64, 48),
        destination: image(96, 96),
        options: GFHostOptions::default(),
        pixel_format: 115,
        source_time_valid: 1,
        source_time: GFTime {
            numerator: 30,
            denominator: 60,
        },
        render_time: GFTime {
            numerator: 12,
            denominator: 24,
        },
        effect_bounds: bounds,
        input_bounds: bounds,
    };
    let mut outputs = Vec::new();
    for fov in [1.0, 0.65] {
        let params = GFRenderParameters {
            fov,
            smoothness: 15.0,
            lens_correction: 100.0,
            zoom_mode: 0,
            ..Default::default()
        };
        assert_eq!(
            unsafe { gf_finalcut_instance_set_render_parameters(instance, &params, &mut error) },
            GFStatus::Ok
        );
        let status =
            unsafe { gf_finalcut_instance_render_metal_v2(instance, &request, &mut error) };
        if status != GFStatus::Ok {
            let message = if error.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr((*error).message) }
                    .to_string_lossy()
                    .into_owned()
            };
            panic!("production bridge failed: {status:?}: {message}");
        }
        let command = queue.commandBuffer().unwrap();
        command.commit();
        command.waitUntilCompleted();
        let mut result = vec![0u8; 96 * 96 * 8];
        unsafe {
            output.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::new(result.as_mut_ptr().cast()).unwrap(),
                96 * 8,
                region(96, 96),
                0,
            );
        }
        let pixels: Vec<f32> = result
            .chunks_exact(8)
            .flat_map(|p| RGBAf16::to_float_glam(p).to_array())
            .collect();
        assert!(pixels.iter().all(|v| v.is_finite()));
        assert!(
            pixels.iter().any(|v| *v > 0.5),
            "bridge returned an empty frame"
        );
        outputs.push(pixels);
    }
    let difference: f32 = outputs[0]
        .iter()
        .zip(&outputs[1])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(
        difference > 10.0,
        "FOV changes must reach the stabilized sample, difference={difference}"
    );
    unsafe {
        gf_finalcut_instance_free(instance);
        gf_finalcut_error_free(error);
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Requires Metal; validates actual project rotation and image origins"]
fn production_rotation_and_origins_preserve_the_displayed_image() {
    use super::*;
    use gyroflow_plugin_base::RGBAf16;
    use gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions;
    use objc2::{rc::Retained, runtime::ProtocolObject};
    use objc2_metal::*;
    use std::{ffi::c_void, ptr::NonNull};
    let device = MTLCreateSystemDefaultDevice().unwrap();
    let queue = device.newCommandQueue().unwrap();
    let texture = |w, h| {
        let d = MTLTextureDescriptor::new();
        unsafe {
            d.setWidth(w);
            d.setHeight(h);
        }
        d.setPixelFormat(MTLPixelFormat::RGBA16Float);
        d.setStorageMode(MTLStorageMode::Shared);
        d.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
        device.newTextureWithDescriptor(&d).unwrap()
    };
    let pointer = |p: &Retained<ProtocolObject<dyn MTLTexture>>| Retained::as_ptr(p) as *mut c_void;
    let region = |w, h| MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width: w,
            height: h,
            depth: 1,
        },
    };
    let output = texture(96, 96);
    for rotation in [0, 90, 180, 270] {
        let (w, h) = if rotation % 180 == 0 {
            (64, 48)
        } else {
            (48, 64)
        };
        let source = texture(w, h);
        let mut reference = None;
        for (source_origin, destination_origin) in [(2, 2), (0, 0), (0, 2), (2, 0)] {
            let full = rgba_bytes((0..w * h).map(|n| {
                let x = n % w;
                let y = if source_origin == 0 {
                    h - 1 - n / w
                } else {
                    n / w
                };
                let (nx, ny) = match rotation {
                    90 => (y, 47 - x),
                    180 => (63 - x, 47 - y),
                    270 => (63 - y, x),
                    _ => (x, y),
                };
                [nx as f32 / 64.0, ny as f32 / 48.0, 0.25, 1.0]
            }));
            let mut half: Vec<u8> = full
                .chunks_exact(16)
                .flat_map(|p| unsafe {
                    std::mem::transmute::<RGBAf16, [u8; 8]>(RGBAf16::from_float_glam(
                        RGBAf::to_float_glam(p),
                    ))
                })
                .collect();
            unsafe {
                source.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                    region(w, h),
                    0,
                    NonNull::new(half.as_mut_ptr().cast()).unwrap(),
                    w * 8,
                );
            }
            let doc=serde_json::to_vec(&serde_json::json!({"version":3,"videofile":"fixture.mov","video_info":{"width":64,"height":48,"rotation":rotation,"fps":60.0,"num_frames":60,"duration_ms":1000.0},"output":{"output_width":w,"output_height":h},"gyro_source":{}})).unwrap();
            let mut instance = GFFinalCutInstance::default();
            let project = parse_project(&doc).unwrap();
            {
                let mut lens = project.manager.lens.write();
                lens.calib_dimension = Dimensions { w: 64, h: 48 };
                lens.asymmetrical = true;
                lens.fisheye_params.camera_matrix =
                    vec![[64.0, 0.0, 28.0], [0.0, 64.0, 18.0], [0.0, 0.0, 1.0]];
                lens.fisheye_params.distortion_coeffs = vec![0.05, 0.0, 0.0, 0.0];
            }
            instance.state.write().unwrap().project = Some(project);
            let mut error = std::ptr::null_mut();
            let params = GFRenderParameters {
                fov: 0.65,
                smoothness: 15.0,
                lens_correction: 100.0,
                ..Default::default()
            };
            assert_eq!(
                unsafe {
                    gf_finalcut_instance_set_render_parameters(&mut instance, &params, &mut error)
                },
                GFStatus::Ok
            );
            let bounds = GFTimeRange {
                start: GFTime {
                    numerator: 0,
                    denominator: 1,
                },
                duration: GFTime {
                    numerator: 1,
                    denominator: 1,
                },
            };
            let mut src = image(w as u32, h as u32);
            src.origin = source_origin;
            let mut dest = image(96, 96);
            dest.origin = destination_origin;
            let req = GFMetalRenderRequestV2 {
                version: 2,
                struct_size: std::mem::size_of::<GFMetalRenderRequestV2>() as u32,
                input_texture: pointer(&source),
                output_texture: pointer(&output),
                command_queue: Retained::as_ptr(&queue) as *mut c_void,
                device_registry_id: device.registryID(),
                source: src,
                destination: dest,
                options: GFHostOptions::default(),
                pixel_format: 115,
                source_time_valid: 1,
                source_time: GFTime {
                    numerator: 1,
                    denominator: 2,
                },
                render_time: GFTime {
                    numerator: 1,
                    denominator: 2,
                },
                effect_bounds: bounds,
                input_bounds: bounds,
            };
            assert_eq!(
                unsafe { gf_finalcut_instance_render_metal_v2(&mut instance, &req, &mut error) },
                GFStatus::Ok
            );
            let command = queue.commandBuffer().unwrap();
            command.commit();
            command.waitUntilCompleted();
            let mut bytes = vec![0u8; 96 * 96 * 8];
            unsafe {
                output.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    NonNull::new(bytes.as_mut_ptr().cast()).unwrap(),
                    96 * 8,
                    region(96, 96),
                    0,
                );
            }
            let pixels: Vec<[f32; 4]> = bytes
                .chunks_exact(8)
                .map(|p| RGBAf16::to_float_glam(p).to_array())
                .collect();
            let read = |x: usize, y: usize| {
                pixels[(if destination_origin == 0 { 95 - y } else { y }) * 96 + x]
            };
            if let Some(expected) = &reference {
                let expected: &Vec<[f32; 4]> = expected;
                for y in 32..64 {
                    for x in 32..64 {
                        for (a, b) in read(x, y).iter().zip(expected[y * 96 + x]) {
                            assert!(
                                (a - b).abs() < 0.04,
                                "rotation={rotation}, origins={source_origin}/{destination_origin} at={x},{y}: {a} != {b}"
                            );
                        }
                    }
                }
            } else {
                let horizontal = (read(60, 48), read(36, 48));
                let vertical = (read(48, 60), read(48, 36));
                match rotation {
                    0 => {
                        assert!(horizontal.0[0] > horizontal.1[0]);
                        assert!(vertical.0[1] > vertical.1[1]);
                    }
                    90 => {
                        assert!(vertical.0[0] > vertical.1[0], "90 must point down");
                        assert!(horizontal.0[1] < horizontal.1[1]);
                    }
                    180 => {
                        assert!(horizontal.0[0] < horizontal.1[0]);
                        assert!(vertical.0[1] < vertical.1[1]);
                    }
                    _ => {
                        assert!(vertical.0[0] < vertical.1[0]);
                        assert!(horizontal.0[1] > horizontal.1[1]);
                    }
                }
                reference = Some(pixels);
            }
            unsafe {
                gf_finalcut_error_free(error);
            }
        }
    }
}
