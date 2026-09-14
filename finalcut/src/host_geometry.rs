// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{GFAffineTransform, GFDimensionsU32, GFProjectGeometry, GFTime, GFTimeRange};
use gyroflow_plugin_base::gyroflow_core::gpu::SamplingTransform;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GFHostImageV2 {
    pub image_rect: [f64; 4],
    pub tile_rect: [f64; 4],
    pub pixel_to_ideal: GFAffineTransform,
    pub texture: GFDimensionsU32,
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
#[derive(Clone, Copy, Debug)]
struct Affine([f64; 6]);
impl Affine {
    fn map(self, p: [f64; 2]) -> [f64; 2] {
        let a = self.0;
        [
            a[0] * p[0] + a[1] * p[1] + a[2],
            a[3] * p[0] + a[4] * p[1] + a[5],
        ]
    }
    fn then(self, next: Self) -> Self {
        let a = next.0;
        let b = self.0;
        Self([
            a[0] * b[0] + a[1] * b[3],
            a[0] * b[1] + a[1] * b[4],
            a[0] * b[2] + a[1] * b[5] + a[2],
            a[3] * b[0] + a[4] * b[3],
            a[3] * b[1] + a[4] * b[4],
            a[3] * b[2] + a[4] * b[5] + a[5],
        ])
    }
    fn inverse(self) -> Result<Self, String> {
        let a = self.0;
        let d = a[0] * a[4] - a[1] * a[3];
        if !a.iter().all(|v| v.is_finite()) || !d.is_finite() || d.abs() <= 1e-12 {
            return Err("Host pixel transform is singular or non-finite".into());
        }
        Ok(Self([
            a[4] / d,
            -a[1] / d,
            (a[1] * a[5] - a[4] * a[2]) / d,
            -a[3] / d,
            a[0] / d,
            (a[3] * a[2] - a[0] * a[5]) / d,
        ]))
    }
    fn sampling(self) -> Result<SamplingTransform, String> {
        let a = self.0;
        let value = SamplingTransform {
            rows: [
                [a[0] as f32, a[1] as f32, a[2] as f32, 0.0],
                [a[3] as f32, a[4] as f32, a[5] as f32, 0.0],
            ],
        };
        if value.is_valid() {
            Ok(value)
        } else {
            Err("Host transform exceeds sampler range".into())
        }
    }
}
fn rect(r: [f64; 4]) -> Result<[f64; 4], String> {
    if r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1] {
        Ok(r)
    } else {
        Err("Host image bounds must be finite and non-empty".into())
    }
}
fn transformed_rect(r: [f64; 4], t: Affine) -> [f64; 4] {
    let p = [[r[0], r[1]], [r[2], r[1]], [r[0], r[3]], [r[2], r[3]]].map(|p| t.map(p));
    [
        p.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
        p.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
        p.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max),
        p.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max),
    ]
}
fn dimensions(d: GFDimensionsU32) -> Result<[f64; 2], String> {
    if d.width == 0 || d.height == 0 || d.width > i32::MAX as u32 || d.height > i32::MAX as u32 {
        return Err("Invalid image dimensions".into());
    }
    Ok([d.width as f64, d.height as f64])
}
fn rotation(degrees: i32, size: [f64; 2]) -> Result<(Affine, [f64; 2]), String> {
    let [w, h] = size;
    Ok(match degrees.rem_euclid(360) {
        0 => (Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]), size),
        90 => (Affine([0.0, -1.0, h, 1.0, 0.0, 0.0]), [h, w]),
        180 => (Affine([-1.0, 0.0, w, 0.0, -1.0, h]), size),
        270 => (Affine([0.0, 1.0, 0.0, -1.0, 0.0, w]), [h, w]),
        _ => return Err("Project rotation must be a quarter turn".into()),
    })
}
fn placement(size: [f64; 2], target: [f64; 4], mode: u32) -> Affine {
    let w = target[2] - target[0];
    let h = target[3] - target[1];
    let sx = w / size[0];
    let sy = h / size[1];
    let (sx, sy) = match mode {
        2 => (sx.max(sy), sx.max(sy)),
        3 => (sx, sy),
        _ => (sx.min(sy), sx.min(sy)),
    };
    // Native pixels point downwards; ideal image coordinates point upwards.
    Affine([
        sx,
        0.0,
        target[0] + (w - size[0] * sx) * 0.5,
        0.0,
        -sy,
        target[3] - (h - size[1] * sy) * 0.5,
    ])
}
fn image_space(image: GFHostImageV2) -> Result<(Affine, Affine, [f64; 4]), String> {
    let size = dimensions(image.texture)?;
    let tile = rect(image.tile_rect)?;
    let bounds = rect(image.image_rect)?;
    if image.reserved != 0
        || !matches!(image.origin, 0 | 2)
        || ((tile[2] - tile[0]) - size[0]).abs() > 1e-4
        || ((tile[3] - tile[1]) - size[1]).abs() > 1e-4
    {
        return Err("Host tile bounds disagree with its texture or origin".into());
    }
    let m = image.pixel_to_ideal.values;
    if !m.iter().all(|v| v.is_finite())
        || m[6].abs() > 1e-12
        || m[7].abs() > 1e-12
        || (m[8] - 1.0).abs() > 1e-12
    {
        return Err("Host pixel transform must be affine".into());
    }
    let pixel_to_ideal = Affine([m[0], m[1], m[2], m[3], m[4], m[5]]);
    let local = if image.origin == 2 {
        Affine([1.0, 0.0, -tile[0], 0.0, -1.0, tile[3]])
    } else {
        Affine([1.0, 0.0, -tile[0], 0.0, 1.0, -tile[1]])
    };
    Ok((
        pixel_to_ideal.inverse()?.then(local),
        local,
        rect(transformed_rect(bounds, pixel_to_ideal))?,
    ))
}
#[derive(Debug)]
pub struct HostMapping {
    pub input: SamplingTransform,
    pub output: SamplingTransform,
    pub source_rect: (usize, usize, usize, usize),
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
    let output = dimensions(project.output_dimensions)?;
    let (source_to_texture, pixel_to_local, source_bounds) = image_space(source)?;
    let (output_to_texture, _, output_bounds) = image_space(destination)?;
    let orientation = match options.input_orientation {
        0 => project.video_rotation,
        1 => 0,
        2 => 90,
        3 => 180,
        _ => 270,
    };
    let (rotate, oriented) = rotation(orientation, native)?;
    let input = rotate
        .then(placement(oriented, source_bounds, options.sizing))
        .then(source_to_texture)
        .sampling()?;
    let output = placement(output, output_bounds, options.sizing)
        .then(output_to_texture)
        .inverse()?
        .sampling()?;
    let valid = transformed_rect(source.image_rect, pixel_to_local);
    let w = source.texture.width as f64;
    let h = source.texture.height as f64;
    let x = valid[0].floor().clamp(0.0, w) as usize;
    let y = valid[1].floor().clamp(0.0, h) as usize;
    let right = valid[2].ceil().clamp(0.0, w) as usize;
    let bottom = valid[3].ceil().clamp(0.0, h) as usize;
    if right <= x || bottom <= y {
        return Err("Host image has no pixels in the requested tile".into());
    }
    Ok(HostMapping {
        input,
        output,
        source_rect: (x, y, right - x, bottom - y),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
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
    fn project(rotation: i32) -> GFProjectGeometry {
        GFProjectGeometry {
            input_dimensions: GFDimensionsU32 {
                width: 1920,
                height: 1080,
            },
            output_dimensions: GFDimensionsU32 {
                width: 1080,
                height: 1920,
            },
            video_rotation: rotation,
            reserved: 0,
        }
    }
    #[test]
    fn rotation_scaling_and_aspect_matrix() {
        for rotation in [0, 90, 180, 270] {
            for (w, h) in [(1920, 1080), (1080, 1920), (1000, 1000), (1280, 960)] {
                for sizing in 0..=3 {
                    let m = build_mapping(
                        image(w, h),
                        image(1920, 1080),
                        project(rotation),
                        GFHostOptions {
                            input_orientation: 0,
                            sizing,
                        },
                    )
                    .unwrap();
                    assert!(m.input.is_valid() && m.output.is_valid());
                }
            }
        }
    }
    #[test]
    fn clockwise_input_maps_marked_corners_without_double_rotation() {
        let m = build_mapping(
            image(1080, 1920),
            image(1080, 1920),
            project(90),
            GFHostOptions::default(),
        )
        .unwrap();
        assert_eq!(m.input.map_point([0.0, 0.0]), [1079.0, 0.0]);
        assert_eq!(m.input.map_point([1919.0, 1079.0]), [0.0, 1919.0]);
        assert_eq!(m.output.map_point([100.0, 200.0]), [100.0, 200.0]);
        let raw = build_mapping(
            image(1920, 1080),
            image(1080, 1920),
            project(90),
            GFHostOptions {
                input_orientation: 1,
                sizing: 0,
            },
        )
        .unwrap();
        assert_eq!(raw.input.map_point([123.0, 456.0]), [123.0, 456.0]);
    }
    #[test]
    fn source_padding_is_not_treated_as_image_content() {
        let mut src = image(1920, 1080);
        src.tile_rect = [-2.0, -4.0, 1924.0, 1082.0];
        src.texture = GFDimensionsU32 {
            width: 1926,
            height: 1086,
        };
        let m = build_mapping(
            src,
            image(1080, 1920),
            project(0),
            GFHostOptions {
                input_orientation: 1,
                sizing: 0,
            },
        )
        .unwrap();
        assert_eq!(m.source_rect, (2, 2, 1920, 1080));
        assert_eq!(m.input.map_point([0.0, 0.0]), [2.0, 2.0]);
    }
}
