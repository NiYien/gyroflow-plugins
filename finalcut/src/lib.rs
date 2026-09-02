#![deny(unsafe_op_in_unsafe_fn)]

mod fcpxml;

pub use fcpxml::{
    BatchRouteDPatchResult, BatchTargetAction, BatchTargetReport, RouteDError, RouteDPatchResult,
    patch_fcpxml_project, patch_fcpxml_project_batch,
};

use std::ffi::CString;
use std::io::{Read, Write};
use std::os::raw::{c_char, c_void};
use std::sync::{Arc, RwLock, atomic::AtomicBool};

use base64::{Engine as _, engine::general_purpose::STANDARD};
pub use gyroflow_plugin_base::PostAffine;
use gyroflow_plugin_base::{
    BufferDescription, BufferSource, Buffers, RGBAf16, StabilizationManager,
};
use num_rational::Ratio;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FINALCUT_RUST_BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROJECT_PAYLOAD_VERSION: u32 = 1;
const PROJECT_PAYLOAD_CODEC: &str = "zlib";
const PROJECT_PAYLOAD_V2_MAGIC: &[u8; 7] = b"GFPRJ2\0";
const PROJECT_PAYLOAD_V2_HEADER_BYTES: usize = 7 + 8 + 32;
const MAX_PROJECT_BYTES: u64 = 256 * 1024 * 1024;

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GFStatus {
    Ok = 0,
    NullPointer = 1,
    InvalidArgument = 2,
    InvalidProject = 3,
    UnknownPayloadVersion = 4,
    MissingTiming = 5,
    StaleTiming = 6,
    UnsupportedPixelFormat = 7,
    RenderFailed = 8,
    Panic = 255,
}

#[repr(C)]
pub struct GFError {
    pub code: GFStatus,
    pub message: *mut c_char,
}

