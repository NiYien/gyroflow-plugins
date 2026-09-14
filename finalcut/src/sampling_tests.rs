// SPDX-License-Identifier: GPL-3.0-or-later
use super::host_geometry::*;
use super::{GFAffineTransform, GFDimensionsU32, GFProjectGeometry};
use gyroflow_plugin_base::gyroflow_core::{
    gpu::{BufferDescription, BufferSource, Buffers, SamplingTransform, wgpu::WgpuWrapper},
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
    let mut result = vec![0x7f; output.0 * output.1 * 16];
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
#[cfg(target_os = "macos")]
#[test]
#[ignore = "Requires host-owned Metal textures; run explicitly"]
fn metal_texture_pixel_centers_match_cpu() {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::*;
    use std::{ffi::c_void, ptr::NonNull};
    let device = MTLCreateSystemDefaultDevice().expect("Metal device");
    let queue = device.newCommandQueue().unwrap();
    let descriptor = MTLTextureDescriptor::new();
    unsafe {
        descriptor.setWidth(64);
        descriptor.setHeight(48);
    }
    descriptor.setPixelFormat(MTLPixelFormat::RGBA32Float);
    descriptor.setStorageMode(MTLStorageMode::Shared);
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    let source = device.newTextureWithDescriptor(&descriptor).unwrap();
    unsafe {
        descriptor.setWidth(96);
        descriptor.setHeight(96);
    }
    let destination = device.newTextureWithDescriptor(&descriptor).unwrap();
    let mut data = rgba_bytes(
        (0..64 * 48).map(|n| [(n % 64) as f32 / 64.0, (n / 64) as f32 / 48.0, 0.25, 1.0]),
    );
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
            NonNull::new(data.as_mut_ptr().cast()).unwrap(),
            64 * 16,
        );
    }
    let ptr = |v: &Retained<ProtocolObject<dyn MTLTexture>>| Retained::as_ptr(v) as *mut c_void;
    let mut buffers = Buffers {
        input: BufferDescription {
            size: (64, 48, 64 * 16),
            data: BufferSource::Metal {
                texture: ptr(&source),
                command_queue: Retained::as_ptr(&queue) as *mut c_void,
            },
            ..Default::default()
        },
        output: BufferDescription {
            size: (96, 96, 96 * 16),
            data: BufferSource::Metal {
                texture: ptr(&destination),
                command_queue: Retained::as_ptr(&queue) as *mut c_void,
            },
            ..Default::default()
        },
    };
    let mut p = params((64, 48), (96, 96));
    p.sampling_output = SamplingTransform {
        rows: [[2.0 / 3.0, 0.0, 0.0, 0.0], [0.0, 2.0 / 3.0, -8.0, 0.0]],
    }
    .rows;
    let model = DistortionModel::from_name("poly3");
    let backend =
        WgpuWrapper::new(&p, RGBAf::wgpu_format().unwrap(), model, None, &buffers, 0).unwrap();
    let matrices = vec![[
        1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ]];
    assert!(backend.undistort_image(
        &mut buffers,
        &FrameTransform {
            matrices,
            kernel_params: p,
            ..Default::default()
        },
        &[]
    ));
    let command = queue.commandBuffer().unwrap();
    command.commit();
    command.waitUntilCompleted();
    let mut pixels = vec![0f32; 96 * 96 * 4];
    unsafe {
        destination.getBytes_bytesPerRow_fromRegion_mipmapLevel(
            NonNull::new(pixels.as_mut_ptr().cast()).unwrap(),
            96 * 16,
            region(96, 96),
            0,
        );
    }
    let expected = render(&mut data, (64, 48), (96, 96), p, false);
    for (i, (a, b)) in pixels.iter().zip(&expected).enumerate() {
        assert!(
            (a - b).abs() < 0.0001,
            "Metal texture mismatch at {i}: {a} != {b}"
        );
    }
}
fn params(input: (usize, usize), output: (usize, usize)) -> KernelParams {
    KernelParams {
        width: 64,
        height: 48,
        output_width: 64,
        output_height: 48,
        stride: input.0 as i32 * 16,
        output_stride: output.0 as i32 * 16,
        matrix_count: 1,
        interpolation: 2,
        flags: 16384,
        bytes_per_pixel: 16,
        pix_element_count: 4,
        f: [1.0, 1.0],
        lens_correction_amount: 1.0,
        source_rect: [0, 0, input.0 as i32, input.1 as i32],
        output_rect: [0, 0, output.0 as i32, output.1 as i32],
        max_pixel_value: 1.0,
        pixel_value_limit: 1.0,
        light_refraction_coefficient: 1.0,
        safe_area_rect: [0.0, 0.0, 64.0, 48.0],
        ..Default::default()
    }
}
fn check_matrix(gpu: bool) {
    for rotation in [0, 90, 180, 270] {
        for native in [(32, 24), (64, 48), (128, 96)] {
            let size = if rotation % 180 == 0 {
                native
            } else {
                (native.1, native.0)
            };
            // The physical host image contains the rotated native gradient.
            let mut input = rgba_bytes((0..size.0 * size.1).map(|n| {
                let x = n % size.0;
                let y = n / size.0;
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
                    let project = GFProjectGeometry {
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
                    };
                    let mapping = build_mapping(
                        image(size.0 as u32, size.1 as u32),
                        image(output.0 as u32, output.1 as u32),
                        project,
                        GFHostOptions {
                            input_orientation: 0,
                            sizing,
                        },
                    )
                    .unwrap();
                    let mut p = params(size, output);
                    p.sampling_input = mapping.input.rows;
                    p.sampling_output = mapping.output.rows;
                    let pixels = render(&mut input, size, output, p, gpu);
                    let sx = output.0 as f32 / 64.0;
                    let sy = output.1 as f32 / 48.0;
                    let (sx, sy) = match sizing {
                        1 => (sx.min(sy), sx.min(sy)),
                        2 => (sx.max(sy), sx.max(sy)),
                        _ => (sx, sy),
                    };
                    let ox = (output.0 as f32 - 64.0 * sx) * 0.5;
                    let oy = (output.1 as f32 - 48.0 * sy) * 0.5;
                    for y in 0..output.1 {
                        for x in 0..output.0 {
                            let nx = (x as f32 + 0.5 - ox) / sx - 0.5;
                            let ny = (y as f32 + 0.5 - oy) / sy - 0.5;
                            let pixel = &pixels[(y * output.0 + x) * 4..][..4];
                            if nx < -0.5001 || ny < -0.5001 || nx >= 63.5001 || ny >= 47.5001 {
                                assert_eq!(
                                    pixel, [0.0; 4],
                                    "unwritten letterbox {rotation} {output:?} {sizing} {x},{y}"
                                );
                            } else if nx >= 1.0 && ny >= 1.0 && nx < 62.0 && ny < 46.0 {
                                let expected = [nx / 64.0, ny / 48.0, 0.25, 1.0];
                                for c in 0..4 {
                                    assert!(
                                        (pixel[c] - expected[c]).abs() < 0.001,
                                        "pixel mismatch gpu={gpu} rotation={rotation} output={output:?} sizing={sizing} xy={x},{y} got={pixel:?} expected={expected:?}"
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
fn host_mapping_renders_scaled_rotated_aspect_pixels_cpu() {
    check_matrix(false);
}
#[test]
#[ignore = "Requires a real GPU; run explicitly in the candidate verification"]
fn host_mapping_renders_scaled_rotated_aspect_pixels_gpu() {
    check_matrix(true);
}
#[test]
fn absent_sampling_preserves_legacy_pixels_cpu() {
    let mut input = rgba_bytes(
        (0..64 * 48).map(|n| [(n % 64) as f32 / 64.0, (n / 64) as f32 / 48.0, 0.25, 1.0]),
    );
    let p = params((64, 48), (64, 48));
    let mapped = render(&mut input, (64, 48), (64, 48), p, false);
    let mut legacy = p;
    legacy.flags = 0;
    legacy.sampling_input = SamplingTransform {
        rows: [[9.0; 4]; 2],
    }
    .rows;
    let original = render(&mut input, (64, 48), (64, 48), legacy, false);
    assert_eq!(mapped, original);
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

#[test]
#[ignore = "Requires a real GPU"]
fn ewa_sampling_includes_both_affines_and_feather_derivatives() {
    let mut input = rgba_bytes(
        (0..64 * 48).map(|n| [(n % 64) as f32 / 64.0, (n / 64) as f32 / 48.0, 0.25, 1.0]),
    );
    for mode in 0..=3 {
        let mut p = params((64, 48), (32, 48));
        p.interpolation = 13;
        p.background_mode = mode;
        p.background_margin = 0.25;
        p.background_margin_feather = 0.15;
        p.ewa_coeffs_p = [1.0, 0.0, -2.5, 1.5];
        p.ewa_coeffs_q = [2.0, -4.0, 2.5, -0.5];
        p.sampling_output = [[2.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        p.sampling_input = [[0.8, 0.0, 6.4, 0.0], [0.0, 0.75, 6.0, 0.0]];
        let cpu = render(&mut input, (64, 48), (32, 48), p, false);
        let gpu = render(&mut input, (64, 48), (32, 48), p, true);
        for (i, (a, b)) in cpu.iter().zip(&gpu).enumerate() {
            assert!(
                (a - b).abs() < 0.003,
                "EWA mode={mode} pixel={i} {a} != {b}"
            );
        }
    }
}
