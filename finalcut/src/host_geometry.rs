// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{GFAffineTransform, GFDimensionsU32, GFProjectGeometry, GFTime, GFTimeRange};
use gyroflow_plugin_base::{PostAffine, StabilizationManager};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GFHostImageV2 {
    pub image_rect: [f64; 4],
    pub tile_rect: [f64; 4],
    pub pixel_to_ideal: GFAffineTransform,
    pub texture: GFDimensionsU32,
    /// FxPlug: bottom-left = 0, top-left = 2.
    pub origin: u32,
    pub reserved: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GFHostOptions {
    /// Auto, unrotated, clockwise 90, 180, clockwise 270.
    pub input_orientation: u32,
    /// Auto (fit), fit, fill, stretch.
    pub sizing: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GFMetalRenderRequestV2 {
    pub version: u32,
    pub struct_size: u32,
    pub input_texture: *mut std::ffi::c_void,
    pub output_texture: *mut std::ffi::c_void,
    pub command_queue: *mut std::ffi::c_void,
    pub device_registry_id: u64,
    pub source: GFHostImageV2,
    pub destination: GFHostImageV2,
    pub options: GFHostOptions,
    pub pixel_format: u32,
    pub source_time_valid: u32,
    pub source_time: GFTime,
    pub render_time: GFTime,
    pub effect_bounds: GFTimeRange,
    pub input_bounds: GFTimeRange,
}
const _: () = {
    assert!(std::mem::size_of::<GFHostImageV2>() == 152);
    assert!(std::mem::size_of::<GFMetalRenderRequestV2>() == 456);
    assert!(std::mem::offset_of!(GFMetalRenderRequestV2, source) == 40);
    assert!(std::mem::offset_of!(GFMetalRenderRequestV2, source_time) == 360);
};

pub type PixelRect = (usize, usize, usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceCrop {
    pub size: (usize, usize),
    pub offset: (usize, usize),
    pub output: (usize, usize),
}

#[derive(Debug)]
pub struct HostMapping {
    pub input_rect: PixelRect,
    pub output_rect: PixelRect,
    pub input_rotation: f32,
    pub output_affine: Option<PostAffine>,
    pub output_flip_v: bool,
    pub framebuffer_inverted: bool,
    pub crop: Option<SourceCrop>,
}

#[derive(Clone, Copy)]
struct ImageSpace {
    local_bounds: [f64; 4],
    ideal_size: [f64; 2],
    texture: [f64; 2],
}

fn positive_rect(r: [f64; 4]) -> Result<[f64; 4], String> {
    if r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1] {
        Ok(r)
    } else {
        Err("Host image bounds must be finite and non-empty".into())
    }
}
fn dimensions(d: GFDimensionsU32) -> Result<[f64; 2], String> {
    if d.width == 0 || d.height == 0 || d.width > i32::MAX as u32 / 16 || d.height > i32::MAX as u32
    {
        return Err("Invalid image dimensions".into());
    }
    Ok([d.width as f64, d.height as f64])
}
fn image_space(image: GFHostImageV2) -> Result<ImageSpace, String> {
    let texture = dimensions(image.texture)?;
    let tile = positive_rect(image.tile_rect)?;
    let r = positive_rect(image.image_rect)?;
    if image.reserved != 0
        || !matches!(image.origin, 0 | 2)
        || ((tile[2] - tile[0]) - texture[0]).abs() > 1e-4
        || ((tile[3] - tile[1]) - texture[1]).abs() > 1e-4
    {
        return Err("Host tile bounds disagree with its texture or origin".into());
    }
    let m = image.pixel_to_ideal.values;
    // The effect advertises FxPlug ScaleTranslate support. Rotation metadata is
    // handled by the existing input-rotation parameter, independently of this matrix.
    if !m.iter().all(|v| v.is_finite())
        || m[0] <= 0.0
        || m[4] <= 0.0
        || [m[1], m[3], m[6], m[7], m[8] - 1.0]
            .iter()
            .any(|v| v.abs() > 1e-9)
    {
        return Err("Host pixel transform is not a valid scale and translation".into());
    }
    let local_bounds = if image.origin == 0 {
        [
            r[0] - tile[0],
            tile[3] - r[3],
            r[2] - tile[0],
            tile[3] - r[1],
        ]
    } else {
        [
            r[0] - tile[0],
            r[1] - tile[1],
            r[2] - tile[0],
            r[3] - tile[1],
        ]
    };
    if local_bounds[0] < -1e-4
        || local_bounds[1] < -1e-4
        || local_bounds[2] > texture[0] + 1e-4
        || local_bounds[3] > texture[1] + 1e-4
    {
        return Err("Stabilization requires the complete source/destination image tile".into());
    }
    Ok(ImageSpace {
        local_bounds,
        ideal_size: [(r[2] - r[0]) * m[0], (r[3] - r[1]) * m[4]],
        texture,
    })
}
fn pixel_rect(r: [f64; 4], texture: [f64; 2]) -> Result<PixelRect, String> {
    let x = r[0].round().clamp(0.0, texture[0]) as usize;
    let y = r[1].round().clamp(0.0, texture[1]) as usize;
    let right = r[2].round().clamp(0.0, texture[0]) as usize;
    let bottom = r[3].round().clamp(0.0, texture[1]) as usize;
    if right <= x || bottom <= y {
        return Err("Host content region has no pixels".into());
    }
    Ok((x, y, right - x, bottom - y))
}
fn fit_rect(space: ImageSpace, size: [f64; 2]) -> Result<PixelRect, String> {
    let scale = (space.ideal_size[0] / size[0]).min(space.ideal_size[1] / size[1]);
    let ratio = [
        size[0] * scale / space.ideal_size[0],
        size[1] * scale / space.ideal_size[1],
    ];
    let r = space.local_bounds;
    let margin = [
        (r[2] - r[0]) * (1.0 - ratio[0]) * 0.5,
        (r[3] - r[1]) * (1.0 - ratio[1]) * 0.5,
    ];
    pixel_rect(
        [
            r[0] + margin[0],
            r[1] + margin[1],
            r[2] - margin[0],
            r[3] - margin[1],
        ],
        space.texture,
    )
}
fn oriented(size: [f64; 2], rotation: i32) -> [f64; 2] {
    if rotation.rem_euclid(180) == 90 {
        [size[1], size[0]]
    } else {
        size
    }
}
fn centered_extent(full: usize, requested: f64) -> usize {
    let crop = (requested.round() as usize).clamp(1, full);
    if (full - crop) % 2 == 1 {
        crop.saturating_sub(1).max(1)
    } else {
        crop
    }
}
fn source_crop(
    native: [f64; 2],
    orientation: i32,
    project_rotation: i32,
    aspect: f64,
) -> Option<SourceCrop> {
    let display = oriented(native, orientation);
    let (w, h) = (display[0] as usize, display[1] as usize);
    let crop = if display[0] / display[1] > aspect {
        (centered_extent(w, display[1] * aspect), h)
    } else {
        (w, centered_extent(h, display[0] / aspect))
    };
    if crop == (w, h) {
        return None;
    }
    let offset = ((w - crop.0) / 2, (h - crop.1) / 2);
    let (size, offset) = if orientation.rem_euclid(180) == 90 {
        ((crop.1, crop.0), (offset.1, offset.0))
    } else {
        (crop, offset)
    };
    let output = if project_rotation.rem_euclid(180) == 90 {
        (size.1, size.0)
    } else {
        size
    };
    Some(SourceCrop {
        size,
        offset,
        output,
    })
}

pub fn build_mapping(
    source: GFHostImageV2,
    destination: GFHostImageV2,
    project: GFProjectGeometry,
    options: GFHostOptions,
) -> Result<HostMapping, String> {
    if options.input_orientation > 4 || options.sizing > 3 || project.reserved != 0 {
        return Err("Invalid host geometry options".into());
    }
    let native = dimensions(project.input_dimensions)?;
    let mut output = dimensions(project.output_dimensions)?;
    let input_space = image_space(source)?;
    let output_space = image_space(destination)?;
    let orientation = match options.input_orientation {
        0 => project.video_rotation,
        1 => 0,
        2 => 90,
        3 => 180,
        _ => 270,
    }
    .rem_euclid(360);
    if orientation % 90 != 0 || project.video_rotation.rem_euclid(90) != 0 {
        return Err("Project rotation must be a quarter turn".into());
    }
    let crop = if options.sizing == 2 {
        source_crop(
            native,
            orientation,
            project.video_rotation,
            input_space.ideal_size[0] / input_space.ideal_size[1],
        )
    } else {
        None
    };
    if let Some(c) = crop {
        output = [c.output.0 as f64, c.output.1 as f64];
    }
    let input_rect = if options.sizing <= 1 {
        fit_rect(input_space, oriented(native, orientation))?
    } else {
        pixel_rect(input_space.local_bounds, input_space.texture)?
    };
    let output_rect = if options.sizing <= 1 {
        fit_rect(output_space, output)?
    } else {
        pixel_rect(output_space.local_bounds, output_space.texture)?
    };
    let output_affine = if options.sizing == 2 {
        let scale =
            (output_space.ideal_size[0] / output[0]).max(output_space.ideal_size[1] / output[1]);
        Some(PostAffine {
            scale_xy: [
                (scale * output[0] / output_space.ideal_size[0]) as f32,
                (scale * output[1] / output_space.ideal_size[1]) as f32,
            ],
            ..Default::default()
        })
    } else {
        None
    };
    Ok(HostMapping {
        input_rect,
        output_rect,
        input_rotation: if source.origin == 0 {
            -(orientation as f32)
        } else {
            orientation as f32
        },
        output_affine,
        output_flip_v: source.origin != destination.origin,
        framebuffer_inverted: source.origin == 0,
        crop,
    })
}

/// FCP owns this per-instance snapshot. It follows the existing Resolve crop
/// adaptation: rebase the principal point in calibration coordinates and restore
/// the original geometry when the host fitting mode changes.
pub struct HostGeometryState {
    pub project: GFProjectGeometry,
    camera_matrix: Vec<[f64; 3]>,
    calibration: (usize, usize),
    applied: Option<SourceCrop>,
    initialized: bool,
}
impl HostGeometryState {
    pub fn new(manager: &StabilizationManager, project: GFProjectGeometry) -> Self {
        let lens = manager.lens.read();
        Self {
            project,
            camera_matrix: lens.fisheye_params.camera_matrix.clone(),
            calibration: (lens.calib_dimension.w, lens.calib_dimension.h),
            applied: None,
            initialized: false,
        }
    }
    pub fn apply(&mut self, manager: &StabilizationManager, crop: Option<SourceCrop>) {
        if self.initialized && self.applied == crop {
            return;
        }
        let native = (
            self.project.input_dimensions.width as usize,
            self.project.input_dimensions.height as usize,
        );
        let output = (
            self.project.output_dimensions.width as usize,
            self.project.output_dimensions.height as usize,
        );
        let (size, output, offset) =
            crop.map(|c| (c.size, c.output, c.offset))
                .unwrap_or((native, output, (0, 0)));
        let mut camera = self.camera_matrix.clone();
        let sx = self.calibration.0.max(1) as f64 / native.0 as f64;
        let sy = self.calibration.1.max(1) as f64 / native.1 as f64;
        if camera.len() >= 2 {
            camera[0][2] -= offset.0 as f64 * sx;
            camera[1][2] -= offset.1 as f64 * sy;
        }
        {
            let mut lens = manager.lens.write();
            lens.fisheye_params.camera_matrix = camera;
            let (w, h) = if crop.is_some() {
                (
                    ((size.0 as f64 * sx).round() as usize).max(1),
                    ((size.1 as f64 * sy).round() as usize).max(1),
                )
            } else {
                self.calibration
            };
            lens.calib_dimension =
                gyroflow_plugin_base::gyroflow_core::lens_profile::Dimensions { w, h };
        }
        {
            let mut p = manager.params.write();
            p.size = size;
            p.output_size = output;
            // FxPlug presents the already-oriented image clockwise in top-down
            // coordinates; the core's top-down video rotation uses the opposite sign.
            p.video_rotation = -(self.project.video_rotation as f64);
        }
        manager.init_size();
        manager.invalidate_smoothing();
        manager.recompute_blocking();
        self.applied = crop;
        self.initialized = true;
    }
}