#[repr(C)]
#[derive(Debug)]
pub struct GFOwnedBytes {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

#[repr(C)]
#[derive(Debug, Default)]
pub struct GFRouteDPatchResult {
    pub xml: GFOwnedBytes,
    pub report: GFOwnedBytes,
}

impl Default for GFOwnedBytes {
    fn default() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GFTime {
    pub numerator: i64,
    pub denominator: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GFTimeRange {
    pub start: GFTime,
    pub duration: GFTime,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GFRenderParameters {
    pub fov: f64,
    pub smoothness: f64,
    pub lens_correction: f64,
    pub horizon_lock_amount: f64,
    pub horizon_lock_roll: f64,
    pub zoom_mode: i32,
    pub overview: u8,
    pub reserved: [u8; 3],
}

pub const GF_FRAME_GEOMETRY_VERSION_LEGACY: u32 = 0;
pub const GF_FRAME_GEOMETRY_VERSION: u32 = 1;

pub type GFGeometryValidity = u32;
pub const GF_GEOMETRY_VALIDITY_LEGACY_UNKNOWN: GFGeometryValidity = 0;
pub const GF_GEOMETRY_VALIDITY_VALID: GFGeometryValidity = 1;
pub const GF_GEOMETRY_VALIDITY_INVALID: GFGeometryValidity = 2;

pub type GFGeometrySupport = u32;
pub const GF_GEOMETRY_SUPPORT_UNKNOWN: GFGeometrySupport = 0;
pub const GF_GEOMETRY_SUPPORT_AFFINE_2D: GFGeometrySupport = 1;
pub const GF_GEOMETRY_SUPPORT_PERSPECTIVE_UNVERIFIED: GFGeometrySupport = 2;
pub const GF_GEOMETRY_SUPPORT_UNSUPPORTED: GFGeometrySupport = 3;

pub type GFImageOrigin = u32;
pub const GF_IMAGE_ORIGIN_BOTTOM_LEFT: GFImageOrigin = 0;
pub const GF_IMAGE_ORIGIN_TOP_LEFT: GFImageOrigin = 2;
pub const GF_IMAGE_ORIGIN_UNKNOWN: GFImageOrigin = u32::MAX;

pub type GFRotationDegrees = i32;
pub const GF_ROTATION_NONE: GFRotationDegrees = 0;
pub const GF_ROTATION_CLOCKWISE_90: GFRotationDegrees = 90;
pub const GF_ROTATION_180: GFRotationDegrees = 180;
pub const GF_ROTATION_CLOCKWISE_270: GFRotationDegrees = 270;
pub const GF_ROTATION_UNKNOWN: GFRotationDegrees = i32::MIN;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GFDimensionsU32 {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GFRectI32 {
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    pub top: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GFAffineTransform {
    pub values: [f64; 9],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GFFrameGeometry {
    pub version: u32,
    pub struct_size: u32,
    pub validity: GFGeometryValidity,
    pub support: GFGeometrySupport,
    pub source_dimensions: GFDimensionsU32,
    pub oriented_dimensions: GFDimensionsU32,
    pub tile_dimensions: GFDimensionsU32,
    pub output_dimensions: GFDimensionsU32,
    pub source_rect: GFRectI32,
    pub destination_rect: GFRectI32,
    pub source_origin: GFImageOrigin,
    pub destination_origin: GFImageOrigin,
    pub input_rotation: GFRotationDegrees,
    pub video_rotation: GFRotationDegrees,
    pub forward_transform: GFAffineTransform,
    pub inverse_transform: GFAffineTransform,
    pub reserved: [u32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GFFrameGeometryMode {
    LegacyUnknown,
    ValidatedAffine,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GFProjectGeometry {
    pub input_dimensions: GFDimensionsU32,
    pub output_dimensions: GFDimensionsU32,
    pub video_rotation: GFRotationDegrees,
    pub reserved: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct GFMappedBufferGeometry {
    pub dimensions: GFDimensionsU32,
    pub rect: Option<(usize, usize, usize, usize)>,
    pub rotation: Option<f32>,
    pub post_affine: Option<PostAffine>,
}

impl PartialEq for GFMappedBufferGeometry {
    fn eq(&self, other: &Self) -> bool {
        self.dimensions == other.dimensions
            && self.rect == other.rect
            && self.rotation.map(f32::to_bits) == other.rotation.map(f32::to_bits)
            && match (self.post_affine, other.post_affine) {
                (None, None) => true,
                (Some(left), Some(right)) => {
                    left.rotation_deg.to_bits() == right.rotation_deg.to_bits()
                        && left.zoom.to_bits() == right.zoom.to_bits()
                        && left.scale_xy.map(f32::to_bits) == right.scale_xy.map(f32::to_bits)
                        && left.offset_norm.map(f32::to_bits) == right.offset_norm.map(f32::to_bits)
                }
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GFMappedFrameGeometry {
    pub mode: GFFrameGeometryMode,
    pub stabilization_input_dimensions: Option<GFDimensionsU32>,
    pub stabilization_output_dimensions: Option<GFDimensionsU32>,
    pub video_rotation: Option<GFRotationDegrees>,
    pub input: GFMappedBufferGeometry,
    pub output: GFMappedBufferGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GFGeometryMappingError {
    pub status: GFStatus,
    pub message: String,
}

impl std::fmt::Display for GFGeometryMappingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GFGeometryMappingError {}

const _: () = {
    assert!(std::mem::size_of::<GFDimensionsU32>() == 8);
    assert!(std::mem::size_of::<GFRectI32>() == 16);
    assert!(std::mem::size_of::<GFAffineTransform>() == 72);
    assert!(std::mem::size_of::<GFProjectGeometry>() == 24);
    assert!(std::mem::align_of::<GFProjectGeometry>() == 4);
    assert!(std::mem::size_of::<GFFrameGeometry>() == 256);
    assert!(std::mem::align_of::<GFFrameGeometry>() == 8);
    assert!(std::mem::offset_of!(GFFrameGeometry, source_dimensions) == 16);
    assert!(std::mem::offset_of!(GFFrameGeometry, source_rect) == 48);
    assert!(std::mem::offset_of!(GFFrameGeometry, forward_transform) == 96);
    assert!(std::mem::offset_of!(GFFrameGeometry, inverse_transform) == 168);
    assert!(std::mem::offset_of!(GFFrameGeometry, reserved) == 240);
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GFMetalRenderRequest {
    pub input_texture: *mut c_void,
    pub output_texture: *mut c_void,
    pub command_queue: *mut c_void,
    pub device_registry_id: u64,
    pub width: u32,
    pub height: u32,
    pub input_row_bytes: u32,
    pub output_row_bytes: u32,
    pub pixel_format: u32,
    pub effect_local_time: GFTime,
    pub effect_bounds: GFTimeRange,
    pub input_bounds: GFTimeRange,
    pub geometry: GFFrameGeometry,
}

const _: () = {
    assert!(std::mem::offset_of!(GFMetalRenderRequest, geometry) == 136);
    assert!(std::mem::size_of::<GFMetalRenderRequest>() == 392);
};

struct LoadedProject {
    manager: Arc<StabilizationManager>,
    _bytes: Arc<[u8]>,
}

#[derive(Serialize, Deserialize)]
struct ProjectPayload {
    version: u32,
    codec: String,
    uncompressed_len: u64,
    content_sha256: String,
    project_base64: String,
}

#[derive(Debug)]
struct PayloadError {
    status: GFStatus,
    message: String,
}

#[derive(Deserialize)]
struct TimingEnvelope {
    version: u32,
    fcpxml_version: String,
    structure_sha256: String,
    asset_ref: String,
    mapping: Vec<TimingEnvelopePoint>,
    effect_bounds: TimingEnvelopeBounds,
    input_bounds: TimingEnvelopeBounds,
    render_ready_snapshot: bool,
}

#[derive(Deserialize)]
struct TimingEnvelopePoint {
    local: String,
    source: String,
}

#[derive(Deserialize)]
struct TimingEnvelopeBounds {
    start: String,
    duration: String,
}

#[derive(Clone)]
struct LoadedTiming {
    mapping: Vec<(Ratio<i128>, Ratio<i128>)>,
}

#[derive(Default)]
struct InstanceState {
    project: Option<LoadedProject>,
    timing: Option<LoadedTiming>,
}

pub struct GFFinalCutInstance {
    state: RwLock<InstanceState>,
}

impl Default for GFFinalCutInstance {
    fn default() -> Self {
        Self {
            state: RwLock::new(InstanceState::default()),
        }
    }
}

unsafe fn clear_error_slot(out_error: *mut *mut GFError) {
    if !out_error.is_null() {
        unsafe { *out_error = std::ptr::null_mut() };
    }
}

fn allocated_error(code: GFStatus, message: &str) -> *mut GFError {
    let sanitized = message.replace('\0', " ");
    let message = CString::new(sanitized)
        .expect("sanitized error text contains no interior NUL")
        .into_raw();
    Box::into_raw(Box::new(GFError { code, message }))
}

unsafe fn set_error(out_error: *mut *mut GFError, code: GFStatus, message: &str) {
    if !out_error.is_null() {
        unsafe { *out_error = allocated_error(code, message) };
    }
}

fn validate_project_sync_readiness(
    project: &serde_json::Value,
    has_sync_points: bool,
    has_accurate_timestamps: bool,
) -> Result<(), String> {
    let has_sync_workflow = project
        .get("synchronization")
        .is_some_and(serde_json::Value::is_object);
    let is_braw = project
        .get("videofile")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|path| path.to_ascii_lowercase().ends_with(".braw"));
    if has_sync_workflow && !has_sync_points && (!has_accurate_timestamps || is_braw) {
        return Err(
            "gyroflow project has no completed synchronization data; synchronize and save it in Gyroflow before importing"
                .to_string(),
        );
    }
    Ok(())
}

fn parse_project(project_bytes: &[u8]) -> Result<LoadedProject, String> {
    if project_bytes.is_empty() {
        return Err("gyroflow project bytes are empty".to_string());
    }
    std::str::from_utf8(project_bytes)
        .map_err(|error| format!("gyroflow project is not valid UTF-8: {error}"))?;
    let project_json: serde_json::Value = serde_json::from_slice(project_bytes)
        .map_err(|error| format!("gyroflow project JSON is invalid: {error}"))?;

    let manager = StabilizationManager::default();
    let mut is_preset = false;
    manager
        .import_gyroflow_data(
            project_bytes,
            true,
            None,
            |_| (),
            Arc::new(AtomicBool::new(false)),
            &mut is_preset,
            true,
        )
        .map_err(|error| format!("gyroflow project import failed: {error}"))?;
    let (has_sync_points, has_accurate_timestamps) = {
        let gyro = manager.gyro.read();
        let has_sync_points = !gyro.get_offsets().is_empty();
        let has_accurate_timestamps = gyro.file_metadata.read().has_accurate_timestamps;
        (has_sync_points, has_accurate_timestamps)
    };
    validate_project_sync_readiness(&project_json, has_sync_points, has_accurate_timestamps)?;
    Ok(LoadedProject {
        manager: Arc::new(manager),
        _bytes: Arc::from(project_bytes),
    })
}

fn encode_project_payload(project_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if project_bytes.is_empty() {
        return Err("gyroflow project bytes are empty".to_string());
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(project_bytes)
        .map_err(|error| format!("project payload compression failed: {error}"))?;
    let compressed = encoder
        .finish()
        .map_err(|error| format!("project payload compression failed: {error}"))?;
    let mut envelope_bytes = Vec::with_capacity(PROJECT_PAYLOAD_V2_HEADER_BYTES + compressed.len());
    envelope_bytes.extend_from_slice(PROJECT_PAYLOAD_V2_MAGIC);
    envelope_bytes.extend_from_slice(&(project_bytes.len() as u64).to_be_bytes());
    envelope_bytes.extend_from_slice(&Sha256::digest(project_bytes));
    envelope_bytes.extend_from_slice(&compressed);
    Ok(STANDARD.encode(envelope_bytes).into_bytes())
}

fn decode_project_payload(payload_bytes: &[u8]) -> Result<Vec<u8>, PayloadError> {
    let envelope_bytes = STANDARD
        .decode(payload_bytes)
        .map_err(|error| PayloadError {
            status: GFStatus::InvalidProject,
            message: format!("project payload Base64 is invalid: {error}"),
        })?;
    let (uncompressed_len, content_sha256, compressed) =
        if envelope_bytes.starts_with(PROJECT_PAYLOAD_V2_MAGIC) {
            if envelope_bytes.len() <= PROJECT_PAYLOAD_V2_HEADER_BYTES {
                return Err(PayloadError {
                    status: GFStatus::InvalidProject,
                    message: "project payload v2 envelope is truncated".to_string(),
                });
            }
            let uncompressed_len = u64::from_be_bytes(
                envelope_bytes[7..15]
                    .try_into()
                    .expect("fixed v2 length field"),
            );
            let content_sha256 = envelope_bytes[15..47]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            (
                uncompressed_len,
                content_sha256,
                envelope_bytes[PROJECT_PAYLOAD_V2_HEADER_BYTES..].to_vec(),
            )
        } else {
            if envelope_bytes.starts_with(b"GFPRJ") {
                return Err(PayloadError {
                    status: GFStatus::UnknownPayloadVersion,
                    message: "unsupported binary project payload version".to_string(),
                });
            }
            let envelope: ProjectPayload =
                serde_json::from_slice(&envelope_bytes).map_err(|error| PayloadError {
                    status: GFStatus::InvalidProject,
                    message: format!("project payload envelope is invalid: {error}"),
                })?;
            if envelope.version != PROJECT_PAYLOAD_VERSION {
                return Err(PayloadError {
                    status: GFStatus::UnknownPayloadVersion,
                    message: format!(
                        "unsupported project payload version {}; expected {} or binary v2",
                        envelope.version, PROJECT_PAYLOAD_VERSION
                    ),
                });
            }
            if envelope.codec != PROJECT_PAYLOAD_CODEC {
                return Err(PayloadError {
                    status: GFStatus::UnknownPayloadVersion,
                    message: format!("unsupported project payload codec {}", envelope.codec),
                });
            }
            let compressed = STANDARD
                .decode(envelope.project_base64.as_bytes())
                .map_err(|error| PayloadError {
                    status: GFStatus::InvalidProject,
                    message: format!("embedded project Base64 is invalid: {error}"),
                })?;
            (
                envelope.uncompressed_len,
                envelope.content_sha256,
                compressed,
            )
        };
    if uncompressed_len == 0 || uncompressed_len > MAX_PROJECT_BYTES {
        return Err(PayloadError {
            status: GFStatus::InvalidProject,
            message: format!(
                "embedded project length {} is outside the supported range",
                uncompressed_len
            ),
        });
    }
    let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
    let mut project_bytes = Vec::with_capacity(uncompressed_len as usize);
    decoder
        .by_ref()
        .take(MAX_PROJECT_BYTES + 1)
        .read_to_end(&mut project_bytes)
        .map_err(|error| PayloadError {
            status: GFStatus::InvalidProject,
            message: format!("embedded project decompression failed: {error}"),
        })?;
    if project_bytes.len() as u64 != uncompressed_len {
        return Err(PayloadError {
            status: GFStatus::InvalidProject,
            message: "embedded project uncompressed length mismatch".to_string(),
        });
    }
    let actual_hash = format!("{:x}", Sha256::digest(&project_bytes));
    if actual_hash != content_sha256 {
        return Err(PayloadError {
            status: GFStatus::InvalidProject,
            message: "embedded gyroflow project content hash mismatch".to_string(),
        });
    }
    Ok(project_bytes)
}

fn owned_bytes(bytes: Vec<u8>) -> GFOwnedBytes {
    let mut bytes = bytes;
    let owned = GFOwnedBytes {
        data: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    owned
}

fn apply_render_parameters(
    project: &LoadedProject,
    parameters: &GFRenderParameters,
) -> Result<(), String> {
    let finite_values = [
        parameters.fov,
        parameters.smoothness,
        parameters.lens_correction,
        parameters.horizon_lock_amount,
        parameters.horizon_lock_roll,
    ];
    if finite_values.iter().any(|value| !value.is_finite()) {
        return Err("render parameters must be finite".to_string());
    }
    if !(0.1..=3.0).contains(&parameters.fov) {
        return Err("FOV must be in [0.1, 3.0]".to_string());
    }
    if !(1.0..=300.0).contains(&parameters.smoothness) {
        return Err("Smoothness must be in [1, 300]".to_string());
    }
    if !(0.0..=100.0).contains(&parameters.lens_correction) {
        return Err("Lens Correction must be in [0, 100]".to_string());
    }
    if !(0.0..=100.0).contains(&parameters.horizon_lock_amount) {
        return Err("Horizon Lock amount must be in [0, 100]".to_string());
    }
    if !(-100.0..=100.0).contains(&parameters.horizon_lock_roll) {
        return Err("Horizon Lock roll must be in [-100, 100]".to_string());
    }
    if !(0..=2).contains(&parameters.zoom_mode) {
        return Err("Zoom Mode must be 0, 1, or 2".to_string());
    }
    if parameters.overview > 1 || parameters.reserved != [0; 3] {
        return Err("Overview or reserved parameter bytes are invalid".to_string());
    }

    let manager = &project.manager;
    manager.params.write().framebuffer_inverted = true;
    manager.set_fov(parameters.fov);
    manager.set_smoothing_param("smoothness", parameters.smoothness / 100.0);
    manager.set_lens_correction_amount(parameters.lens_correction / 100.0);
    manager.set_horizon_lock(
        parameters.horizon_lock_amount,
        parameters.horizon_lock_roll,
        false,
        0.0,
        false,
        0.0,
        0.0,
        0.0,
        0.0,
    );
    manager.set_adaptive_zoom(match parameters.zoom_mode {
        0 => 0.0,
        2 => -1.0,
        _ => 4.0,
    });
    manager.set_fov_overview(parameters.overview != 0);
    manager.invalidate_blocking_smoothing();
    manager.recompute_blocking();
    manager
        .params
        .write()
        .calculate_ramped_timestamps(&manager.keyframes.read(), false, false);
    Ok(())
}

fn parse_ratio(value: &str) -> Result<Ratio<i128>, String> {
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| format!("timing value is not a rational: {value}"))?;
    let numerator = numerator
        .parse::<i128>()
        .map_err(|_| format!("timing numerator is invalid: {value}"))?;
    let denominator = denominator
        .parse::<i128>()
        .map_err(|_| format!("timing denominator is invalid: {value}"))?;
    if denominator == 0 {
        return Err("timing denominator is zero".to_string());
    }
    Ok(Ratio::new(numerator, denominator))
}

fn ratio_from_time(time: GFTime) -> Result<Ratio<i128>, String> {
    if time.denominator == 0 {
        return Err("observed time denominator is zero".to_string());
    }
    Ok(Ratio::new(
        i128::from(time.numerator),
        i128::from(time.denominator),
    ))
}

fn time_from_ratio(value: &Ratio<i128>) -> Result<GFTime, String> {
    let numerator = i64::try_from(*value.numer())
        .map_err(|_| "resolved source-time numerator exceeds i64".to_string())?;
    let denominator = i64::try_from(*value.denom())
        .map_err(|_| "resolved source-time denominator exceeds i64".to_string())?;
    Ok(GFTime {
        numerator,
        denominator,
    })
}

fn ratio_string(value: &Ratio<i128>) -> String {
    format!("{}/{}", value.numer(), value.denom())
}

fn decode_timing_payload(
    payload_bytes: &[u8],
    observed_effect_bounds: GFTimeRange,
    observed_input_bounds: GFTimeRange,
) -> Result<LoadedTiming, PayloadError> {
    if payload_bytes.is_empty() {
        return Err(PayloadError {
            status: GFStatus::MissingTiming,
            message: "Reprocess Project Required: timing payload is missing".to_string(),
        });
    }
    let envelope_bytes = STANDARD
        .decode(payload_bytes)
        .map_err(|error| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: timing Base64 is invalid: {error}"),
        })?;
    let envelope: TimingEnvelope =
        serde_json::from_slice(&envelope_bytes).map_err(|error| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: timing envelope is invalid: {error}"),
        })?;
    if envelope.version != 1 {
        return Err(PayloadError {
            status: GFStatus::UnknownPayloadVersion,
            message: format!(
                "Reprocess Project Required: unsupported timing payload version {}",
                envelope.version
            ),
        });
    }
    if !matches!(envelope.fcpxml_version.as_str(), "1.12" | "1.13" | "1.14") {
        return Err(PayloadError {
            status: GFStatus::UnknownPayloadVersion,
            message: format!(
                "Reprocess Project Required: unsupported FCPXML version {}",
                envelope.fcpxml_version
            ),
        });
    }
    if !envelope.render_ready_snapshot
        || envelope.asset_ref.is_empty()
        || envelope.structure_sha256.len() != 64
        || !envelope
            .structure_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PayloadError {
            status: GFStatus::MissingTiming,
            message: "Reprocess Project Required: timing structure metadata is invalid".to_string(),
        });
    }

    let payload_effect_start =
        parse_ratio(&envelope.effect_bounds.start).map_err(|message| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let payload_effect_duration =
        parse_ratio(&envelope.effect_bounds.duration).map_err(|message| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let payload_input_start =
        parse_ratio(&envelope.input_bounds.start).map_err(|message| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let payload_input_duration =
        parse_ratio(&envelope.input_bounds.duration).map_err(|message| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let observed_effect_start =
        ratio_from_time(observed_effect_bounds.start).map_err(|message| PayloadError {
            status: GFStatus::StaleTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let observed_effect_duration =
        ratio_from_time(observed_effect_bounds.duration).map_err(|message| PayloadError {
            status: GFStatus::StaleTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let observed_input_start =
        ratio_from_time(observed_input_bounds.start).map_err(|message| PayloadError {
            status: GFStatus::StaleTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    let observed_input_duration =
        ratio_from_time(observed_input_bounds.duration).map_err(|message| PayloadError {
            status: GFStatus::StaleTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    if payload_effect_start != observed_effect_start
        || payload_effect_duration != observed_effect_duration
        || payload_input_start != observed_input_start
        || payload_input_duration != observed_input_duration
    {
        return Err(PayloadError {
            status: GFStatus::StaleTiming,
            message: format!(
                "Reprocess Project Required: observed effect/input bounds changed; expected effect start={} duration={}, input start={} duration={}; observed effect start={} duration={}, input start={} duration={}",
                ratio_string(&payload_effect_start),
                ratio_string(&payload_effect_duration),
                ratio_string(&payload_input_start),
                ratio_string(&payload_input_duration),
                ratio_string(&observed_effect_start),
                ratio_string(&observed_effect_duration),
                ratio_string(&observed_input_start),
                ratio_string(&observed_input_duration),
            ),
        });
    }

    let mapping: Vec<_> = envelope
        .mapping
        .into_iter()
        .map(|point| Ok((parse_ratio(&point.local)?, parse_ratio(&point.source)?)))
        .collect::<Result<_, String>>()
        .map_err(|message| PayloadError {
            status: GFStatus::MissingTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
    if mapping.len() < 2
        || mapping.first().map(|point| point.0.clone()) != Some(Ratio::from_integer(0))
        || mapping.last().map(|point| point.0.clone()) != Some(payload_effect_duration)
        || mapping.windows(2).any(|window| window[0].0 >= window[1].0)
    {
        return Err(PayloadError {
            status: GFStatus::MissingTiming,
            message: "Reprocess Project Required: timing mapping is incomplete or ambiguous"
                .to_string(),
        });
    }
    Ok(LoadedTiming { mapping })
}

fn resolve_source_time(
    timing: &LoadedTiming,
    local_time: Ratio<i128>,
) -> Result<Ratio<i128>, String> {
    let first = timing.mapping.first().unwrap();
    let last = timing.mapping.last().unwrap();
    if local_time < first.0 || local_time > last.0 {
        return Err("effect-local time is outside the processed snapshot".to_string());
    }
    if local_time == last.0 {
        return Ok(last.1.clone());
    }
    let segment = timing
        .mapping
        .windows(2)
        .find(|window| local_time >= window[0].0 && local_time <= window[1].0)
        .ok_or_else(|| "timing mapping has no segment for effect-local time".to_string())?;
    let local_delta = segment[1].0.clone() - segment[0].0.clone();
    let position = (local_time - segment[0].0.clone()) / local_delta;
    Ok(segment[0].1.clone() + position * (segment[1].1.clone() - segment[0].1.clone()))
}

fn resolve_render_source_time(
    timing: Option<&LoadedTiming>,
    render_time: Ratio<i128>,
    effect_bounds: GFTimeRange,
) -> Result<Ratio<i128>, PayloadError> {
    if let Some(timing) = timing {
        let start = ratio_from_time(effect_bounds.start).map_err(|message| PayloadError {
            status: GFStatus::StaleTiming,
            message: format!("Reprocess Project Required: {message}"),
        })?;
        return resolve_source_time(timing, render_time - start).map_err(|message| PayloadError {
            status: GFStatus::StaleTiming,
            message: format!("Reprocess Project Required: {message}"),
        });
    }

    let start = ratio_from_time(effect_bounds.start).map_err(|message| PayloadError {
        status: GFStatus::InvalidArgument,
        message: format!("Direct stabilization requires valid effect bounds: {message}"),
    })?;
    let duration = ratio_from_time(effect_bounds.duration).map_err(|message| PayloadError {
        status: GFStatus::InvalidArgument,
        message: format!("Direct stabilization requires valid effect bounds: {message}"),
    })?;
    if duration <= Ratio::from_integer(0) {
        return Err(PayloadError {
            status: GFStatus::InvalidArgument,
            message: "Direct stabilization requires a positive effect duration".to_string(),
        });
    }
    let source_time = render_time - start;
    if source_time < Ratio::from_integer(0) || source_time > duration {
        return Err(PayloadError {
            status: GFStatus::InvalidArgument,
            message: "Direct stabilization render time is outside the effect bounds".to_string(),
        });
    }
    Ok(source_time)
}

fn is_zero_dimensions(dimensions: GFDimensionsU32) -> bool {
    dimensions.width == 0 && dimensions.height == 0
}

fn is_zero_rect(rect: GFRectI32) -> bool {
    rect.left == 0 && rect.bottom == 0 && rect.right == 0 && rect.top == 0
}

fn is_legacy_zero_geometry(geometry: &GFFrameGeometry) -> bool {
    geometry.version == GF_FRAME_GEOMETRY_VERSION_LEGACY
        && geometry.struct_size == 0
        && geometry.validity == GF_GEOMETRY_VALIDITY_LEGACY_UNKNOWN
        && geometry.support == GF_GEOMETRY_SUPPORT_UNKNOWN
        && is_zero_dimensions(geometry.source_dimensions)
        && is_zero_dimensions(geometry.oriented_dimensions)
        && is_zero_dimensions(geometry.tile_dimensions)
        && is_zero_dimensions(geometry.output_dimensions)
        && is_zero_rect(geometry.source_rect)
        && is_zero_rect(geometry.destination_rect)
        && geometry.source_origin == 0
        && geometry.destination_origin == 0
        && geometry.input_rotation == 0
        && geometry.video_rotation == 0
        && geometry
            .forward_transform
            .values
            .iter()
            .all(|value| *value == 0.0)
        && geometry
            .inverse_transform
            .values
            .iter()
            .all(|value| *value == 0.0)
        && geometry.reserved.iter().all(|value| *value == 0)
}

fn rect_dimensions(rect: GFRectI32) -> Result<GFDimensionsU32, &'static str> {
    let width = i64::from(rect.right) - i64::from(rect.left);
    let height = i64::from(rect.top) - i64::from(rect.bottom);
    if width <= 0 || height <= 0 {
        return Err("geometry rectangles must have positive extents");
    }
    Ok(GFDimensionsU32 {
        width: u32::try_from(width).map_err(|_| "geometry rectangle width exceeds uint32_t")?,
        height: u32::try_from(height).map_err(|_| "geometry rectangle height exceeds uint32_t")?,
    })
}

fn is_affine_matrix(values: &[f64; 9]) -> bool {
    values[6].abs() <= 1.0e-12 && values[7].abs() <= 1.0e-12 && (values[8] - 1.0).abs() <= 1.0e-12
}

fn affine_determinant(values: &[f64; 9]) -> f64 {
    values[0] * values[4] - values[1] * values[3]
}

fn multiply_matrix3(left: &[f64; 9], right: &[f64; 9]) -> [f64; 9] {
    let mut product = [0.0; 9];
    for row in 0..3 {
        for column in 0..3 {
            product[row * 3 + column] = (0..3)
                .map(|index| left[row * 3 + index] * right[index * 3 + column])
                .sum();
        }
    }
    product
}

fn is_approximately_identity(values: &[f64; 9]) -> bool {
    const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    values
        .iter()
        .zip(IDENTITY)
        .all(|(actual, expected)| (*actual - expected).abs() <= 1.0e-9)
}

fn validate_affine_pair(
    forward: &GFAffineTransform,
    inverse: &GFAffineTransform,
) -> Result<(), &'static str> {
    if !forward
        .values
        .iter()
        .chain(inverse.values.iter())
        .all(|value| value.is_finite())
    {
        return Err("geometry affine values must be finite");
    }
    if !is_affine_matrix(&forward.values) || !is_affine_matrix(&inverse.values) {
        return Err("geometry matrices must be homogeneous 2D affine transforms");
    }
    if affine_determinant(&forward.values).abs() <= 1.0e-12
        || affine_determinant(&inverse.values).abs() <= 1.0e-12
    {
        return Err("geometry affine transform is singular");
    }
    if !is_approximately_identity(&multiply_matrix3(&forward.values, &inverse.values))
        || !is_approximately_identity(&multiply_matrix3(&inverse.values, &forward.values))
    {
        return Err("geometry forward and inverse affine transforms are inconsistent");
    }
    Ok(())
}

pub fn validate_frame_geometry(
    geometry: &GFFrameGeometry,
) -> Result<GFFrameGeometryMode, &'static str> {
    if geometry.version == GF_FRAME_GEOMETRY_VERSION_LEGACY {
        return if is_legacy_zero_geometry(geometry) {
            Ok(GFFrameGeometryMode::LegacyUnknown)
        } else {
            Err("legacy geometry must be fully zero-initialized")
        };
    }
    if geometry.version != GF_FRAME_GEOMETRY_VERSION {
        return Err("unsupported frame geometry version");
    }
    if geometry.struct_size != std::mem::size_of::<GFFrameGeometry>() as u32 {
        return Err("frame geometry size does not match its version");
    }
    match geometry.validity {
        GF_GEOMETRY_VALIDITY_VALID => {}
        GF_GEOMETRY_VALIDITY_LEGACY_UNKNOWN => {
            return Err("versioned frame geometry validity cannot be legacy/unknown");
        }
        GF_GEOMETRY_VALIDITY_INVALID => {
            return Err("frame geometry validity is explicitly invalid");
        }
        _ => return Err("unknown frame geometry validity classification"),
    }
    match geometry.support {
        GF_GEOMETRY_SUPPORT_AFFINE_2D => {}
        GF_GEOMETRY_SUPPORT_PERSPECTIVE_UNVERIFIED => {
            return Err("unverified perspective geometry is unsupported");
        }
        GF_GEOMETRY_SUPPORT_UNKNOWN => {
            return Err("frame geometry support classification is unknown");
        }
        GF_GEOMETRY_SUPPORT_UNSUPPORTED => {
            return Err("frame geometry is explicitly unsupported");
        }
        _ => return Err("unknown frame geometry support classification"),
    }
    if !matches!(
        geometry.source_origin,
        GF_IMAGE_ORIGIN_BOTTOM_LEFT | GF_IMAGE_ORIGIN_TOP_LEFT
    ) || !matches!(
        geometry.destination_origin,
        GF_IMAGE_ORIGIN_BOTTOM_LEFT | GF_IMAGE_ORIGIN_TOP_LEFT
    ) {
        return Err("unknown frame geometry image origin");
    }
    if !matches!(
        geometry.input_rotation,
        GF_ROTATION_NONE | GF_ROTATION_CLOCKWISE_90 | GF_ROTATION_180 | GF_ROTATION_CLOCKWISE_270
    ) || !matches!(
        geometry.video_rotation,
        GF_ROTATION_NONE | GF_ROTATION_CLOCKWISE_90 | GF_ROTATION_180 | GF_ROTATION_CLOCKWISE_270
    ) {
        return Err("unknown frame geometry rotation");
    }
    if geometry.reserved.iter().any(|value| *value != 0) {
        return Err("frame geometry reserved fields must be zero");
    }
    let dimensions = [
        geometry.source_dimensions,
        geometry.oriented_dimensions,
        geometry.tile_dimensions,
        geometry.output_dimensions,
    ];
    if dimensions
        .iter()
        .any(|dimensions| dimensions.width == 0 || dimensions.height == 0)
    {
        return Err("frame geometry dimensions must be non-zero");
    }
    rect_dimensions(geometry.source_rect)?;
    rect_dimensions(geometry.destination_rect)?;
    validate_affine_pair(&geometry.forward_transform, &geometry.inverse_transform)?;
    Ok(GFFrameGeometryMode::ValidatedAffine)
}

fn geometry_mapping_error(message: impl Into<String>) -> GFGeometryMappingError {
    GFGeometryMappingError {
        status: GFStatus::InvalidArgument,
        message: message.into(),
    }
}

fn local_to_absolute_transform(rect: GFRectI32, origin: GFImageOrigin) -> [f64; 9] {
    match origin {
        GF_IMAGE_ORIGIN_BOTTOM_LEFT => [
            1.0,
            0.0,
            f64::from(rect.left),
            0.0,
            1.0,
            f64::from(rect.bottom),
            0.0,
            0.0,
            1.0,
        ],
        GF_IMAGE_ORIGIN_TOP_LEFT => [
            1.0,
            0.0,
            f64::from(rect.left),
            0.0,
            -1.0,
            f64::from(rect.top),
            0.0,
            0.0,
            1.0,
        ],
        _ => unreachable!("frame geometry origin was validated"),
    }
}

fn absolute_to_local_transform(rect: GFRectI32, origin: GFImageOrigin) -> [f64; 9] {
    match origin {
        GF_IMAGE_ORIGIN_BOTTOM_LEFT => [
            1.0,
            0.0,
            -f64::from(rect.left),
            0.0,
            1.0,
            -f64::from(rect.bottom),
            0.0,
            0.0,
            1.0,
        ],
        GF_IMAGE_ORIGIN_TOP_LEFT => [
            1.0,
            0.0,
            -f64::from(rect.left),
            0.0,
            -1.0,
            f64::from(rect.top),
            0.0,
            0.0,
            1.0,
        ],
        _ => unreachable!("frame geometry origin was validated"),
    }
}

fn localized_forward_transform(geometry: &GFFrameGeometry) -> [f64; 9] {
    let source_local_to_absolute =
        local_to_absolute_transform(geometry.source_rect, geometry.source_origin);
    let destination_absolute_to_local =
        absolute_to_local_transform(geometry.destination_rect, geometry.destination_origin);
    multiply_matrix3(
        &destination_absolute_to_local,
        &multiply_matrix3(
            &geometry.forward_transform.values,
            &source_local_to_absolute,
        ),
    )
}

fn post_affine_from_local_transform(
    transform: &[f64; 9],
    stabilization_output: GFDimensionsU32,
    destination_output: GFDimensionsU32,
) -> Result<Option<PostAffine>, GFGeometryMappingError> {
    const TOLERANCE: f64 = 1.0e-9;
    let a = transform[0];
    let b = transform[1];
    let c = transform[3];
    let d = transform[4];
    let determinant = a * d - b * c;
    if determinant < -TOLERANCE {
        return Err(geometry_mapping_error(
            "frame geometry reflection is unsupported by core PostAffine",
        ));
    }
    if determinant <= TOLERANCE {
        return Err(geometry_mapping_error(
            "frame geometry affine transform is singular",
        ));
    }

    let column_x = a.hypot(c);
    let column_y = b.hypot(d);
    let scale_reference = column_x.max(column_y).max(1.0);
    let column_dot = a * b + c * d;
    if column_dot.abs() > TOLERANCE * scale_reference * scale_reference {
        return Err(geometry_mapping_error(
            "frame geometry shear is unsupported by core PostAffine",
        ));
    }
    let is_uniform = (column_x - column_y).abs() <= TOLERANCE * scale_reference;
    let (zoom, scale_xy) = if is_uniform {
        ((column_x + column_y) * 0.5, [1.0, 1.0])
    } else {
        (1.0, [column_x as f32, column_y as f32])
    };
    let rotation_radians = c.atan2(a);
    let source_center_x = f64::from(stabilization_output.width) * 0.5;
    let source_center_y = f64::from(stabilization_output.height) * 0.5;
    let destination_center_x = f64::from(destination_output.width) * 0.5;
    let destination_center_y = f64::from(destination_output.height) * 0.5;
    let translated_x =
        transform[2] + a * source_center_x + b * source_center_y - destination_center_x;
    let translated_y =
        transform[5] + c * source_center_x + d * source_center_y - destination_center_y;
    let inverse_determinant = 1.0 / determinant;
    let offset_x = (d * translated_x - b * translated_y) * inverse_determinant;
    let offset_y = (-c * translated_x + a * translated_y) * inverse_determinant;
    let post_affine = PostAffine {
        rotation_deg: rotation_radians.to_degrees() as f32,
        zoom: zoom as f32,
        scale_xy,
        offset_norm: [
            (offset_x / f64::from(stabilization_output.width)) as f32,
            (offset_y / f64::from(stabilization_output.height)) as f32,
        ],
    };
    if !post_affine.rotation_deg.is_finite()
        || !post_affine.zoom.is_finite()
        || !post_affine.scale_xy.iter().all(|value| value.is_finite())
        || !post_affine
            .offset_norm
            .iter()
            .all(|value| value.is_finite())
    {
        return Err(geometry_mapping_error(
            "frame geometry exceeds core PostAffine numeric range",
        ));
    }
    if (post_affine.rotation_deg as f64).abs() <= TOLERANCE
        && ((post_affine.zoom as f64) - 1.0).abs() <= TOLERANCE
        && post_affine
            .scale_xy
            .iter()
            .all(|value| ((*value as f64) - 1.0).abs() <= TOLERANCE)
        && post_affine
            .offset_norm
            .iter()
            .all(|value| (*value as f64).abs() <= TOLERANCE)
    {
        return Ok(None);
    }
    Ok(Some(post_affine))
}

/// Maps a validated, normalized Final Cut geometry snapshot to gyroflow-core buffer geometry.
///
/// Version 1 transforms are row-major forward maps from the current frame's oriented source
/// ideal-square-pixel coordinates to destination coordinates. Both domains use absolute pixel
/// boundary coordinates with +x right and +y up. Tile rectangles and image origins are rebased
/// before decomposition. The loaded project's input/output sizes and video rotation remain the
/// stabilization authority; this function validates them and never derives manager state from the
/// Final Cut canvas. Consequently, Final Cut geometry does not alter adaptive zoom or FOV state.
pub fn map_frame_geometry(
    geometry: &GFFrameGeometry,
    texture_dimensions: GFDimensionsU32,
    project: GFProjectGeometry,
) -> Result<GFMappedFrameGeometry, GFGeometryMappingError> {
    let mode = validate_frame_geometry(geometry).map_err(geometry_mapping_error)?;
    let legacy_buffer = GFMappedBufferGeometry {
        dimensions: texture_dimensions,
        rect: None,
        rotation: None,
        post_affine: None,
    };
    if mode == GFFrameGeometryMode::LegacyUnknown {
        return Ok(GFMappedFrameGeometry {
            mode,
            stabilization_input_dimensions: None,
            stabilization_output_dimensions: None,
            video_rotation: None,
            input: legacy_buffer,
            output: legacy_buffer,
        });
    }

    if texture_dimensions != geometry.tile_dimensions {
        return Err(geometry_mapping_error(
            "Metal texture dimensions conflict with frame geometry tile dimensions",
        ));
    }
    if geometry.source_dimensions != project.input_dimensions {
        return Err(geometry_mapping_error(
            "frame geometry source dimensions conflict with project input dimensions",
        ));
    }
    if geometry.oriented_dimensions != project.output_dimensions {
        return Err(geometry_mapping_error(
            "frame geometry oriented dimensions conflict with project output dimensions",
        ));
    }
    if geometry.video_rotation != project.video_rotation {
        return Err(geometry_mapping_error(
            "frame geometry video rotation conflicts with the loaded project",
        ));
    }
    if geometry.input_rotation != GF_ROTATION_NONE
        && geometry.input_rotation != project.video_rotation
    {
        return Err(geometry_mapping_error(
            "frame geometry input rotation conflicts with the loaded project",
        ));
    }
    let local_rect = Some((
        0,
        0,
        geometry.tile_dimensions.width as usize,
        geometry.tile_dimensions.height as usize,
    ));
    let post_affine = post_affine_from_local_transform(
        &localized_forward_transform(geometry),
        project.output_dimensions,
        geometry.output_dimensions,
    )?;
    Ok(GFMappedFrameGeometry {
        mode,
        stabilization_input_dimensions: Some(project.input_dimensions),
        stabilization_output_dimensions: Some(project.output_dimensions),
        video_rotation: Some(project.video_rotation),
        input: GFMappedBufferGeometry {
            dimensions: geometry.tile_dimensions,
            rect: local_rect,
            rotation: Some(geometry.input_rotation as f32),
            post_affine: None,
        },
        output: GFMappedBufferGeometry {
            dimensions: geometry.tile_dimensions,
            rect: local_rect,
            rotation: None,
            post_affine,
        },
    })
}

fn normalized_project_rotation(rotation: f64) -> Result<GFRotationDegrees, GFGeometryMappingError> {
    if !rotation.is_finite() {
        return Err(geometry_mapping_error(
            "loaded project video rotation must be finite",
        ));
    }
    let rounded = rotation.round();
    if (rotation - rounded).abs() > 1.0e-9 {
        return Err(geometry_mapping_error(
            "loaded project video rotation must be a quarter turn",
        ));
    }
    let normalized = (rounded as i64).rem_euclid(360) as i32;
    if matches!(
        normalized,
        GF_ROTATION_NONE | GF_ROTATION_CLOCKWISE_90 | GF_ROTATION_180 | GF_ROTATION_CLOCKWISE_270
    ) {
        Ok(normalized)
    } else {
        Err(geometry_mapping_error(
            "loaded project video rotation must be a quarter turn",
        ))
    }
}

fn project_geometry_from_manager(
    manager: &StabilizationManager,
) -> Result<GFProjectGeometry, GFGeometryMappingError> {
    let params = manager.params.read();
    let input_width = u32::try_from(params.size.0)
        .map_err(|_| geometry_mapping_error("project input width exceeds uint32_t"))?;
    let input_height = u32::try_from(params.size.1)
        .map_err(|_| geometry_mapping_error("project input height exceeds uint32_t"))?;
    let output_width = u32::try_from(params.output_size.0)
        .map_err(|_| geometry_mapping_error("project output width exceeds uint32_t"))?;
    let output_height = u32::try_from(params.output_size.1)
        .map_err(|_| geometry_mapping_error("project output height exceeds uint32_t"))?;
    if input_width == 0 || input_height == 0 || output_width == 0 || output_height == 0 {
        return Err(geometry_mapping_error(
            "loaded project geometry dimensions must be non-zero",
        ));
    }
    Ok(GFProjectGeometry {
        input_dimensions: GFDimensionsU32 {
            width: input_width,
            height: input_height,
        },
        output_dimensions: GFDimensionsU32 {
            width: output_width,
            height: output_height,
        },
        video_rotation: normalized_project_rotation(params.video_rotation)?,
        reserved: 0,
    })
}

fn validate_metal_render_request(request: &GFMetalRenderRequest) -> Result<(), PayloadError> {
    if request.input_texture.is_null() || request.output_texture.is_null() {
        return Err(PayloadError {
            status: GFStatus::NullPointer,
            message: "Metal render requires input and output textures".to_string(),
        });
    }
    if request.device_registry_id == 0 {
        return Err(PayloadError {
            status: GFStatus::InvalidArgument,
            message: "Metal render requires the FxImageTile device registry ID".to_string(),
        });
    }
    if request.pixel_format != 115 {
        return Err(PayloadError {
            status: GFStatus::UnsupportedPixelFormat,
            message: format!(
                "unsupported Final Cut Metal pixel format {}; expected RGBA16Float (115)",
                request.pixel_format
            ),
        });
    }
    if request.width == 0 || request.height < 4 {
        return Err(PayloadError {
            status: GFStatus::InvalidArgument,
            message:
                "Metal render dimensions must describe a non-empty frame at least four rows high"
                    .to_string(),
        });
    }
    let geometry_mode =
        validate_frame_geometry(&request.geometry).map_err(|message| PayloadError {
            status: GFStatus::InvalidArgument,
            message: message.to_string(),
        })?;
    if geometry_mode == GFFrameGeometryMode::ValidatedAffine
        && (request.width != request.geometry.tile_dimensions.width
            || request.height != request.geometry.tile_dimensions.height)
    {
        return Err(PayloadError {
            status: GFStatus::InvalidArgument,
            message: "Metal texture dimensions conflict with the frame geometry tile dimensions"
                .to_string(),
        });
    }
    let minimum_row_bytes = request.width.checked_mul(8).ok_or_else(|| PayloadError {
        status: GFStatus::InvalidArgument,
        message: "Metal render row-byte calculation overflowed".to_string(),
    })?;
    if request.input_row_bytes < minimum_row_bytes || request.output_row_bytes < minimum_row_bytes {
        return Err(PayloadError {
            status: GFStatus::InvalidArgument,
            message: "RGBA16Float row bytes are smaller than width * 8".to_string(),
        });
    }
    ratio_from_time(request.effect_local_time).map_err(|message| PayloadError {
        status: GFStatus::InvalidArgument,
        message,
    })?;
    Ok(())
}

fn rational_seconds_to_microseconds(value: &Ratio<i128>) -> Result<i64, String> {
    let scaled = value
        .numer()
        .checked_mul(1_000_000)
        .ok_or_else(|| "source-time microsecond conversion overflowed i128".to_string())?;
    let denominator = *value.denom();
    let quotient = scaled / denominator;
    let remainder = scaled % denominator;
    let round_away_from_zero = remainder
        .abs()
        .checked_mul(2)
        .is_some_and(|twice| twice >= denominator);
    let rounded = if round_away_from_zero {
        quotient + remainder.signum()
    } else {
        quotient
    };
    i64::try_from(rounded).map_err(|_| "source time exceeds the i64 microsecond range".to_string())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_create(
    out_error: *mut *mut GFError,
) -> *mut GFFinalCutInstance {
    unsafe { clear_error_slot(out_error) };
    match std::panic::catch_unwind(|| Box::into_raw(Box::new(GFFinalCutInstance::default()))) {
        Ok(instance) => instance,
        Err(_) => {
            unsafe { set_error(out_error, GFStatus::Panic, "instance creation panicked") };
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_free(instance: *mut GFFinalCutInstance) {
    if !instance.is_null() {
        unsafe { drop(Box::from_raw(instance)) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_load_project(
    instance: *mut GFFinalCutInstance,
    project_bytes: *const u8,
    project_len: usize,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "gyroflow project load requires an instance",
            )
        };
        return GFStatus::NullPointer;
    }
    if project_len == 0 {
        unsafe {
            set_error(
                out_error,
                GFStatus::InvalidProject,
                "gyroflow project bytes are empty",
            )
        };
        return GFStatus::InvalidProject;
    }
    if project_bytes.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "gyroflow project byte pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }

    let project_bytes = unsafe { std::slice::from_raw_parts(project_bytes, project_len) };
    let parsed = std::panic::catch_unwind(|| parse_project(project_bytes));
    match parsed {
        Ok(Ok(project)) => {
            let instance = unsafe { &*instance };
            match instance.state.write() {
                Ok(mut state) => {
                    state.project = Some(project);
                    state.timing = None;
                    GFStatus::Ok
                }
                Err(_) => {
                    unsafe {
                        set_error(
                            out_error,
                            GFStatus::Panic,
                            "gyroflow project state lock is poisoned",
                        )
                    };
                    GFStatus::Panic
                }
            }
        }
        Ok(Err(message)) => {
            unsafe { set_error(out_error, GFStatus::InvalidProject, &message) };
            GFStatus::InvalidProject
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "gyroflow project parsing panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_route_d_patch(
    input_bytes: *const u8,
    input_len: usize,
    processed_name_bytes: *const u8,
    processed_name_len: usize,
    out_result: *mut GFRouteDPatchResult,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if out_result.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "Route D patch output pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }
    unsafe { *out_result = GFRouteDPatchResult::default() };
    if input_len == 0 {
        unsafe {
            set_error(
                out_error,
                GFStatus::InvalidArgument,
                "Route D requires complete project FCPXML bytes",
            )
        };
        return GFStatus::InvalidArgument;
    }
    if input_bytes.is_null() || (processed_name_len > 0 && processed_name_bytes.is_null()) {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "Route D input or processed-name pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }
    let input = unsafe { std::slice::from_raw_parts(input_bytes, input_len) };
    let processed_name = if processed_name_len == 0 {
        None
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(processed_name_bytes, processed_name_len) };
        match std::str::from_utf8(bytes) {
            Ok(name) => Some(name),
            Err(error) => {
                unsafe {
                    set_error(
                        out_error,
                        GFStatus::InvalidArgument,
                        &format!("processed project name is not UTF-8: {error}"),
                    )
                };
                return GFStatus::InvalidArgument;
            }
        }
    };
    let patched = std::panic::catch_unwind(|| patch_fcpxml_project(input, processed_name));
    match patched {
        Ok(Ok(patched)) => {
            let report = serde_json::json!({
                "original_project_name": patched.original_project_name,
                "processed_project_name": patched.processed_project_name,
                "import_token": patched.import_token,
                "occurrence_count": patched.occurrence_count,
                "behavior": "Creates a new processed project and preserves the original project"
            });
            let report = match serde_json::to_vec(&report) {
                Ok(report) => report,
                Err(error) => {
                    unsafe {
                        set_error(
                            out_error,
                            GFStatus::Panic,
                            &format!("Route D report encoding failed: {error}"),
                        )
                    };
                    return GFStatus::Panic;
                }
            };
            unsafe {
                *out_result = GFRouteDPatchResult {
                    xml: owned_bytes(patched.xml),
                    report: owned_bytes(report),
                }
            };
            GFStatus::Ok
        }
        Ok(Err(error)) => {
            unsafe { set_error(out_error, GFStatus::InvalidArgument, &error.to_string()) };
            GFStatus::InvalidArgument
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "Route D FCPXML patching panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_route_d_batch_patch(
    input_bytes: *const u8,
    input_len: usize,
    processed_name_bytes: *const u8,
    processed_name_len: usize,
    out_result: *mut GFRouteDPatchResult,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if out_result.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "batch Route D patch output pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }
    unsafe { *out_result = GFRouteDPatchResult::default() };
    if input_len == 0 {
        unsafe {
            set_error(
                out_error,
                GFStatus::InvalidArgument,
                "batch Route D requires complete project FCPXML bytes",
            )
        };
        return GFStatus::InvalidArgument;
    }
    if input_bytes.is_null() || (processed_name_len > 0 && processed_name_bytes.is_null()) {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "batch Route D input or processed-name pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }
    let input = unsafe { std::slice::from_raw_parts(input_bytes, input_len) };
    let processed_name = if processed_name_len == 0 {
        None
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(processed_name_bytes, processed_name_len) };
        match std::str::from_utf8(bytes) {
            Ok(name) => Some(name),
            Err(error) => {
                unsafe {
                    set_error(
                        out_error,
                        GFStatus::InvalidArgument,
                        &format!("processed project name is not UTF-8: {error}"),
                    )
                };
                return GFStatus::InvalidArgument;
            }
        }
    };
    let patched = std::panic::catch_unwind(|| patch_fcpxml_project_batch(input, processed_name));
    match patched {
        Ok(Ok(patched)) => {
            let report = serde_json::json!({
                "original_project_name": patched.original_project_name,
                "processed_project_name": patched.processed_project_name,
                "import_token": patched.import_token,
                "occurrence_count": patched.occurrence_count,
                "inserted_count": patched.inserted_count,
                "updated_count": patched.updated_count,
                "skipped_count": patched.skipped_count,
                "failed_count": 0,
                "targets": patched.targets,
                "behavior": "Creates a validated new processed project and preserves the original project"
            });
            let report = match serde_json::to_vec(&report) {
                Ok(report) => report,
                Err(error) => {
                    unsafe {
                        set_error(
                            out_error,
                            GFStatus::Panic,
                            &format!("batch Route D report encoding failed: {error}"),
                        )
                    };
                    return GFStatus::Panic;
                }
            };
            unsafe {
                *out_result = GFRouteDPatchResult {
                    xml: owned_bytes(patched.xml),
                    report: owned_bytes(report),
                }
            };
            GFStatus::Ok
        }
        Ok(Err(error)) => {
            unsafe { set_error(out_error, GFStatus::InvalidArgument, &error.to_string()) };
            GFStatus::InvalidArgument
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "batch Route D FCPXML patching panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_route_d_patch_result_free(result: *mut GFRouteDPatchResult) {
    if result.is_null() {
        return;
    }
    let result = unsafe { &mut *result };
    unsafe {
        gf_finalcut_owned_bytes_free(&mut result.xml);
        gf_finalcut_owned_bytes_free(&mut result.report);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_project_payload_encode(
    project_bytes: *const u8,
    project_len: usize,
    out_payload: *mut GFOwnedBytes,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if out_payload.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "project payload output pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }
    unsafe { *out_payload = GFOwnedBytes::default() };
    if project_len == 0 {
        unsafe {
            set_error(
                out_error,
                GFStatus::InvalidProject,
                "gyroflow project bytes are empty",
            )
        };
        return GFStatus::InvalidProject;
    }
    if project_bytes.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "gyroflow project byte pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }
    let project_bytes = unsafe { std::slice::from_raw_parts(project_bytes, project_len) };
    match std::panic::catch_unwind(|| encode_project_payload(project_bytes)) {
        Ok(Ok(payload)) => {
            unsafe { *out_payload = owned_bytes(payload) };
            GFStatus::Ok
        }
        Ok(Err(message)) => {
            unsafe { set_error(out_error, GFStatus::InvalidProject, &message) };
            GFStatus::InvalidProject
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "project payload encoding panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_load_project_payload(
    instance: *mut GFFinalCutInstance,
    payload_bytes: *const u8,
    payload_len: usize,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "project payload load requires an instance",
            )
        };
        return GFStatus::NullPointer;
    }
    if payload_len == 0 {
        unsafe {
            set_error(
                out_error,
                GFStatus::InvalidProject,
                "project payload bytes are empty",
            )
        };
        return GFStatus::InvalidProject;
    }
    if payload_bytes.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "project payload byte pointer is null",
            )
        };
        return GFStatus::NullPointer;
    }

    let payload_bytes = unsafe { std::slice::from_raw_parts(payload_bytes, payload_len) };
    let loaded = std::panic::catch_unwind(|| {
        let project_bytes = decode_project_payload(payload_bytes)?;
        parse_project(&project_bytes).map_err(|message| PayloadError {
            status: GFStatus::InvalidProject,
            message,
        })
    });
    match loaded {
        Ok(Ok(project)) => {
            let instance = unsafe { &*instance };
            match instance.state.write() {
                Ok(mut state) => {
                    state.project = Some(project);
                    state.timing = None;
                    GFStatus::Ok
                }
                Err(_) => {
                    unsafe {
                        set_error(
                            out_error,
                            GFStatus::Panic,
                            "gyroflow project state lock is poisoned",
                        )
                    };
                    GFStatus::Panic
                }
            }
        }
        Ok(Err(error)) => {
            unsafe { set_error(out_error, error.status, &error.message) };
            error.status
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "project payload decoding panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_load_timing_payload(
    instance: *mut GFFinalCutInstance,
    payload_bytes: *const u8,
    payload_len: usize,
    observed_effect_bounds: *const GFTimeRange,
    observed_input_bounds: *const GFTimeRange,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null()
        || observed_effect_bounds.is_null()
        || observed_input_bounds.is_null()
        || (payload_len > 0 && payload_bytes.is_null())
    {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "timing payload load requires an instance, bounds, and payload bytes",
            )
        };
        return GFStatus::NullPointer;
    }
    let instance = unsafe { &*instance };
    {
        let mut state = match instance.state.write() {
            Ok(state) => state,
            Err(_) => {
                unsafe { set_error(out_error, GFStatus::Panic, "timing state lock is poisoned") };
                return GFStatus::Panic;
            }
        };
        state.timing = None;
        if state.project.is_none() {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::InvalidProject,
                    "timing payload load requires a loaded gyroflow project",
                )
            };
            return GFStatus::InvalidProject;
        }
    }
    let payload = if payload_len == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(payload_bytes, payload_len) }
    };
    let effect_bounds = unsafe { *observed_effect_bounds };
    let input_bounds = unsafe { *observed_input_bounds };
    let decoded =
        std::panic::catch_unwind(|| decode_timing_payload(payload, effect_bounds, input_bounds));
    match decoded {
        Ok(Ok(timing)) => {
            let mut state = match instance.state.write() {
                Ok(state) => state,
                Err(_) => {
                    unsafe {
                        set_error(out_error, GFStatus::Panic, "timing state lock is poisoned")
                    };
                    return GFStatus::Panic;
                }
            };
            if state.project.is_none() {
                unsafe {
                    set_error(
                        out_error,
                        GFStatus::InvalidProject,
                        "gyroflow project changed while timing payload was decoded",
                    )
                };
                return GFStatus::InvalidProject;
            }
            state.timing = Some(timing);
            GFStatus::Ok
        }
        Ok(Err(error)) => {
            unsafe { set_error(out_error, error.status, &error.message) };
            error.status
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "timing payload decoding panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_resolve_source_time(
    instance: *mut GFFinalCutInstance,
    effect_local_time: GFTime,
    out_source_time: *mut GFTime,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null() || out_source_time.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "source-time resolution requires an instance and output pointer",
            )
        };
        return GFStatus::NullPointer;
    }
    unsafe {
        *out_source_time = GFTime {
            numerator: 0,
            denominator: 1,
        }
    };
    let instance = unsafe { &*instance };
    let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let local_time = ratio_from_time(effect_local_time)
            .map_err(|message| (GFStatus::InvalidArgument, message))?;
        let state = instance
            .state
            .read()
            .map_err(|_| (GFStatus::Panic, "timing state lock is poisoned".to_string()))?;
        let timing = state.timing.as_ref().ok_or_else(|| {
            (
                GFStatus::MissingTiming,
                "Reprocess Project Required: no current timing payload".to_string(),
            )
        })?;
        let source_time = resolve_source_time(timing, local_time)
            .map_err(|message| (GFStatus::StaleTiming, message))?;
        time_from_ratio(&source_time).map_err(|message| (GFStatus::RenderFailed, message))
    }));
    match resolved {
        Ok(Ok(source_time)) => {
            unsafe { *out_source_time = source_time };
            GFStatus::Ok
        }
        Ok(Err((status, message))) => {
            unsafe { set_error(out_error, status, &message) };
            status
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "source-time resolution panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_render_metal(
    instance: *mut GFFinalCutInstance,
    request: *const GFMetalRenderRequest,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null() || request.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "Metal render requires an instance and request",
            )
        };
        return GFStatus::NullPointer;
    }
    let instance = unsafe { &*instance };
    let request = unsafe { *request };
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        validate_metal_render_request(&request)?;
        let (manager, timing) = {
            let state = instance.state.read().map_err(|_| PayloadError {
                status: GFStatus::Panic,
                message: "render state lock is poisoned".to_string(),
            })?;
            let project = state.project.as_ref().ok_or_else(|| PayloadError {
                status: GFStatus::InvalidProject,
                message: "Import Gyroflow Project before rendering".to_string(),
            })?;
            (Arc::clone(&project.manager), state.timing.clone())
        };
        let geometry_mode =
            validate_frame_geometry(&request.geometry).map_err(|message| PayloadError {
                status: GFStatus::InvalidArgument,
                message: message.to_string(),
            })?;
        let project_geometry = if geometry_mode == GFFrameGeometryMode::ValidatedAffine {
            project_geometry_from_manager(&manager).map_err(|error| PayloadError {
                status: error.status,
                message: error.message,
            })?
        } else {
            GFProjectGeometry {
                input_dimensions: GFDimensionsU32::default(),
                output_dimensions: GFDimensionsU32::default(),
                video_rotation: GF_ROTATION_NONE,
                reserved: 0,
            }
        };
        let mapped_geometry = map_frame_geometry(
            &request.geometry,
            GFDimensionsU32 {
                width: request.width,
                height: request.height,
            },
            project_geometry,
        )
        .map_err(|error| PayloadError {
            status: error.status,
            message: error.message,
        })?;
        let render_time =
            ratio_from_time(request.effect_local_time).map_err(|message| PayloadError {
                status: GFStatus::InvalidArgument,
                message,
            })?;
        let source_time =
            resolve_render_source_time(timing.as_ref(), render_time, request.effect_bounds)?;
        let timestamp_us =
            rational_seconds_to_microseconds(&source_time).map_err(|message| PayloadError {
                status: GFStatus::RenderFailed,
                message,
            })?;
        let mut buffers = Buffers {
            input: BufferDescription {
                size: (
                    mapped_geometry.input.dimensions.width as usize,
                    mapped_geometry.input.dimensions.height as usize,
                    request.input_row_bytes as usize,
                ),
                rect: mapped_geometry.input.rect,
                rotation: mapped_geometry.input.rotation,
                data: BufferSource::Metal {
                    texture: request.input_texture,
                    command_queue: request.command_queue,
                },
                ..Default::default()
            },
            output: BufferDescription {
                size: (
                    mapped_geometry.output.dimensions.width as usize,
                    mapped_geometry.output.dimensions.height as usize,
                    request.output_row_bytes as usize,
                ),
                rect: mapped_geometry.output.rect,
                rotation: mapped_geometry.output.rotation,
                post_affine: mapped_geometry.output.post_affine,
                data: BufferSource::Metal {
                    texture: request.output_texture,
                    command_queue: request.command_queue,
                },
                ..Default::default()
            },
        };
        manager
            .process_pixels::<RGBAf16>(timestamp_us, None, &mut buffers)
            .map_err(|error| PayloadError {
                status: GFStatus::RenderFailed,
                message: format!("Gyroflow Metal render failed: {error}"),
            })?;
        Ok::<(), PayloadError>(())
    }));
    match rendered {
        Ok(Ok(())) => GFStatus::Ok,
        Ok(Err(error)) => {
            unsafe { set_error(out_error, error.status, &error.message) };
            error.status
        }
        Err(_) => {
            unsafe { set_error(out_error, GFStatus::Panic, "Gyroflow Metal render panicked") };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_set_render_parameters(
    instance: *mut GFFinalCutInstance,
    parameters: *const GFRenderParameters,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null() || parameters.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "render parameter update requires an instance and parameters",
            )
        };
        return GFStatus::NullPointer;
    }
    let instance = unsafe { &*instance };
    let parameters = unsafe { &*parameters };
    let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = instance.state.read().map_err(|_| {
            (
                GFStatus::Panic,
                "project state lock is poisoned".to_string(),
            )
        })?;
        let project = state.project.as_ref().ok_or_else(|| {
            (
                GFStatus::InvalidProject,
                "render parameter update requires a loaded gyroflow project".to_string(),
            )
        })?;
        apply_render_parameters(project, parameters)
            .map_err(|message| (GFStatus::InvalidArgument, message))
    }));
    match applied {
        Ok(Ok(())) => GFStatus::Ok,
        Ok(Err((status, message))) => {
            unsafe { set_error(out_error, status, &message) };
            status
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "render parameter update panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_has_project(
    instance: *const GFFinalCutInstance,
) -> u8 {
    if instance.is_null() {
        return 0;
    }
    let instance = unsafe { &*instance };
    instance
        .state
        .read()
        .map(|state| u8::from(state.project.is_some()))
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_instance_get_project_geometry(
    instance: *const GFFinalCutInstance,
    out_geometry: *mut GFProjectGeometry,
    out_error: *mut *mut GFError,
) -> GFStatus {
    unsafe { clear_error_slot(out_error) };
    if instance.is_null() || out_geometry.is_null() {
        unsafe {
            set_error(
                out_error,
                GFStatus::NullPointer,
                "project geometry query requires an instance and output pointer",
            )
        };
        return GFStatus::NullPointer;
    }
    unsafe { *out_geometry = GFProjectGeometry::default() };
    let instance = unsafe { &*instance };
    let queried = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = instance.state.read().map_err(|_| GFGeometryMappingError {
            status: GFStatus::Panic,
            message: "project state lock is poisoned".to_string(),
        })?;
        let project = state
            .project
            .as_ref()
            .ok_or_else(|| GFGeometryMappingError {
                status: GFStatus::InvalidProject,
                message: "project geometry query requires a loaded gyroflow project".to_string(),
            })?;
        project_geometry_from_manager(&project.manager)
    }));
    match queried {
        Ok(Ok(geometry)) => {
            unsafe { *out_geometry = geometry };
            GFStatus::Ok
        }
        Ok(Err(error)) => {
            unsafe { set_error(out_error, error.status, &error.message) };
            error.status
        }
        Err(_) => {
            unsafe {
                set_error(
                    out_error,
                    GFStatus::Panic,
                    "project geometry query panicked",
                )
            };
            GFStatus::Panic
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_error_free(error: *mut GFError) {
    if error.is_null() {
        return;
    }
    let error = unsafe { Box::from_raw(error) };
    if !error.message.is_null() {
        unsafe { drop(CString::from_raw(error.message)) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gf_finalcut_owned_bytes_free(bytes: *mut GFOwnedBytes) {
    if bytes.is_null() {
        return;
    }
    let bytes = unsafe { &mut *bytes };
    if !bytes.data.is_null() {
        unsafe { drop(Vec::from_raw_parts(bytes.data, bytes.len, bytes.capacity)) };
    }
    *bytes = GFOwnedBytes::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_bytes_free_clears_the_caller_visible_buffer() {
        let mut source = vec![1_u8, 2, 3, 4];
        let mut bytes = GFOwnedBytes {
            data: source.as_mut_ptr(),
            len: source.len(),
            capacity: source.capacity(),
        };
        std::mem::forget(source);

        unsafe { gf_finalcut_owned_bytes_free(&mut bytes) };

        assert!(bytes.data.is_null());
        assert_eq!(bytes.len, 0);
        assert_eq!(bytes.capacity, 0);
    }

    #[test]
    fn instance_state_lock_is_initialized() {
        let instance = GFFinalCutInstance::default();
        assert!(instance.state.read().is_ok());
    }

    #[test]
    fn unsynchronized_inaccurate_timestamp_project_is_not_render_ready() {
        let project = serde_json::json!({
            "synchronization": { "max_sync_points": 2 },
            "videofile": "file:///P1004783.MOV"
        });

        let error = validate_project_sync_readiness(&project, false, false)
            .expect_err("unsynchronized project must fail closed");

        assert_eq!(
            error,
            "gyroflow project has no completed synchronization data; synchronize and save it in Gyroflow before importing"
        );
    }

    #[test]
    fn completed_sync_points_make_a_project_render_ready() {
        let project = serde_json::json!({
            "synchronization": { "max_sync_points": 2 },
            "videofile": "file:///P1004783.MOV"
        });

        validate_project_sync_readiness(&project, true, false)
            .expect("completed sync points are render ready");
    }

    #[test]
    fn accurate_timestamp_project_does_not_require_sync_points() {
        let project = serde_json::json!({
            "synchronization": { "max_sync_points": 2 },
            "videofile": "file:///GX010001.MP4"
        });

        validate_project_sync_readiness(&project, false, true)
            .expect("accurate timestamps are render ready without sync points");
    }

    #[test]
    fn render_parameters_map_to_core_units_and_zoom_sentinel() {
        let project = parse_project(include_bytes!("../tests/fixtures/phase0-valid.gyroflow"))
            .expect("fixture project");
        let parameters = GFRenderParameters {
            fov: 1.25,
            smoothness: 42.0,
            lens_correction: 80.0,
            horizon_lock_amount: 30.0,
            horizon_lock_roll: 5.0,
            zoom_mode: 2,
            overview: 1,
            reserved: [0; 3],
        };

        apply_render_parameters(&project, &parameters).expect("valid parameters");

        let core = project.manager.params.read();
        assert_eq!(core.fov, 1.25);
        assert_eq!(core.lens_correction_amount, 0.8);
        assert_eq!(core.adaptive_zoom_window, -1.0);
        assert!(core.fov_overview);
        assert!(core.framebuffer_inverted);
        drop(core);
        let smoothing = project.manager.smoothing.read();
        assert_eq!(smoothing.current().get_parameter("smoothness"), 0.42);
        assert_eq!(smoothing.horizon_lock.horizonlockpercent, 30.0);
        assert_eq!(smoothing.horizon_lock.horizonroll, 5.0);
        drop(smoothing);
        assert!(
            !project
                .manager
                .smoothing_invalidated
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(
            !project
                .manager
                .zooming_invalidated
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(
            !project
                .manager
                .undistortion_invalidated
                .load(std::sync::atomic::Ordering::SeqCst)
        );
    }

    #[test]
    fn no_dynamic_and_static_zoom_remain_independent_from_live_fcp_affine() {
        let project = parse_project(
            br#"{
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
            }"#,
        )
        .expect("project geometry fixture");
        let project_geometry = project_geometry_from_manager(&project.manager).unwrap();
        let mut live_geometry = GFFrameGeometry {
            version: GF_FRAME_GEOMETRY_VERSION,
            struct_size: std::mem::size_of::<GFFrameGeometry>() as u32,
            validity: GF_GEOMETRY_VALIDITY_VALID,
            support: GF_GEOMETRY_SUPPORT_AFFINE_2D,
            source_dimensions: project_geometry.input_dimensions,
            oriented_dimensions: project_geometry.output_dimensions,
            tile_dimensions: GFDimensionsU32 {
                width: 1920,
                height: 1080,
            },
            output_dimensions: GFDimensionsU32 {
                width: 1920,
                height: 1080,
            },
            source_rect: GFRectI32 {
                left: 0,
                bottom: 0,
                right: 1920,
                top: 1080,
            },
            destination_rect: GFRectI32 {
                left: 0,
                bottom: 0,
                right: 1920,
                top: 1080,
            },
            source_origin: GF_IMAGE_ORIGIN_BOTTOM_LEFT,
            destination_origin: GF_IMAGE_ORIGIN_BOTTOM_LEFT,
            input_rotation: GF_ROTATION_NONE,
            video_rotation: GF_ROTATION_NONE,
            forward_transform: GFAffineTransform {
                values: [1.0, 0.0, 96.0, 0.0, 1.0, -54.0, 0.0, 0.0, 1.0],
            },
            inverse_transform: GFAffineTransform {
                values: [1.0, 0.0, -96.0, 0.0, 1.0, 54.0, 0.0, 0.0, 1.0],
            },
            reserved: [0; 4],
        };
        let cases = [(0, 0.0), (1, 4.0), (2, -1.0)];

        for (zoom_mode, expected_window) in cases {
            let parameters = GFRenderParameters {
                fov: 1.0 + f64::from(zoom_mode) * 0.1,
                smoothness: 50.0,
                lens_correction: 100.0,
                horizon_lock_amount: 0.0,
                horizon_lock_roll: 0.0,
                zoom_mode,
                overview: 0,
                reserved: [0; 3],
            };
            apply_render_parameters(&project, &parameters).expect("zoom mode");
            live_geometry.forward_transform.values[2] = 96.0 * f64::from(zoom_mode + 1);
            live_geometry.inverse_transform.values[2] = -96.0 * f64::from(zoom_mode + 1);
            map_frame_geometry(
                &live_geometry,
                live_geometry.tile_dimensions,
                project_geometry,
            )
            .expect("live Final Cut affine");

            let core = project.manager.params.read();
            assert_eq!(core.adaptive_zoom_window, expected_window);
            assert_eq!(core.fov, parameters.fov);
        }
    }

    fn valid_metal_request() -> GFMetalRenderRequest {
        GFMetalRenderRequest {
            input_texture: std::ptr::dangling_mut::<u8>().cast(),
            output_texture: std::ptr::dangling_mut::<u8>().cast(),
            command_queue: std::ptr::null_mut(),
            device_registry_id: 42,
            width: 1920,
            height: 1080,
            input_row_bytes: 1920 * 8,
            output_row_bytes: 1920 * 8,
            pixel_format: 115,
            effect_local_time: GFTime {
                numerator: 1,
                denominator: 24,
            },
            effect_bounds: GFTimeRange {
                start: GFTime {
                    numerator: 0,
                    denominator: 1,
                },
                duration: GFTime {
                    numerator: 1,
                    denominator: 1,
                },
            },
            input_bounds: GFTimeRange {
                start: GFTime {
                    numerator: 0,
                    denominator: 1,
                },
                duration: GFTime {
                    numerator: 1,
                    denominator: 1,
                },
            },
            geometry: GFFrameGeometry::default(),
        }
    }

    #[test]
    fn direct_render_time_subtracts_nonzero_effect_start_exactly() {
        let bounds = GFTimeRange {
            start: GFTime {
                numerator: 3600,
                denominator: 1,
            },
            duration: GFTime {
                numerator: 30,
                denominator: 1,
            },
        };
        let resolved = resolve_render_source_time(None, Ratio::new(108_041, 30), bounds)
            .expect("ordinary direct time");

        assert_eq!(resolved, Ratio::new(41, 30));
    }

    #[test]
    fn stale_timing_error_reports_expected_and_observed_bounds() {
        let envelope = serde_json::json!({
            "version": 1,
            "fcpxml_version": "1.14",
            "structure_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "asset_ref": "r4",
            "mapping": [
                {"local": "0/1", "source": "0/1"},
                {"local": "13013/2000", "source": "13013/2000"}
            ],
            "effect_bounds": {"start": "0/1", "duration": "13013/2000"},
            "input_bounds": {"start": "0/1", "duration": "13013/2000"},
            "render_ready_snapshot": true
        });
        let payload = STANDARD.encode(serde_json::to_vec(&envelope).unwrap());
        let observed_effect = GFTimeRange {
            start: GFTime {
                numerator: 1_171_170,
                denominator: 180_000,
            },
            duration: GFTime {
                numerator: 1_171_170,
                denominator: 180_000,
            },
        };
        let observed_input = GFTimeRange {
            start: GFTime {
                numerator: 9_392_080_698,
                denominator: 180_000,
            },
            duration: GFTime {
                numerator: 1_171_170,
                denominator: 180_000,
            },
        };

        let error = match decode_timing_payload(payload.as_bytes(), observed_effect, observed_input)
        {
            Ok(_) => panic!("nonzero host bounds must be diagnosed as stale"),
            Err(error) => error,
        };

        assert_eq!(error.status, GFStatus::StaleTiming);
        assert_eq!(
            error.message,
            "Reprocess Project Required: observed effect/input bounds changed; expected effect start=0/1 duration=13013/2000, input start=0/1 duration=13013/2000; observed effect start=13013/2000 duration=13013/2000, input start=521782261/10000 duration=13013/2000"
        );
    }

    #[test]
    fn route_d_mapping_subtracts_nonzero_effect_start_exactly() {
        let timing = LoadedTiming {
            mapping: vec![
                (Ratio::from_integer(0), Ratio::from_integer(5)),
                (Ratio::from_integer(10), Ratio::from_integer(25)),
            ],
        };
        let bounds = GFTimeRange {
            start: GFTime {
                numerator: 100,
                denominator: 1,
            },
            duration: GFTime {
                numerator: 10,
                denominator: 1,
            },
        };
        let resolved = resolve_render_source_time(Some(&timing), Ratio::from_integer(104), bounds)
            .expect("Route D source time");

        assert_eq!(resolved, Ratio::from_integer(13));
    }

    #[test]
    fn direct_render_time_rejects_invalid_or_out_of_range_bounds() {
        let invalid = GFTimeRange {
            start: GFTime {
                numerator: 0,
                denominator: 0,
            },
            duration: GFTime {
                numerator: 10,
                denominator: 1,
            },
        };
        assert_eq!(
            resolve_render_source_time(None, Ratio::from_integer(1), invalid)
                .unwrap_err()
                .status,
            GFStatus::InvalidArgument
        );

        let bounds = GFTimeRange {
            start: GFTime {
                numerator: 10,
                denominator: 1,
            },
            duration: GFTime {
                numerator: 5,
                denominator: 1,
            },
        };
        assert_eq!(
            resolve_render_source_time(None, Ratio::from_integer(9), bounds)
                .unwrap_err()
                .status,
            GFStatus::InvalidArgument
        );
    }

    #[test]
    fn metal_request_accepts_rgba16float_and_a_null_command_queue() {
        validate_metal_render_request(&valid_metal_request()).expect("valid Final Cut request");
    }

    #[test]
    fn metal_request_rejects_formats_other_than_rgba16float() {
        let mut request = valid_metal_request();
        request.pixel_format = 125;

        let error = validate_metal_render_request(&request).unwrap_err();
        assert_eq!(error.status, GFStatus::UnsupportedPixelFormat);
    }

    #[test]
    fn rational_time_is_rounded_only_at_microsecond_boundary() {
        assert_eq!(
            rational_seconds_to_microseconds(&Ratio::new(1, 24)).unwrap(),
            41_667
        );
        assert_eq!(
            rational_seconds_to_microseconds(&Ratio::new(-1, 24)).unwrap(),
            -41_667
        );
        assert_eq!(
            rational_seconds_to_microseconds(&Ratio::new(1, 2_000_000)).unwrap(),
            1
        );
    }

    #[test]
    fn project_payload_compresses_and_round_trips_large_project_bytes() {
        let project = format!(
            "{{\"version\":3,\"gyro_source\":{{}},\"capacity_probe\":\"{}\"}}",
            "0123456789abcdef".repeat(64 * 1024)
        )
        .into_bytes();

        let payload = encode_project_payload(&project).expect("encode");
        let decoded = decode_project_payload(&payload).expect("decode");

        assert_eq!(decoded, project);
        assert!(
            payload.len() < project.len() / 4,
            "repetitive capacity fixture should compress materially"
        );
    }

    #[test]
    fn instance_state_and_timing_resolution_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GFFinalCutInstance>();

        let instance = Arc::new(GFFinalCutInstance::default());
        instance.state.write().unwrap().timing = Some(LoadedTiming {
            mapping: vec![
                (Ratio::from_integer(0), Ratio::from_integer(10)),
                (Ratio::from_integer(10), Ratio::from_integer(20)),
            ],
        });
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let instance = Arc::clone(&instance);
                std::thread::spawn(move || {
                    for frame in 0..1000 {
                        let state = instance.state.read().unwrap();
                        let timing = state.timing.as_ref().unwrap();
                        let resolved =
                            resolve_source_time(timing, Ratio::new(frame % 1000, 100)).unwrap();
                        assert!(resolved >= Ratio::from_integer(10));
                        assert!(resolved <= Ratio::from_integer(20));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("concurrent resolver");
        }
    }
}
