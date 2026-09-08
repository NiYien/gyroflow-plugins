use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use num_bigint::BigInt;
use num_rational::Ratio;
use num_traits::{Signed, ToPrimitive, Zero};
use roxmltree::{Attribute, Document, Node, ParsingOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const EFFECT_UUID: &str = "ABAD71F5-23F5-46F6-AB08-C11603168AA4";
const EFFECT_TEMPLATE_UID: &str = "~/Effects.localized/NiYien/Gyroflow/Gyroflow NiYien.moef";
const TIMING_PAYLOAD_VERSION: u32 = 1;
const PROJECT_BANK_MANIFEST_VERSION: u32 = 1;
const PROJECT_BANK_CHUNK_BYTES: usize = 416 * 1024;
const PROJECT_BANK_CHUNKS: usize = 10;
const PROJECT_BANK_MAXIMUM_BYTES: usize = 4 * 1024 * 1024;
const RAW_PROJECT_TOO_LARGE: &str = "raw project exceeds the 256 MiB limit";
const PROJECT_PARAMETER_KEY_PREFIX: &str = "9999/10013/10016/3/10036";
const VISIBLE_PARAMETER_IDS: [(&str, u32); 7] = [
    ("FOV", 2001),
    ("Smoothness", 2002),
    ("Lens Correction", 2003),
    ("Horizon Lock", 2004),
    ("Horizon Roll", 2005),
    ("Zoom Mode", 2006),
    ("Stabilization Overview", 2007),
];
const SUPPORTED_FCPXML_VERSIONS: &[&str] = &["1.12", "1.13", "1.14"];
// Final Cut 12.3 normalizes evaluated retime values to this internal timescale
// before applying the timeMap frame-sampling policy.
const FCP_INTERNAL_TIMESCALE: i128 = 720_000;

type Rational = Ratio<i128>;

#[derive(Debug)]
pub struct RouteDError {
    message: String,
    category: RouteDErrorCategory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteDErrorCategory {
    InvalidInput,
    UnsafeStructure,
    NoUpdateableTargets,
}

impl RouteDError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            category: RouteDErrorCategory::InvalidInput,
        }
    }

    fn unsafe_structure(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            category: RouteDErrorCategory::UnsafeStructure,
        }
    }

    fn no_updateable_targets(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            category: RouteDErrorCategory::NoUpdateableTargets,
        }
    }

    pub fn category(&self) -> RouteDErrorCategory {
        self.category
    }
}

impl fmt::Display for RouteDError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RouteDError {}

#[derive(Debug)]
pub struct RouteDPatchResult {
    pub xml: Vec<u8>,
    pub original_project_name: String,
    pub processed_project_name: String,
    pub import_token: String,
    pub occurrence_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchTargetAction {
    UpdatedProject,
    TimingOnly,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchSkipReason {
    MissingProject,
    PermissionDenied,
    InvalidProject,
    IncompatibleProject,
    PayloadTooLarge,
    AmbiguousMedia,
    UnsupportedStructure,
    InvalidTiming,
    BlockedGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BatchTargetReport {
    pub occurrence: usize,
    pub clip_name: String,
    pub asset_ref: Option<String>,
    pub media_url: Option<String>,
    pub expected_project_path: Option<String>,
    pub project_display_name: Option<String>,
    pub action: BatchTargetAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<BatchSkipReason>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry_status: Option<GeometryStatus>,
    pub geometry_reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry_detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryStatus {
    RuntimeLive,
    Blocked,
}

struct GeometryPreflight {
    status: GeometryStatus,
    reasons: Vec<String>,
    detail: String,
}

#[derive(Debug)]
pub struct BatchRouteDPatchResult {
    pub xml: Vec<u8>,
    pub original_project_name: String,
    pub occurrence_count: usize,
    pub updated_project_count: usize,
    pub timing_only_count: usize,
    pub skipped_count: usize,
    pub targets: Vec<BatchTargetReport>,
}

#[derive(Clone, Serialize)]
struct MappingPoint {
    local: String,
    source: String,
}

#[derive(Clone, Serialize)]
struct Bounds {
    start: String,
    duration: String,
}

#[derive(Serialize)]
struct TimingPayload {
    version: u32,
    fcpxml_version: String,
    occurrence: usize,
    structure_sha256: String,
    asset_ref: String,
    mapping: Vec<MappingPoint>,
    effect_bounds: Bounds,
    input_bounds: Bounds,
    render_ready_snapshot: bool,
}

#[derive(Serialize)]
struct StructureFingerprint {
    fcpxml_version: String,
    clip_tag: String,
    asset_ref: String,
    asset_start: String,
    clip_start: String,
    clip_offset: String,
    clip_duration: String,
    mapping: Vec<MappingPoint>,
    compound_ref: Option<CompoundFingerprint>,
}

#[derive(Serialize)]
struct CompoundFingerprint {
    media_id: String,
    offset: String,
    start: String,
    duration: String,
}

struct ResolvedClip {
    asset_ref: String,
    mapping: Vec<MappingPoint>,
    bounds_start: Rational,
    duration: Rational,
    structure_sha256: String,
}

struct Replacement {
    range: Range<usize>,
    value: String,
}

struct TargetPatchPlan {
    replacements: Vec<Replacement>,
    report: BatchTargetReport,
}

#[derive(Deserialize)]
struct ProjectBankManifest {
    version: u32,
    generation: u64,
    chunk_count: usize,
    encoded_length: usize,
    payload_sha256: String,
}

struct ProjectBankCandidate {
    generation: u64,
    payload: String,
    key_prefix: String,
}

#[derive(Clone)]
struct CurvePoint {
    time: Rational,
    value: Rational,
}

enum Smooth2Segment {
    Linear {
        start: CurvePoint,
        end: CurvePoint,
    },
    Bezier {
        start: CurvePoint,
        control: CurvePoint,
        end: CurvePoint,
    },
}

fn parse_time(value: &str) -> Result<Rational, RouteDError> {
    let value = value
        .strip_suffix('s')
        .ok_or_else(|| RouteDError::new(format!("FCPXML time lacks s suffix: {value}")))?;
    let (numerator, denominator) = match value.split_once('/') {
        Some((numerator, denominator)) => (numerator, denominator),
        None => (value, "1"),
    };
    let numerator = numerator
        .parse::<i128>()
        .map_err(|_| RouteDError::new(format!("invalid FCPXML time numerator: {value}")))?;
    let denominator = denominator
        .parse::<i128>()
        .map_err(|_| RouteDError::new(format!("invalid FCPXML time denominator: {value}")))?;
    if denominator == 0 {
        return Err(RouteDError::new("FCPXML time denominator is zero"));
    }
    Ok(Ratio::new(numerator, denominator))
}

fn rational_string(value: &Rational) -> String {
    format!("{}/{}", value.numer(), value.denom())
}

fn required_time(node: Node<'_, '_>, name: &str) -> Result<Rational, RouteDError> {
    let value = node.attribute(name).ok_or_else(|| {
        RouteDError::new(format!(
            "{} occurrence is missing {name}",
            node.tag_name().name()
        ))
    })?;
    parse_time(value)
}

fn optional_time(node: Node<'_, '_>, name: &str) -> Result<Rational, RouteDError> {
    node.attribute(name)
        .map(parse_time)
        .unwrap_or_else(|| Ok(Ratio::from_integer(0)))
}

fn resource_by_id<'a>(
    document: &'a Document<'a>,
    tag: &str,
    id: &str,
) -> Result<Node<'a, 'a>, RouteDError> {
    let matches: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name(tag) && node.attribute("id") == Some(id))
        .collect();
    if matches.len() != 1 {
        return Err(RouteDError::unsafe_structure(format!(
            "expected one {tag} resource for {id}, found {}",
            matches.len()
        )));
    }
    Ok(matches[0])
}

fn frame_duration_for_format(
    document: &Document<'_>,
    format_id: &str,
    context: &str,
) -> Result<Rational, RouteDError> {
    let format = resource_by_id(document, "format", format_id)?;
    let frame_duration = required_time(format, "frameDuration")?;
    if frame_duration <= Ratio::from_integer(0) {
        return Err(RouteDError::unsafe_structure(format!(
            "{context} frameDuration must be positive"
        )));
    }
    Ok(frame_duration)
}

fn canonical_frame_rate_label(frame_duration: &Rational) -> Option<&'static str> {
    [
        (1001, 24000, "23.98"),
        (1, 24, "24"),
        (1, 25, "25"),
        (1001, 30000, "29.97"),
        (1, 30, "30"),
        (1, 48, "48"),
        (1, 50, "50"),
        (1001, 60000, "59.94"),
        (1, 60, "60"),
        (1001, 120000, "119.88"),
        (1, 120, "120"),
    ]
    .into_iter()
    .find_map(|(numerator, denominator, label)| {
        (frame_duration == &Ratio::new(numerator, denominator)).then_some(label)
    })
}

fn validate_noop_conform_rate(
    document: &Document<'_>,
    clip: Node<'_, '_>,
    asset: Node<'_, '_>,
) -> Result<(), RouteDError> {
    let conform_rates: Vec<_> = clip
        .children()
        .filter(|node| node.has_tag_name("conform-rate"))
        .collect();
    if conform_rates.is_empty() {
        return Ok(());
    }
    if conform_rates.len() != 1 {
        return Err(RouteDError::new(
            "asset-clip must contain at most one conform-rate",
        ));
    }
    let conform_rate = conform_rates[0];
    if conform_rate
        .attributes()
        .any(|attribute| attribute.name() != "srcFrameRate")
    {
        return Err(RouteDError::new(
            "conform-rate with additional behavior is unsupported",
        ));
    }
    let source_format = asset
        .attribute("format")
        .ok_or_else(|| RouteDError::new("conformed asset has no format"))?;
    let sequence = clip
        .ancestors()
        .find(|node| node.has_tag_name("sequence"))
        .ok_or_else(|| RouteDError::new("conformed asset-clip has no sequence"))?;
    let output_format = sequence
        .attribute("format")
        .ok_or_else(|| RouteDError::new("conformed sequence has no format"))?;
    let source_frame_duration =
        frame_duration_for_format(document, source_format, "conform-rate source")?;
    let output_frame_duration =
        frame_duration_for_format(document, output_format, "conform-rate output")?;
    if source_frame_duration != output_frame_duration {
        return Err(RouteDError::new(
            "conform-rate changes frame duration and requires an explicit verified expansion",
        ));
    }
    let declared_rate = conform_rate
        .attribute("srcFrameRate")
        .ok_or_else(|| RouteDError::new("conform-rate is missing srcFrameRate"))?;
    let expected_rate = canonical_frame_rate_label(&source_frame_duration).ok_or_else(|| {
        RouteDError::new("conform-rate uses an unsupported exact source frame duration")
    })?;
    if declared_rate != expected_rate {
        return Err(RouteDError::new(
            "conform-rate srcFrameRate does not match the exact source frame duration",
        ));
    }
    Ok(())
}

fn rational_to_big(value: &Rational) -> Ratio<BigInt> {
    Ratio::new(BigInt::from(*value.numer()), BigInt::from(*value.denom()))
}

fn bezier_value(
    start: &Ratio<BigInt>,
    control: &Ratio<BigInt>,
    end: &Ratio<BigInt>,
    parameter: &Ratio<BigInt>,
) -> Ratio<BigInt> {
    let one = Ratio::from_integer(BigInt::from(1));
    let inverse = one - parameter;
    let three = Ratio::from_integer(BigInt::from(3));
    inverse.clone().pow(3) * start
        + three.clone() * inverse.clone().pow(2) * parameter * control
        + three * inverse * parameter.clone().pow(2) * control
        + parameter.clone().pow(3) * end
}

fn round_nonnegative_ratio(value: &Ratio<BigInt>) -> Result<BigInt, RouteDError> {
    if value.is_negative() {
        return Err(RouteDError::new(
            "smooth2 mapping produced a negative absolute media time",
        ));
    }
    let numerator = value.numer();
    let denominator = value.denom();
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder.clone() * 2 >= *denominator {
        Ok(quotient + 1)
    } else {
        Ok(quotient)
    }
}

fn sampled_source_frame(
    absolute_source: &Ratio<BigInt>,
    asset_start: &Ratio<BigInt>,
    source_frame_duration: &Ratio<BigInt>,
) -> Result<BigInt, RouteDError> {
    let internal_timescale = Ratio::from_integer(BigInt::from(FCP_INTERNAL_TIMESCALE));
    let rounded_tick = round_nonnegative_ratio(&(absolute_source * &internal_timescale))?;
    let rounded_source = Ratio::new(rounded_tick, BigInt::from(FCP_INTERNAL_TIMESCALE));
    let local_source = rounded_source - asset_start;
    if local_source.is_negative() {
        return Err(RouteDError::new(
            "smooth2 mapping resolves before the asset start",
        ));
    }
    let frame_position = local_source / source_frame_duration;
    Ok(frame_position.numer() / frame_position.denom())
}

fn sampled_bezier_source_frame(
    local_time: &Rational,
    start: &CurvePoint,
    control: &CurvePoint,
    end: &CurvePoint,
    asset_start: &Ratio<BigInt>,
    source_frame_duration: &Ratio<BigInt>,
) -> Result<BigInt, RouteDError> {
    let target = rational_to_big(local_time);
    let start_time = rational_to_big(&start.time);
    let control_time = rational_to_big(&control.time);
    let end_time = rational_to_big(&end.time);
    let start_value = rational_to_big(&start.value);
    let control_value = rational_to_big(&control.value);
    let end_value = rational_to_big(&end.value);
    if target == start_time {
        return sampled_source_frame(&start_value, asset_start, source_frame_duration);
    }
    if target == end_time {
        return sampled_source_frame(&end_value, asset_start, source_frame_duration);
    }

    let zero = Ratio::from_integer(BigInt::zero());
    let one = Ratio::from_integer(BigInt::from(1));
    let two = Ratio::from_integer(BigInt::from(2));
    let mut lower = zero;
    let mut upper = one;
    // Keep a rational parameter interval until both curve bounds provably select
    // the same source frame. This avoids floating-point frame accumulation.
    for _ in 0..192 {
        let parameter = (&lower + &upper) / &two;
        let mapped_time = bezier_value(&start_time, &control_time, &end_time, &parameter);
        if mapped_time == target {
            let value = bezier_value(&start_value, &control_value, &end_value, &parameter);
            return sampled_source_frame(&value, asset_start, source_frame_duration);
        }
        if mapped_time < target {
            lower = parameter;
        } else {
            upper = parameter;
        }

        let lower_value = bezier_value(&start_value, &control_value, &end_value, &lower);
        let upper_value = bezier_value(&start_value, &control_value, &end_value, &upper);
        let lower_frame = sampled_source_frame(&lower_value, asset_start, source_frame_duration)?;
        let upper_frame = sampled_source_frame(&upper_value, asset_start, source_frame_duration)?;
        if lower_frame == upper_frame {
            return Ok(lower_frame);
        }
    }
    Err(RouteDError::new(
        "smooth2 source-frame expansion did not converge to a provable sample",
    ))
}

fn sampled_segment_source_frame(
    local_time: &Rational,
    segment: &Smooth2Segment,
    asset_start: &Ratio<BigInt>,
    source_frame_duration: &Ratio<BigInt>,
) -> Result<BigInt, RouteDError> {
    match segment {
        Smooth2Segment::Linear { start, end } => {
            let position = (local_time - &start.time) / (&end.time - &start.time);
            let absolute_source = rational_to_big(&start.value)
                + rational_to_big(&position)
                    * (rational_to_big(&end.value) - rational_to_big(&start.value));
            sampled_source_frame(&absolute_source, asset_start, source_frame_duration)
        }
        Smooth2Segment::Bezier {
            start,
            control,
            end,
        } => sampled_bezier_source_frame(
            local_time,
            start,
            control,
            end,
            asset_start,
            source_frame_duration,
        ),
    }
}

fn smooth2_mapping(
    document: &Document<'_>,
    filter: Node<'_, '_>,
    asset: Node<'_, '_>,
    time_map: Node<'_, '_>,
    time_points: &[Node<'_, '_>],
    clip_start: &Rational,
    asset_start: &Rational,
    clip_duration: &Rational,
    fcpxml_version: &str,
) -> Result<Vec<MappingPoint>, RouteDError> {
    if fcpxml_version != "1.14" {
        return Err(RouteDError::new(
            "smooth2 per-frame expansion is verified only for FCPXML 1.14",
        ));
    }
    if !matches!(time_map.attribute("frameSampling"), None | Some("floor")) {
        return Err(RouteDError::new(
            "smooth2 expansion currently requires floor frameSampling",
        ));
    }

    let output_sequence = filter
        .ancestors()
        .find(|node| node.has_tag_name("sequence"))
        .ok_or_else(|| RouteDError::new("smooth2 occurrence has no sequence ancestor"))?;
    let output_format_id = output_sequence
        .attribute("format")
        .ok_or_else(|| RouteDError::new("smooth2 occurrence sequence has no format reference"))?;
    let output_frame_duration = frame_duration_for_format(document, output_format_id, "output")?;
    let asset_format_id = asset
        .attribute("format")
        .ok_or_else(|| RouteDError::new("smooth2 asset has no format reference"))?;
    let source_frame_duration = frame_duration_for_format(document, asset_format_id, "source")?;

    let mut points = Vec::with_capacity(time_points.len());
    let mut handles = Vec::with_capacity(time_points.len());
    for point in time_points {
        let local_time = required_time(*point, "time")? - clip_start.clone();
        let value = required_time(*point, "value")?;
        let incoming = optional_time(*point, "inTime")?;
        let outgoing = optional_time(*point, "outTime")?;
        if incoming < Ratio::from_integer(0) || outgoing < Ratio::from_integer(0) {
            return Err(RouteDError::new(
                "smooth2 transition times must be nonnegative",
            ));
        }
        if incoming > Ratio::from_integer(1) || outgoing > Ratio::from_integer(1) {
            return Err(RouteDError::new(
                "smooth2 transition times longer than one second are not verified",
            ));
        }
        points.push(CurvePoint {
            time: local_time,
            value,
        });
        handles.push((incoming, outgoing));
    }
    if points[0].time != Ratio::from_integer(0) || points[points.len() - 1].time < *clip_duration {
        return Err(RouteDError::new(
            "timeMap endpoints do not cover the full effect-local duration",
        ));
    }
    if points
        .windows(2)
        .any(|window| window[0].time >= window[1].time)
    {
        return Err(RouteDError::new(
            "timeMap local control points must be strictly increasing",
        ));
    }
    let source_differences: Vec<_> = points
        .windows(2)
        .map(|window| &window[1].value - &window[0].value)
        .collect();
    let nondecreasing = source_differences
        .iter()
        .all(|difference| *difference >= Ratio::from_integer(0));
    let nonincreasing = source_differences
        .iter()
        .all(|difference| *difference <= Ratio::from_integer(0));
    if !nondecreasing && !nonincreasing {
        return Err(RouteDError::new(
            "smooth2 direction changes require a separately verified expansion",
        ));
    }

    let mut segments = Vec::new();
    let mut cursor = points[0].clone();
    for index in 1..points.len() - 1 {
        let previous = &points[index - 1];
        let key = &points[index];
        let next = &points[index + 1];
        let incoming = &handles[index].0;
        let outgoing = &handles[index].1;
        let previous_slope = (&key.value - &previous.value) / (&key.time - &previous.time);
        let next_slope = (&next.value - &key.value) / (&next.time - &key.time);
        // Final Cut smooth2 uses a parametric cubic Bezier whose two control
        // points are the original time point. The endpoints lie on the adjacent
        // linear-speed segments at the declared transition times.
        let start = CurvePoint {
            time: &key.time - incoming,
            value: &key.value - &previous_slope * incoming,
        };
        let end = CurvePoint {
            time: &key.time + outgoing,
            value: &key.value + &next_slope * outgoing,
        };
        if start.time < cursor.time || end.time > next.time {
            return Err(RouteDError::new(
                "smooth2 transition windows overlap or exceed their adjacent segments",
            ));
        }
        if start.time > cursor.time {
            segments.push(Smooth2Segment::Linear {
                start: cursor.clone(),
                end: start.clone(),
            });
        } else if start.value != cursor.value {
            return Err(RouteDError::new(
                "smooth2 transition window has an ambiguous boundary value",
            ));
        }
        if end.time > start.time {
            segments.push(Smooth2Segment::Bezier {
                start,
                control: key.clone(),
                end: end.clone(),
            });
        }
        cursor = end;
    }
    let last = points[points.len() - 1].clone();
    if cursor.time < last.time {
        segments.push(Smooth2Segment::Linear {
            start: cursor,
            end: last,
        });
    }
    if segments.is_empty() {
        return Err(RouteDError::new("smooth2 expansion produced no segments"));
    }

    let frame_count = clip_duration / &output_frame_duration;
    if *frame_count.denom() != 1 {
        return Err(RouteDError::new(
            "smooth2 clip duration is not an exact output-frame count",
        ));
    }
    let frame_count = frame_count
        .numer()
        .to_usize()
        .ok_or_else(|| RouteDError::new("smooth2 output-frame count exceeds capacity"))?;
    let big_asset_start = rational_to_big(asset_start);
    let big_source_frame_duration = rational_to_big(&source_frame_duration);
    let mut mapping = Vec::with_capacity(frame_count + 1);
    let mut segment_index = 0usize;
    for frame in 0..=frame_count {
        let local_time = &output_frame_duration * Ratio::from_integer(frame as i128);
        while segment_index + 1 < segments.len() {
            let end_time = match &segments[segment_index] {
                Smooth2Segment::Linear { end, .. } | Smooth2Segment::Bezier { end, .. } => {
                    &end.time
                }
            };
            if local_time <= *end_time {
                break;
            }
            segment_index += 1;
        }
        let source_frame = sampled_segment_source_frame(
            &local_time,
            &segments[segment_index],
            &big_asset_start,
            &big_source_frame_duration,
        )?;
        let source = &big_source_frame_duration * Ratio::from_integer(source_frame);
        let source_numerator = source
            .numer()
            .to_i128()
            .ok_or_else(|| RouteDError::new("smooth2 source numerator exceeds i128"))?;
        let source_denominator = source
            .denom()
            .to_i128()
            .ok_or_else(|| RouteDError::new("smooth2 source denominator exceeds i128"))?;
        mapping.push(MappingPoint {
            local: rational_string(&local_time),
            source: rational_string(&Ratio::new(source_numerator, source_denominator)),
        });
    }
    Ok(mapping)
}

fn compound_context(
    project: Node<'_, '_>,
    filter: Node<'_, '_>,
) -> Result<Option<CompoundFingerprint>, RouteDError> {
    let media = filter
        .ancestors()
        .find(|node| node.has_tag_name("media") && node.attribute("id").is_some());
    let Some(media) = media else {
        if filter.ancestors().any(|node| node == project) {
            return Ok(None);
        }
        return Err(RouteDError::new(
            "NiYien effect occurrence is not reachable from the project",
        ));
    };
    let media_id = media.attribute("id").unwrap();
    let references: Vec<_> = project
        .descendants()
        .filter(|node| node.has_tag_name("ref-clip") && node.attribute("ref") == Some(media_id))
        .collect();
    if references.len() != 1 {
        return Err(RouteDError::new(format!(
            "compound media {media_id} has {} project references; occurrence binding is ambiguous",
            references.len()
        )));
    }
    let reference = references[0];
    if reference
        .descendants()
        .any(|node| node.has_tag_name("timeMap") || node.has_tag_name("conform-rate"))
    {
        return Err(RouteDError::new(
            "retimed or conformed Compound Clip requires a separately expanded snapshot",
        ));
    }
    let inner_sequence = media
        .children()
        .find(|node| node.has_tag_name("sequence"))
        .ok_or_else(|| RouteDError::new(format!("compound media {media_id} has no sequence")))?;
    let inner_duration = required_time(inner_sequence, "duration")?;
    let reference_duration = required_time(reference, "duration")?;
    if reference_duration != inner_duration {
        return Err(RouteDError::new(
            "trimmed Compound Clip reference requires reprocessing after expansion",
        ));
    }
    Ok(Some(CompoundFingerprint {
        media_id: media_id.to_string(),
        offset: rational_string(&optional_time(reference, "offset")?),
        start: rational_string(&optional_time(reference, "start")?),
        duration: rational_string(&reference_duration),
    }))
}

fn effect_clip_container<'a>(filter: Node<'a, 'a>) -> Result<Node<'a, 'a>, RouteDError> {
    filter
        .ancestors()
        .find(|node| matches!(node.tag_name().name(), "asset-clip" | "clip"))
        .ok_or_else(|| RouteDError::new("NiYien filter has no supported clip ancestor"))
}

fn clip_asset_ref<'a>(clip: Node<'a, 'a>) -> Result<&'a str, RouteDError> {
    if clip.has_tag_name("asset-clip") {
        return clip
            .attribute("ref")
            .ok_or_else(|| RouteDError::new("asset-clip has no asset ref"));
    }
    if !clip.has_tag_name("clip") {
        return Err(RouteDError::new("unsupported clip wrapper"));
    }
    let clip_items: Vec<_> = clip
        .children()
        .filter(|node| {
            matches!(
                node.tag_name().name(),
                "video"
                    | "audio"
                    | "asset-clip"
                    | "clip"
                    | "ref-clip"
                    | "sync-clip"
                    | "mc-clip"
                    | "gap"
                    | "title"
                    | "caption"
                    | "spine"
            )
        })
        .collect();
    let [video] = clip_items.as_slice() else {
        return Err(RouteDError::new(
            "clip wrapper must contain exactly one direct video item",
        ));
    };
    if !video.has_tag_name("video") {
        return Err(RouteDError::new(
            "clip wrapper's only media item is not video",
        ));
    }
    video
        .attribute("ref")
        .ok_or_else(|| RouteDError::new("clip wrapper video has no asset ref"))
}

fn resolve_clip(
    document: &Document<'_>,
    project: Node<'_, '_>,
    filter: Node<'_, '_>,
    fcpxml_version: &str,
) -> Result<ResolvedClip, RouteDError> {
    let clip = effect_clip_container(filter)?;
    let _geometry = geometry_preflight(document, clip)?;
    let asset_ref = clip_asset_ref(clip)?;
    let asset = resource_by_id(document, "asset", asset_ref)?;
    validate_noop_conform_rate(document, clip, asset)?;
    let asset_start = optional_time(asset, "start")?;
    let clip_start = required_time(clip, "start")?;
    let clip_duration = required_time(clip, "duration")?;
    if clip_duration <= Ratio::from_integer(0) {
        return Err(RouteDError::new("asset-clip duration must be positive"));
    }

    let time_maps: Vec<_> = clip
        .children()
        .filter(|node| node.has_tag_name("timeMap"))
        .collect();
    if time_maps.len() > 1 {
        return Err(RouteDError::new("asset-clip has multiple timeMap elements"));
    }
    let mapping = if let Some(time_map) = time_maps.first() {
        let exported_time_points: Vec<_> = time_map
            .children()
            .filter(|node| node.has_tag_name("timept"))
            .collect();
        if exported_time_points.len() < 2 {
            return Err(RouteDError::new(
                "timeMap requires at least two timept values",
            ));
        }
        let exported_times: Vec<_> = exported_time_points
            .iter()
            .map(|point| required_time(*point, "time"))
            .collect::<Result<_, _>>()?;
        if exported_times
            .windows(2)
            .any(|window| window[0] >= window[1])
        {
            return Err(RouteDError::new(
                "timeMap control points must be strictly increasing",
            ));
        }
        let clip_end = clip_start.clone() + clip_duration.clone();
        let in_range_count = exported_times
            .iter()
            .take_while(|time| **time <= clip_end)
            .count();
        let can_ignore_trailing_points = in_range_count == 2
            && in_range_count < exported_time_points.len()
            && exported_times[0] == clip_start
            && exported_times[1] == clip_end
            && exported_time_points[..in_range_count].iter().all(|point| {
                point.attribute("interp") == Some("linear")
                    && point.attribute("inTime").is_none()
                    && point.attribute("outTime").is_none()
            });
        let time_points = if can_ignore_trailing_points {
            exported_time_points[..in_range_count].to_vec()
        } else {
            exported_time_points
        };
        let has_handles = time_points.iter().any(|point| {
            point.attribute("inTime").is_some() || point.attribute("outTime").is_some()
        });
        let has_deprecated_smoothing = time_points
            .iter()
            .any(|point| point.attribute("interp") == Some("smooth"));
        if has_deprecated_smoothing {
            return Err(RouteDError::new(
                "deprecated smooth timeMap interpolation lacks a verified per-frame expansion",
            ));
        }
        let all_explicit_linear = time_points
            .iter()
            .all(|point| point.attribute("interp") == Some("linear"));
        let all_smooth2 = time_points
            .iter()
            .all(|point| matches!(point.attribute("interp"), None | Some("smooth2")));
        let use_linear_points = (all_explicit_linear || time_points.len() == 2) && !has_handles;
        let points: Vec<_> = if use_linear_points {
            time_points
                .iter()
                .map(|point| {
                    let local = required_time(*point, "time")? - clip_start.clone();
                    let source = required_time(*point, "value")? - asset_start.clone();
                    Ok(MappingPoint {
                        local: rational_string(&local),
                        source: rational_string(&source),
                    })
                })
                .collect::<Result<_, RouteDError>>()?
        } else if all_smooth2 {
            smooth2_mapping(
                document,
                filter,
                asset,
                *time_map,
                &time_points,
                &clip_start,
                &asset_start,
                &clip_duration,
                fcpxml_version,
            )?
        } else {
            return Err(RouteDError::new(
                "mixed timeMap interpolation lacks a verified per-frame expansion",
            ));
        };
        let first_local = parse_payload_rational(&points[0].local)?;
        let last_local = parse_payload_rational(&points[points.len() - 1].local)?;
        if first_local != Ratio::from_integer(0) || last_local != clip_duration {
            return Err(RouteDError::new(
                "timeMap endpoints do not cover the full effect-local duration",
            ));
        }
        for window in points.windows(2) {
            if parse_payload_rational(&window[0].local)?
                >= parse_payload_rational(&window[1].local)?
            {
                return Err(RouteDError::new(
                    "timeMap local control points must be strictly increasing",
                ));
            }
        }
        points
    } else {
        let source_start = clip_start.clone() - asset_start.clone();
        vec![
            MappingPoint {
                local: "0/1".to_string(),
                source: rational_string(&source_start),
            },
            MappingPoint {
                local: rational_string(&clip_duration),
                source: rational_string(&(source_start + clip_duration.clone())),
            },
        ]
    };

    let compound_ref = compound_context(project, filter)?;
    let fingerprint = StructureFingerprint {
        fcpxml_version: fcpxml_version.to_string(),
        clip_tag: clip.tag_name().name().to_string(),
        asset_ref: asset_ref.to_string(),
        asset_start: rational_string(&asset_start),
        clip_start: rational_string(&clip_start),
        clip_offset: rational_string(&optional_time(clip, "offset")?),
        clip_duration: rational_string(&clip_duration),
        mapping: mapping.clone(),
        compound_ref,
    };
    let fingerprint_bytes = serde_json::to_vec(&fingerprint)
        .map_err(|error| RouteDError::new(format!("structure hash encoding failed: {error}")))?;
    Ok(ResolvedClip {
        asset_ref: asset_ref.to_string(),
        mapping,
        bounds_start: clip_start,
        duration: clip_duration,
        structure_sha256: format!("{:x}", Sha256::digest(fingerprint_bytes)),
    })
}

fn parse_payload_rational(value: &str) -> Result<Rational, RouteDError> {
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| RouteDError::new(format!("invalid payload rational: {value}")))?;
    let numerator = numerator
        .parse::<i128>()
        .map_err(|_| RouteDError::new(format!("invalid payload rational: {value}")))?;
    let denominator = denominator
        .parse::<i128>()
        .map_err(|_| RouteDError::new(format!("invalid payload rational: {value}")))?;
    if denominator == 0 {
        return Err(RouteDError::new("payload rational denominator is zero"));
    }
    Ok(Ratio::new(numerator, denominator))
}

fn attribute_value_range(
    input: &str,
    attribute: Attribute<'_, '_>,
) -> Result<Range<usize>, RouteDError> {
    let attribute_range = attribute.range();
    let raw = &input[attribute_range.clone()];
    let equals = raw
        .find('=')
        .ok_or_else(|| RouteDError::new("attribute has no equals sign"))?;
    let quoted = &raw[equals + 1..];
    let quote_offset = quoted
        .find(['\'', '"'])
        .ok_or_else(|| RouteDError::new("attribute value is not quoted"))?;
    let quote = quoted.as_bytes()[quote_offset] as char;
    let value_start_in_raw = equals + 1 + quote_offset + 1;
    let value_tail = &raw[value_start_in_raw..];
    let value_end = value_tail
        .find(quote)
        .ok_or_else(|| RouteDError::new("attribute value has no closing quote"))?;
    Ok(attribute_range.start + value_start_in_raw
        ..attribute_range.start + value_start_in_raw + value_end)
}

fn xml_attribute_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn valid_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn reserved_project_parameter_id_from_key(key: &str) -> Option<u32> {
    let suffix = key.strip_prefix(&format!("{PROJECT_PARAMETER_KEY_PREFIX}/"))?;
    if suffix.contains('/') {
        return None;
    }
    let identifier = suffix.parse::<u32>().ok()?;
    matches!(
        identifier,
        1901..=1906 | 1910..=1919 | 1930..=1939 | 2001..=2007
    )
    .then_some(identifier)
}

fn decode_project_bank(
    filter: Node<'_, '_>,
    occurrence: usize,
    bank: char,
) -> Result<Option<ProjectBankCandidate>, RouteDError> {
    let (manifest_id, chunk_start) = match bank {
        'A' => (1904_u32, 1910_u32),
        'B' => (1905_u32, 1930_u32),
        _ => return Err(RouteDError::new("internal project payload bank is unknown")),
    };
    let manifest_params = parameter_with_production_id(filter, manifest_id);
    if manifest_params.len() > 1 {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload bank {bank} has multiple manifests"
        )));
    }
    let Some(manifest_param) = manifest_params.first().copied() else {
        return Ok(None);
    };
    let encoded_manifest = manifest_param.attribute("value").unwrap_or_default();
    if encoded_manifest.is_empty() {
        return Ok(None);
    }
    let manifest_bytes = STANDARD.decode(encoded_manifest).map_err(|error| {
        RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload bank {bank} manifest Base64 is invalid: {error}"
        ))
    })?;
    let manifest: ProjectBankManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            RouteDError::new(format!(
                "NiYien occurrence {occurrence} project payload bank {bank} manifest JSON is invalid: {error}"
            ))
        })?;
    if manifest.version != PROJECT_BANK_MANIFEST_VERSION {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload bank {bank} manifest version {} is unknown",
            manifest.version
        )));
    }
    if manifest.generation == 0
        || manifest.chunk_count == 0
        || manifest.chunk_count > PROJECT_BANK_CHUNKS
        || manifest.encoded_length == 0
        || manifest.encoded_length > PROJECT_BANK_MAXIMUM_BYTES
        || manifest.chunk_count != manifest.encoded_length.div_ceil(PROJECT_BANK_CHUNK_BYTES)
        || !valid_lowercase_sha256(&manifest.payload_sha256)
    {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload bank {bank} manifest bounds are invalid"
        )));
    }
    let key_prefix = PROJECT_PARAMETER_KEY_PREFIX.to_string();

    let mut payload = String::with_capacity(manifest.encoded_length);
    for index in 0..manifest.chunk_count {
        let chunk_params = parameter_with_production_id(filter, chunk_start + index as u32);
        if chunk_params.len() != 1 {
            return Err(RouteDError::unsafe_structure(format!(
                "NiYien occurrence {occurrence} project payload bank {bank} chunk {} is missing or ambiguous",
                index + 1
            )));
        }
        let chunk = chunk_params[0];
        let value = chunk.attribute("value").ok_or_else(|| {
            RouteDError::new(format!(
                "NiYien occurrence {occurrence} project payload bank {bank} chunk {} has no value",
                index + 1
            ))
        })?;
        let expected_length = if index + 1 < manifest.chunk_count {
            PROJECT_BANK_CHUNK_BYTES
        } else {
            manifest.encoded_length - PROJECT_BANK_CHUNK_BYTES * (manifest.chunk_count - 1)
        };
        if !value.is_ascii() || value.len() != expected_length {
            return Err(RouteDError::unsafe_structure(format!(
                "NiYien occurrence {occurrence} project payload bank {bank} chunk {} length is invalid",
                index + 1
            )));
        }
        payload.push_str(value);
    }
    if payload.len() != manifest.encoded_length
        || format!("{:x}", Sha256::digest(payload.as_bytes())) != manifest.payload_sha256
    {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload bank {bank} hash is invalid"
        )));
    }
    STANDARD.decode(payload.as_bytes()).map_err(|error| {
        RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload bank {bank} content Base64 is invalid: {error}"
        ))
    })?;
    Ok(Some(ProjectBankCandidate {
        generation: manifest.generation,
        payload,
        key_prefix,
    }))
}

fn project_payload_key_prefix(
    filter: Node<'_, '_>,
    occurrence: usize,
) -> Result<String, RouteDError> {
    let (candidate_a, error_a) = match decode_project_bank(filter, occurrence, 'A') {
        Ok(candidate) => (candidate, None),
        Err(error) => (None, Some(error)),
    };
    let (candidate_b, error_b) = match decode_project_bank(filter, occurrence, 'B') {
        Ok(candidate) => (candidate, None),
        Err(error) => (None, Some(error)),
    };
    let selected = match (candidate_a, candidate_b) {
        (Some(first), Some(second)) => {
            if first.key_prefix != second.key_prefix {
                return Err(RouteDError::unsafe_structure(format!(
                    "NiYien occurrence {occurrence} project payload banks have different key prefixes"
                )));
            }
            if first.generation == second.generation {
                if first.payload != second.payload {
                    return Err(RouteDError::new(format!(
                        "NiYien occurrence {occurrence} project payload banks have the same generation with different content"
                    )));
                }
                Some(first)
            } else if first.generation > second.generation {
                Some(first)
            } else {
                Some(second)
            }
        }
        (Some(candidate), None) | (None, Some(candidate)) => Some(candidate),
        (None, None) => None,
    };
    if let Some(selected) = selected {
        return Ok(selected.key_prefix);
    }

    let legacy_params = parameter_with_production_id(filter, 1902);
    if legacy_params.len() > 1 {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} has multiple legacy Project Payload parameters"
        )));
    }
    if let Some(legacy) = legacy_params.first() {
        if legacy
            .attribute("value")
            .is_some_and(|value| !value.is_empty())
        {
            return Ok(PROJECT_PARAMETER_KEY_PREFIX.to_string());
        }
    }
    if let Some(error) = error_a.or(error_b) {
        return Err(error);
    }
    Err(RouteDError::new(format!(
        "NiYien occurrence {occurrence} has no provable Timing Payload mapping"
    )))
}

fn validate_encoded_project_payload(payload: &str) -> Result<(), RouteDError> {
    let bytes = super::decode_project_payload(payload.as_bytes())
        .map_err(|error| RouteDError::new(error.message))?;
    super::parse_project(&bytes)
        .map(|_| ())
        .map_err(RouteDError::new)
}

fn selected_valid_project_payload(
    filter: Node<'_, '_>,
    occurrence: usize,
) -> Result<Option<ProjectBankCandidate>, RouteDError> {
    let mut candidates = Vec::new();
    for bank in ['A', 'B'] {
        if let Ok(Some(candidate)) = decode_project_bank(filter, occurrence, bank)
            && validate_encoded_project_payload(&candidate.payload).is_ok()
        {
            candidates.push(candidate);
        }
    }
    if candidates.len() == 2
        && candidates[0].generation == candidates[1].generation
        && candidates[0].payload != candidates[1].payload
    {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} project payload banks have the same generation with different content"
        )));
    }
    if let Some(candidate) = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.generation)
    {
        return Ok(Some(candidate));
    }

    let legacy = parameter_with_production_id(filter, 1902);
    if legacy.len() > 1 {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} has multiple legacy Project Payload parameters"
        )));
    }
    let Some(legacy) = legacy.first() else {
        return Ok(None);
    };
    let payload = legacy.attribute("value").unwrap_or_default();
    if payload.is_empty() || validate_encoded_project_payload(payload).is_err() {
        return Ok(None);
    }
    Ok(Some(ProjectBankCandidate {
        generation: 0,
        payload: payload.to_string(),
        key_prefix: PROJECT_PARAMETER_KEY_PREFIX.to_string(),
    }))
}

fn route_d_instance_identity(input: &[u8], node_offset: usize) -> String {
    let seed = format!(
        "route-d-instance-v1|{node_offset}|{:x}",
        Sha256::digest(input)
    );
    import_token(seed.as_bytes())
}

fn encoded_project_banks(
    payload: &str,
    key_prefix: &str,
    instance_identity: &str,
) -> Result<String, RouteDError> {
    if payload.is_empty() || !payload.is_ascii() || payload.len() > PROJECT_BANK_MAXIMUM_BYTES {
        return Err(RouteDError::new(format!(
            "encoded project payload length {} exceeds the 4 MiB bank limit",
            payload.len()
        )));
    }
    let chunk_count = payload.len().div_ceil(PROJECT_BANK_CHUNK_BYTES);
    if chunk_count == 0 || chunk_count > PROJECT_BANK_CHUNKS {
        return Err(RouteDError::new(
            "encoded project payload chunk count is invalid",
        ));
    }
    let payload_sha256 = format!("{:x}", Sha256::digest(payload.as_bytes()));
    let mut parameters = format!(
        "<param name=\"Instance Identity\" key=\"{key_prefix}/1901\" value=\"{instance_identity}\"/>"
    );
    for (bank, manifest_id, chunk_start, generation) in [
        ('A', 1904_u32, 1910_u32, 2_u64),
        ('B', 1905_u32, 1930_u32, 1_u64),
    ] {
        let manifest = serde_json::json!({
            "version": PROJECT_BANK_MANIFEST_VERSION,
            "generation": generation,
            "chunk_count": chunk_count,
            "encoded_length": payload.len(),
            "payload_sha256": payload_sha256,
        });
        let manifest = STANDARD.encode(serde_json::to_vec(&manifest).map_err(|error| {
            RouteDError::new(format!("project bank manifest encoding failed: {error}"))
        })?);
        parameters.push_str(&format!(
            "<param name=\"Project Payload Manifest {bank}\" key=\"{key_prefix}/{manifest_id}\" value=\"{manifest}\"/>"
        ));
        for (index, chunk) in payload
            .as_bytes()
            .chunks(PROJECT_BANK_CHUNK_BYTES)
            .enumerate()
        {
            let chunk = std::str::from_utf8(chunk)
                .map_err(|_| RouteDError::new("encoded project payload is not ASCII"))?;
            parameters.push_str(&format!(
                "<param name=\"Project Payload {bank} {:02}\" key=\"{key_prefix}/{}\" value=\"{chunk}\"/>",
                index + 1,
                chunk_start + index as u32
            ));
        }
    }
    Ok(parameters)
}

struct SiblingProject {
    media_url: String,
    expected_path: PathBuf,
    display_name: String,
    bytes: Vec<u8>,
    payload: String,
    parameters: super::GFRenderParameters,
}

struct SiblingProjectError {
    reason: BatchSkipReason,
    detail: String,
    media_url: Option<String>,
    expected_path: Option<PathBuf>,
    display_name: Option<String>,
}

fn read_bounded_project_file(mut file: File) -> io::Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sibling project is not a regular file",
        ));
    }
    if metadata.len() > super::MAX_PROJECT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            RAW_PROJECT_TOO_LARGE,
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(super::MAX_PROJECT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > super::MAX_PROJECT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            RAW_PROJECT_TOO_LARGE,
        ));
    }
    Ok(bytes)
}

fn open_project_file_nofollow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    options.open(path)
}

fn read_project_file(path: &Path) -> io::Result<Vec<u8>> {
    read_bounded_project_file(open_project_file_nofollow(path)?)
}

struct AuthorizedRoot {
    selected: PathBuf,
    canonical: PathBuf,
}

fn canonical_authorized_roots(roots: &[PathBuf]) -> Result<Vec<AuthorizedRoot>, RouteDError> {
    let mut authorized = Vec::new();
    for root in roots {
        let resolved = std::fs::canonicalize(root).map_err(|error| {
            RouteDError::new(format!(
                "authorized media root {} cannot be resolved: {error}",
                root.display()
            ))
        })?;
        if !resolved.is_dir() {
            return Err(RouteDError::new(format!(
                "authorized media root is not a directory: {}",
                root.display()
            )));
        }
        if !authorized
            .iter()
            .any(|entry: &AuthorizedRoot| entry.canonical == resolved)
        {
            authorized.push(AuthorizedRoot {
                selected: root.to_path_buf(),
                canonical: resolved,
            });
        }
    }
    Ok(authorized)
}

#[cfg(unix)]
fn open_relative_project_nofollow(root: &Path, relative: &Path) -> io::Result<File> {
    let mut root_options = OpenOptions::new();
    root_options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    let mut current = root_options.open(root)?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sibling project path resolves to the authorized directory",
        ));
    }
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sibling project path escapes the authorized media root",
            ));
        };
        let name = CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "sibling project path contains an embedded NUL",
            )
        })?;
        let final_component = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if final_component {
                0
            } else {
                libc::O_DIRECTORY
            };
        let descriptor = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            let error = io::Error::last_os_error();
            if !final_component
                && matches!(
                    error.raw_os_error(),
                    Some(code) if code == libc::ELOOP || code == libc::ENOTDIR
                )
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "sibling project ancestor is not a no-follow directory",
                ));
            }
            return Err(error);
        }
        current = unsafe { File::from_raw_fd(descriptor) };
    }
    Ok(current)
}

fn read_authorized_project_file(path: &Path, roots: &[AuthorizedRoot]) -> io::Result<Vec<u8>> {
    for root in roots {
        let relative = path
            .strip_prefix(&root.selected)
            .or_else(|_| path.strip_prefix(&root.canonical));
        let Ok(relative) = relative else {
            continue;
        };
        #[cfg(unix)]
        let file = open_relative_project_nofollow(&root.canonical, relative)?;
        #[cfg(not(unix))]
        let file = {
            let canonical = std::fs::canonicalize(path)?;
            if !canonical.starts_with(&root.canonical) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "sibling project path escapes the authorized media root",
                ));
            }
            open_project_file_nofollow(&canonical)?
        };
        return read_bounded_project_file(file);
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "sibling project is outside the authorized media roots",
    ))
}

fn sibling_project_for_asset_with_reader(
    document: &Document<'_>,
    asset_ref: &str,
    project_reader: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> Result<SiblingProject, SiblingProjectError> {
    let asset =
        resource_by_id(document, "asset", asset_ref).map_err(|error| SiblingProjectError {
            reason: BatchSkipReason::UnsupportedStructure,
            detail: error.to_string(),
            media_url: None,
            expected_path: None,
            display_name: None,
        })?;
    let originals: Vec<_> = asset
        .children()
        .filter(|node| {
            node.has_tag_name("media-rep") && node.attribute("kind") == Some("original-media")
        })
        .collect();
    if originals.len() != 1 {
        return Err(SiblingProjectError {
            reason: BatchSkipReason::AmbiguousMedia,
            detail: format!(
                "asset {asset_ref} must have exactly one original-media URL; found {}",
                originals.len()
            ),
            media_url: None,
            expected_path: None,
            display_name: None,
        });
    }
    let media_url = originals[0]
        .attribute("src")
        .ok_or_else(|| SiblingProjectError {
            reason: BatchSkipReason::AmbiguousMedia,
            detail: format!("asset {asset_ref} original-media URL is missing"),
            media_url: None,
            expected_path: None,
            display_name: None,
        })?
        .to_string();
    let parsed = url::Url::parse(&media_url).map_err(|error| SiblingProjectError {
        reason: BatchSkipReason::AmbiguousMedia,
        detail: format!("asset {asset_ref} original-media URL is invalid: {error}"),
        media_url: Some(media_url.clone()),
        expected_path: None,
        display_name: None,
    })?;
    if parsed.scheme() != "file" {
        return Err(SiblingProjectError {
            reason: BatchSkipReason::IncompatibleProject,
            detail: format!("asset {asset_ref} original-media URL is not local"),
            media_url: Some(media_url),
            expected_path: None,
            display_name: None,
        });
    }
    let media_path = parsed.to_file_path().map_err(|_| SiblingProjectError {
        reason: BatchSkipReason::IncompatibleProject,
        detail: format!("asset {asset_ref} original-media URL is not a local path"),
        media_url: Some(media_url.clone()),
        expected_path: None,
        display_name: None,
    })?;
    let candidate = sibling_project_path(&media_path).map_err(|error| SiblingProjectError {
        reason: BatchSkipReason::IncompatibleProject,
        detail: error.to_string(),
        media_url: Some(media_url.clone()),
        expected_path: None,
        display_name: None,
    })?;
    let display_name = candidate
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| SiblingProjectError {
            reason: BatchSkipReason::IncompatibleProject,
            detail: format!(
                "sibling project path has no filename: {}",
                candidate.display()
            ),
            media_url: Some(media_url.clone()),
            expected_path: Some(candidate.clone()),
            display_name: None,
        })?;
    let bytes = project_reader(&candidate).map_err(|error| SiblingProjectError {
        reason: match error.kind() {
            io::ErrorKind::NotFound => BatchSkipReason::MissingProject,
            io::ErrorKind::PermissionDenied => BatchSkipReason::PermissionDenied,
            io::ErrorKind::InvalidData if error.to_string().contains(RAW_PROJECT_TOO_LARGE) => {
                BatchSkipReason::PayloadTooLarge
            }
            _ => BatchSkipReason::IncompatibleProject,
        },
        detail: format!(
            "unable to read sibling project {}: {error}",
            candidate.display()
        ),
        media_url: Some(media_url.clone()),
        expected_path: Some(candidate.clone()),
        display_name: Some(display_name.clone()),
    })?;
    let project = super::parse_project(&bytes).map_err(|error| SiblingProjectError {
        reason: BatchSkipReason::InvalidProject,
        detail: format!(
            "sibling project {} is invalid: {error}",
            candidate.display()
        ),
        media_url: Some(media_url.clone()),
        expected_path: Some(candidate.clone()),
        display_name: Some(display_name.clone()),
    })?;
    let parameters = super::snapshot_project_parameters(&project.manager).map_err(|error| {
        SiblingProjectError {
            reason: BatchSkipReason::InvalidProject,
            detail: format!(
                "sibling project {} parameters are invalid: {error}",
                candidate.display()
            ),
            media_url: Some(media_url.clone()),
            expected_path: Some(candidate.clone()),
            display_name: Some(display_name.clone()),
        }
    })?;
    let payload = super::encode_project_payload(&bytes).map_err(|error| SiblingProjectError {
        reason: BatchSkipReason::InvalidProject,
        detail: format!(
            "sibling project {} cannot be encoded: {error}",
            candidate.display()
        ),
        media_url: Some(media_url.clone()),
        expected_path: Some(candidate.clone()),
        display_name: Some(display_name.clone()),
    })?;
    let payload = String::from_utf8(payload).map_err(|_| SiblingProjectError {
        reason: BatchSkipReason::InvalidProject,
        detail: "encoded sibling project payload is not ASCII".to_string(),
        media_url: Some(media_url.clone()),
        expected_path: Some(candidate.clone()),
        display_name: Some(display_name.clone()),
    })?;
    if payload.len() > PROJECT_BANK_MAXIMUM_BYTES {
        return Err(SiblingProjectError {
            reason: BatchSkipReason::PayloadTooLarge,
            detail: format!(
                "sibling project {} encodes beyond the 4 MiB limit",
                candidate.display()
            ),
            media_url: Some(media_url),
            expected_path: Some(candidate),
            display_name: Some(display_name),
        });
    }
    Ok(SiblingProject {
        media_url,
        expected_path: candidate,
        display_name,
        bytes,
        payload,
        parameters,
    })
}

fn sibling_project_path(media_path: &Path) -> Result<PathBuf, RouteDError> {
    let parent = media_path
        .parent()
        .ok_or_else(|| RouteDError::new("original-media path has no parent directory"))?;
    let stem = media_path
        .file_stem()
        .ok_or_else(|| RouteDError::new("original-media path has no basename"))?;
    let mut sibling_name = stem.to_os_string();
    sibling_name.push(".gyroflow");
    Ok(parent.join(sibling_name))
}

fn filter_children_replacement(
    input: &str,
    filter: Node<'_, '_>,
    children: String,
) -> Result<Replacement, RouteDError> {
    let range = filter.range();
    let source = &input[range.clone()];
    if let Some(offset) = source.rfind("</filter-video>") {
        let insertion = range.start + offset;
        return Ok(Replacement {
            range: insertion..insertion,
            value: children,
        });
    }
    if let Some(offset) = source.rfind("/>") {
        if source[offset + 2..].trim().is_empty() {
            return Ok(Replacement {
                range: range.start + offset..range.end,
                value: format!(">{children}</filter-video>"),
            });
        }
    }
    Err(RouteDError::new("NiYien filter-video is not expandable"))
}

fn clip_name(clip: Node<'_, '_>, asset_ref: &str) -> String {
    clip.attribute("name").unwrap_or(asset_ref).to_string()
}

#[derive(Clone, Copy)]
struct CanonicalFormat {
    width: Option<u32>,
    height: Option<u32>,
    pasp_h: u32,
    pasp_v: u32,
}

impl CanonicalFormat {
    fn detail(self) -> String {
        let dimensions = match (self.width, self.height) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => "unknown".to_string(),
        };
        format!("{dimensions} pasp={}/{}", self.pasp_h, self.pasp_v)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GeometryValueKind {
    Scalar,
    Point,
    Scale,
}

type GeometryParameterKeys = HashMap<String, GeometryValueKind>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CropEdge {
    Left = 0,
    Top = 1,
    Right = 2,
    Bottom = 3,
}

#[derive(Clone)]
struct CropKeyframe {
    time: Rational,
    value: f64,
}

#[derive(Clone)]
struct CropAnimation {
    keyframes: Vec<CropKeyframe>,
}

fn validate_attributes(
    node: Node<'_, '_>,
    allowed: &[&str],
    context: &str,
) -> Result<(), RouteDError> {
    if let Some(attribute) = node
        .attributes()
        .find(|attribute| !allowed.contains(&attribute.name()))
    {
        return Err(RouteDError::new(format!(
            "unknown {context} attribute {}",
            attribute.name()
        )));
    }
    Ok(())
}

fn positive_u32(value: &str, context: &str) -> Result<u32, RouteDError> {
    let value = value
        .parse::<u32>()
        .map_err(|_| RouteDError::new(format!("{context} must be a positive integer")))?;
    if value == 0 {
        return Err(RouteDError::new(format!(
            "{context} must be a positive integer"
        )));
    }
    Ok(value)
}

fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn canonical_format(
    document: &Document<'_>,
    format_id: Option<&str>,
    context: &str,
) -> Result<Option<CanonicalFormat>, RouteDError> {
    let Some(format_id) = format_id else {
        return Ok(None);
    };
    let format = resource_by_id(document, "format", format_id)?;
    let width = format.attribute("width");
    let height = format.attribute("height");
    let (width, height) = match (width, height) {
        (None, None) => (None, None),
        (Some(width), Some(height)) => (
            Some(positive_u32(width, &format!("{context} format width"))?),
            Some(positive_u32(height, &format!("{context} format height"))?),
        ),
        (Some(_), None) => {
            return Err(RouteDError::new(format!(
                "{context} format declares width without height"
            )));
        }
        (None, Some(_)) => {
            return Err(RouteDError::new(format!(
                "{context} format declares height without width"
            )));
        }
    };
    let pasp_h = format.attribute("paspH");
    let pasp_v = format.attribute("paspV");
    let (pasp_h, pasp_v) = match (pasp_h, pasp_v) {
        (None, None) => (1, 1),
        (Some(horizontal), Some(vertical)) => {
            let horizontal = positive_u32(horizontal, &format!("{context} format paspH"))?;
            let vertical = positive_u32(vertical, &format!("{context} format paspV"))?;
            let divisor = greatest_common_divisor(horizontal, vertical);
            (horizontal / divisor, vertical / divisor)
        }
        (Some(_), None) => {
            return Err(RouteDError::new(format!(
                "{context} format declares paspH without paspV"
            )));
        }
        (None, Some(_)) => {
            return Err(RouteDError::new(format!(
                "{context} format declares paspV without paspH"
            )));
        }
    };
    if format
        .attribute("projection")
        .is_some_and(|projection| projection != "none")
    {
        return Err(RouteDError::new(format!(
            "{context} format projection requires unsupported perspective geometry"
        )));
    }
    if format
        .attribute("stereoscopic")
        .is_some_and(|stereoscopic| stereoscopic != "mono")
    {
        return Err(RouteDError::new(format!(
            "{context} stereoscopic format geometry is unsupported"
        )));
    }
    Ok(Some(CanonicalFormat {
        width,
        height,
        pasp_h,
        pasp_v,
    }))
}

fn finite_values(
    value: &str,
    expected_lengths: &[usize],
    context: &str,
) -> Result<Vec<f64>, RouteDError> {
    let values: Vec<_> = value.split_ascii_whitespace().collect();
    if !expected_lengths.contains(&values.len()) {
        return Err(RouteDError::new(format!(
            "{context} has an unknown component count"
        )));
    }
    values
        .into_iter()
        .map(|component| {
            let component = component.parse::<f64>().map_err(|_| {
                RouteDError::new(format!("{context} values must be finite numbers"))
            })?;
            if !component.is_finite() {
                return Err(RouteDError::unsafe_structure(format!(
                    "{context} values must be finite numbers"
                )));
            }
            Ok(component)
        })
        .collect()
}

fn validate_scale(values: &[f64], context: &str) -> Result<(), RouteDError> {
    if values.iter().any(|value| *value == 0.0) {
        return Err(RouteDError::new(format!(
            "{context} is singular because scale is zero"
        )));
    }
    if values.iter().any(|value| *value < 0.0) {
        return Err(RouteDError::new(format!(
            "{context} reflection is outside ScaleTranslate support"
        )));
    }
    Ok(())
}

fn parameter_kind(name: &str, parent: Option<GeometryValueKind>) -> Option<GeometryValueKind> {
    match name.trim() {
        "位置"
        | "锚点"
        | "錨點"
        | "アンカー"
        | "위치"
        | "앵커"
        | "Положение"
        | "Привязка"
        | "Опорная точка" => return Some(GeometryValueKind::Point),
        "缩放"
        | "缩放（全部）"
        | "缩放 X"
        | "缩放 Y"
        | "縮放"
        | "縮放（全部）"
        | "縮放 X"
        | "縮放 Y"
        | "調整"
        | "調整（すべて）"
        | "調整 X"
        | "調整 Y"
        | "크기"
        | "크기 조절"
        | "크기(전체)"
        | "크기 X"
        | "크기 Y"
        | "Масштаб"
        | "Масштаб (все)"
        | "Масштаб X"
        | "Масштаб Y" => {
            return Some(GeometryValueKind::Scale);
        }
        "旋转" | "旋轉" | "回転" | "회전" | "Поворот" | "左" | "上" | "右" | "下" | "왼쪽"
        | "위" | "위쪽" | "오른쪽" | "아래" | "아래쪽" | "Слева" | "Сверху" | "Справа"
        | "Снизу" => return Some(GeometryValueKind::Scalar),
        "全部" | "すべて" | "전체" | "Все" => return parent,
        _ => {}
    }
    let normalized: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match normalized.as_str() {
        "position" | "anchor" => Some(GeometryValueKind::Point),
        "scale" | "scaleall" | "scalex" | "scaley" => Some(GeometryValueKind::Scale),
        "rotation" | "left" | "top" | "right" | "bottom" | "x" | "y" => {
            Some(GeometryValueKind::Scalar)
        }
        "all" | "amount" => parent,
        _ => None,
    }
}

fn geometry_parameter_kind(
    parameter: Node<'_, '_>,
    parent: Option<GeometryValueKind>,
    context: &str,
    keys: &mut GeometryParameterKeys,
) -> Result<GeometryValueKind, RouteDError> {
    let name = parameter
        .attribute("name")
        .ok_or_else(|| RouteDError::new(format!("{context} param has no name")))?;
    let kind = parameter_kind(name, parent)
        .ok_or_else(|| RouteDError::new(format!("unknown {context} parameter {name}")))?;
    if let Some(key) = parameter.attribute("key") {
        if key.is_empty() {
            return Err(RouteDError::new(format!(
                "{context} parameter {name} has an empty opaque geometry parameter key"
            )));
        }
        if let Some(previous) = keys.get(key) {
            if *previous != kind {
                return Err(RouteDError::new(format!(
                    "opaque geometry parameter key {key} has conflicting kinds"
                )));
            }
        } else {
            keys.insert(key.to_string(), kind);
        }
    }
    Ok(kind)
}

fn validate_geometry_value(
    value: &str,
    kind: GeometryValueKind,
    context: &str,
) -> Result<(), RouteDError> {
    let expected_lengths: &[usize] = match kind {
        GeometryValueKind::Scalar => &[1],
        GeometryValueKind::Point => &[2],
        GeometryValueKind::Scale => &[1, 2],
    };
    let values = finite_values(value, expected_lengths, context)?;
    if kind == GeometryValueKind::Scale {
        validate_scale(&values, context)?;
    }
    Ok(())
}

fn validate_keyframe_animation(
    animation: Node<'_, '_>,
    kind: GeometryValueKind,
    context: &str,
) -> Result<(), RouteDError> {
    validate_attributes(animation, &[], "keyframeAnimation")?;
    let children: Vec<_> = animation.children().filter(Node::is_element).collect();
    if children.is_empty() || children.iter().any(|child| !child.has_tag_name("keyframe")) {
        return Err(RouteDError::new(format!(
            "unknown {context} animation structure"
        )));
    }
    for keyframe in children {
        validate_attributes(
            keyframe,
            &["time", "value", "auxValue", "interp", "curve"],
            "geometry keyframe",
        )?;
        let time = keyframe
            .attribute("time")
            .ok_or_else(|| RouteDError::new(format!("{context} keyframe has no time")))?;
        parse_time(time)?;
        let value = keyframe
            .attribute("value")
            .ok_or_else(|| RouteDError::new(format!("{context} keyframe has no value")))?;
        validate_geometry_value(value, kind, &format!("{context} keyframe"))?;
        if let Some(auxiliary) = keyframe.attribute("auxValue") {
            finite_values(auxiliary, &[1, 2], &format!("{context} keyframe auxValue"))?;
        }
    }
    Ok(())
}

fn validate_geometry_parameter(
    parameter: Node<'_, '_>,
    parent_kind: Option<GeometryValueKind>,
    context: &str,
    keys: &mut GeometryParameterKeys,
) -> Result<bool, RouteDError> {
    validate_attributes(
        parameter,
        &["name", "key", "value", "auxValue", "enabled"],
        "geometry param",
    )?;
    let name = parameter
        .attribute("name")
        .ok_or_else(|| RouteDError::new(format!("{context} param has no name")))?;
    let kind = geometry_parameter_kind(parameter, parent_kind, context, keys)?;
    if let Some(enabled) = parameter.attribute("enabled")
        && !matches!(enabled, "0" | "1")
    {
        return Err(RouteDError::new(format!(
            "{context} param enabled value is unknown"
        )));
    }
    if let Some(value) = parameter.attribute("value") {
        validate_geometry_value(value, kind, &format!("{context} parameter {name}"))?;
    }
    if let Some(auxiliary) = parameter.attribute("auxValue") {
        finite_values(
            auxiliary,
            &[1, 2],
            &format!("{context} parameter {name} auxValue"),
        )?;
    }
    let mut animated = false;
    let mut animation_count = 0usize;
    for child in parameter.children().filter(Node::is_element) {
        if child.has_tag_name("keyframeAnimation") {
            animation_count += 1;
            validate_keyframe_animation(child, kind, context)?;
            animated = true;
        } else if child.has_tag_name("param") {
            animated |= validate_geometry_parameter(child, Some(kind), context, keys)?;
        } else {
            return Err(RouteDError::new(format!(
                "unknown {context} parameter child {}",
                child.tag_name().name()
            )));
        }
    }
    if animation_count > 1 {
        return Err(RouteDError::new(format!(
            "{context} parameter has multiple keyframeAnimation elements"
        )));
    }
    Ok(animated)
}

fn geometry_scopes<'a>(clip: Node<'a, 'a>) -> Vec<Node<'a, 'a>> {
    let mut scopes = vec![clip];
    if clip.has_tag_name("clip") {
        scopes.extend(clip.children().filter(|node| node.has_tag_name("video")));
    }
    scopes
}

fn geometry_children<'a>(clip: Node<'a, 'a>, tag: &str) -> Vec<Node<'a, 'a>> {
    geometry_scopes(clip)
        .into_iter()
        .flat_map(|scope| scope.children())
        .filter(|node| node.has_tag_name(tag))
        .collect()
}

fn enabled_attribute(node: Node<'_, '_>, context: &str) -> Result<bool, RouteDError> {
    match node.attribute("enabled") {
        None | Some("1") => Ok(true),
        Some("0") => Ok(false),
        Some(_) => Err(RouteDError::new(format!(
            "{context} enabled value is unknown"
        ))),
    }
}

fn conform_detail(clip: Node<'_, '_>) -> Result<String, RouteDError> {
    let nodes = geometry_children(clip, "adjust-conform");
    if nodes.len() > 1 {
        return Err(RouteDError::new(
            "conflicting geometry has multiple adjust-conform elements",
        ));
    }
    let Some(conform) = nodes.first().copied() else {
        return Ok("fit".to_string());
    };
    validate_attributes(conform, &["type"], "adjust-conform")?;
    if conform.children().any(|child| child.is_element()) {
        return Err(RouteDError::new("unknown adjust-conform child structure"));
    }
    match conform.attribute("type").unwrap_or("fit") {
        kind @ ("fit" | "fill" | "none") => Ok(kind.to_string()),
        kind => Err(RouteDError::new(format!(
            "unknown adjust-conform type {kind}"
        ))),
    }
}

fn transform_detail(
    clip: Node<'_, '_>,
    keys: &mut GeometryParameterKeys,
) -> Result<String, RouteDError> {
    let nodes = geometry_children(clip, "adjust-transform");
    if nodes.len() > 1 {
        return Err(RouteDError::new(
            "conflicting geometry has multiple adjust-transform elements",
        ));
    }
    let Some(transform) = nodes.first().copied() else {
        return Ok("none".to_string());
    };
    validate_attributes(
        transform,
        &[
            "enabled", "position", "scale", "rotation", "anchor", "tracking",
        ],
        "adjust-transform",
    )?;
    if transform.attribute("tracking").is_some() {
        return Err(RouteDError::new(
            "tracked transform geometry is not a proven ScaleTranslate structure",
        ));
    }
    let enabled = enabled_attribute(transform, "adjust-transform")?;
    validate_geometry_value(
        transform.attribute("position").unwrap_or("0 0"),
        GeometryValueKind::Point,
        "adjust-transform position",
    )?;
    validate_geometry_value(
        transform.attribute("scale").unwrap_or("1 1"),
        GeometryValueKind::Scale,
        "adjust-transform scale",
    )?;
    validate_geometry_value(
        transform.attribute("rotation").unwrap_or("0"),
        GeometryValueKind::Scalar,
        "adjust-transform rotation",
    )?;
    validate_geometry_value(
        transform.attribute("anchor").unwrap_or("0 0"),
        GeometryValueKind::Point,
        "adjust-transform anchor",
    )?;
    let mut animated = false;
    for child in transform.children().filter(Node::is_element) {
        if !child.has_tag_name("param") {
            return Err(RouteDError::new(format!(
                "unknown adjust-transform child {}",
                child.tag_name().name()
            )));
        }
        animated |= validate_geometry_parameter(child, None, "adjust-transform", keys)?;
    }
    Ok(if !enabled {
        "disabled"
    } else if animated {
        "animated"
    } else {
        "static"
    }
    .to_string())
}

fn rect_values(rect: Node<'_, '_>, context: &str) -> Result<[f64; 4], RouteDError> {
    validate_attributes(rect, &["left", "top", "right", "bottom"], context)?;
    let mut values = [0.0; 4];
    for (index, attribute) in ["left", "top", "right", "bottom"].into_iter().enumerate() {
        values[index] = finite_values(
            rect.attribute(attribute).unwrap_or("0"),
            &[1],
            &format!("{context} {attribute}"),
        )?[0];
        if values[index] < 0.0 {
            return Err(RouteDError::new(format!(
                "{context} negative crop is outside verified geometry support"
            )));
        }
    }
    Ok(values)
}

fn validate_static_crop_extent(
    values: [f64; 4],
    format: Option<CanonicalFormat>,
    context: &str,
) -> Result<(), RouteDError> {
    if values[1] + values[3] >= 100.0 {
        return Err(RouteDError::new(format!(
            "{context} is singular because top and bottom remove the full frame"
        )));
    }
    let Some(CanonicalFormat {
        width: Some(width),
        height: Some(height),
        pasp_h,
        pasp_v,
    }) = format
    else {
        return Err(RouteDError::new(format!(
            "{context} cannot be proven without asset format dimensions"
        )));
    };
    let width_in_height_percent =
        100.0 * f64::from(width) * f64::from(pasp_h) / f64::from(height) / f64::from(pasp_v);
    if values[0] + values[2] >= width_in_height_percent {
        return Err(RouteDError::new(format!(
            "{context} is singular because left and right remove the full frame"
        )));
    }
    Ok(())
}

fn crop_edge(name: &str) -> Option<CropEdge> {
    match name.trim() {
        "左" | "왼쪽" | "Слева" => return Some(CropEdge::Left),
        "上" | "위" | "위쪽" | "Сверху" => return Some(CropEdge::Top),
        "右" | "오른쪽" | "Справа" => return Some(CropEdge::Right),
        "下" | "아래" | "아래쪽" | "Снизу" => return Some(CropEdge::Bottom),
        _ => {}
    }
    let normalized: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match normalized.as_str() {
        "left" => Some(CropEdge::Left),
        "top" => Some(CropEdge::Top),
        "right" => Some(CropEdge::Right),
        "bottom" => Some(CropEdge::Bottom),
        _ => None,
    }
}

fn nonnegative_crop_value(value: &str, context: &str) -> Result<f64, RouteDError> {
    let value = finite_values(value, &[1], context)?[0];
    if value < 0.0 {
        return Err(RouteDError::new(format!(
            "{context} negative crop is outside verified geometry support"
        )));
    }
    Ok(value)
}

fn validate_inactive_geometry_parameter(
    parameter: Node<'_, '_>,
    context: &str,
) -> Result<(), RouteDError> {
    validate_attributes(
        parameter,
        &["name", "key", "value", "auxValue", "enabled"],
        "geometry param",
    )?;
    if parameter.attribute("name").is_none() {
        return Err(RouteDError::new(format!("{context} param has no name")));
    }
    if let Some(enabled) = parameter.attribute("enabled")
        && !matches!(enabled, "0" | "1")
    {
        return Err(RouteDError::new(format!(
            "{context} param enabled value is unknown"
        )));
    }
    for child in parameter.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "fadeIn" | "fadeOut" => {
                validate_attributes(child, &["type", "duration"], child.tag_name().name())?;
                if child.attribute("duration").is_none()
                    || child.children().any(|grandchild| grandchild.is_element())
                {
                    return Err(RouteDError::new(format!(
                        "unknown inactive {context} {} structure",
                        child.tag_name().name()
                    )));
                }
                if let Some(kind) = child.attribute("type")
                    && !matches!(kind, "linear" | "easeIn" | "easeOut" | "easeInOut")
                {
                    return Err(RouteDError::new(format!(
                        "unknown inactive {context} fade type"
                    )));
                }
            }
            "keyframeAnimation" => {
                validate_attributes(child, &[], "keyframeAnimation")?;
                for keyframe in child.children().filter(Node::is_element) {
                    if !keyframe.has_tag_name("keyframe") {
                        return Err(RouteDError::new(format!(
                            "unknown inactive {context} animation structure"
                        )));
                    }
                    validate_attributes(
                        keyframe,
                        &["time", "value", "auxValue", "interp", "curve"],
                        "geometry keyframe",
                    )?;
                    if keyframe.attribute("time").is_none()
                        || keyframe.attribute("value").is_none()
                        || keyframe
                            .children()
                            .any(|grandchild| grandchild.is_element())
                    {
                        return Err(RouteDError::new(format!(
                            "unknown inactive {context} keyframe structure"
                        )));
                    }
                    if let Some(interp) = keyframe.attribute("interp")
                        && !matches!(interp, "linear" | "ease" | "easeIn" | "easeOut")
                    {
                        return Err(RouteDError::new(format!(
                            "unknown inactive {context} keyframe interpolation"
                        )));
                    }
                    if let Some(curve) = keyframe.attribute("curve")
                        && !matches!(curve, "linear" | "smooth")
                    {
                        return Err(RouteDError::new(format!(
                            "unknown inactive {context} keyframe curve"
                        )));
                    }
                }
            }
            "param" => validate_inactive_geometry_parameter(child, context)?,
            tag => {
                return Err(RouteDError::new(format!(
                    "unknown inactive {context} parameter child {tag}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_inactive_crop_rect(rect: Node<'_, '_>, context: &str) -> Result<(), RouteDError> {
    validate_attributes(rect, &["left", "top", "right", "bottom"], context)?;
    if rect.has_tag_name("pan-rect") {
        if rect.children().any(|child| child.is_element()) {
            return Err(RouteDError::new(format!(
                "unknown inactive {context} child structure"
            )));
        }
        return Ok(());
    }
    for child in rect.children().filter(Node::is_element) {
        if !child.has_tag_name("param") {
            return Err(RouteDError::new(format!(
                "unknown inactive {context} child {}",
                child.tag_name().name()
            )));
        }
        validate_inactive_geometry_parameter(child, context)?;
    }
    Ok(())
}

fn parse_linear_crop_animation(
    animation: Node<'_, '_>,
    context: &str,
) -> Result<CropAnimation, RouteDError> {
    validate_attributes(animation, &[], "keyframeAnimation")?;
    let children: Vec<_> = animation.children().filter(Node::is_element).collect();
    if children.is_empty() || children.iter().any(|child| !child.has_tag_name("keyframe")) {
        return Err(RouteDError::new(format!(
            "unknown {context} animation structure"
        )));
    }
    let mut keyframes = Vec::with_capacity(children.len());
    for keyframe in children {
        validate_attributes(
            keyframe,
            &["time", "value", "auxValue", "interp", "curve"],
            "geometry keyframe",
        )?;
        if keyframe.children().any(|child| child.is_element()) {
            return Err(RouteDError::new(format!(
                "unknown {context} keyframe structure"
            )));
        }
        if !matches!(keyframe.attribute("interp"), None | Some("linear"))
            || keyframe.attribute("curve") != Some("linear")
            || keyframe.attribute("auxValue").is_some()
        {
            return Err(RouteDError::new(format!(
                "{context} crop animation requires proven linear interpolation"
            )));
        }
        let time = keyframe
            .attribute("time")
            .ok_or_else(|| RouteDError::new(format!("{context} keyframe has no time")))?;
        let value = keyframe
            .attribute("value")
            .ok_or_else(|| RouteDError::new(format!("{context} keyframe has no value")))?;
        let keyframe = CropKeyframe {
            time: parse_time(time)?,
            value: nonnegative_crop_value(value, &format!("{context} keyframe"))?,
        };
        if keyframes
            .last()
            .is_some_and(|previous: &CropKeyframe| previous.time >= keyframe.time)
        {
            return Err(RouteDError::new(format!(
                "{context} keyframe times must be strictly increasing"
            )));
        }
        keyframes.push(keyframe);
    }
    Ok(CropAnimation { keyframes })
}

fn crop_animation_value(
    animation: &CropAnimation,
    time: &Rational,
    context: &str,
) -> Result<f64, RouteDError> {
    let first = &animation.keyframes[0];
    if time <= &first.time {
        return Ok(first.value);
    }
    let last = animation.keyframes.last().unwrap();
    if time >= &last.time {
        return Ok(last.value);
    }
    let [start, end] = animation
        .keyframes
        .windows(2)
        .find(|window| window[0].time <= *time && *time <= window[1].time)
        .unwrap()
    else {
        unreachable!()
    };
    let parameter = ((time.clone() - start.time.clone()) / (end.time.clone() - start.time.clone()))
        .to_f64()
        .ok_or_else(|| RouteDError::new(format!("{context} keyframe time is out of range")))?;
    let value = start.value + (end.value - start.value) * parameter;
    if !value.is_finite() {
        return Err(RouteDError::new(format!(
            "{context} interpolated crop must be finite"
        )));
    }
    Ok(value)
}

fn validate_active_crop_rect(
    rect: Node<'_, '_>,
    format: Option<CanonicalFormat>,
    context: &str,
    keys: &mut GeometryParameterKeys,
) -> Result<bool, RouteDError> {
    let mut values = rect_values(rect, context)?;
    let mut animations: [Option<CropAnimation>; 4] = [None, None, None, None];
    let mut seen_edges = [false; 4];
    let mut crop_keys: HashMap<&str, CropEdge> = HashMap::new();
    for parameter in rect.children().filter(Node::is_element) {
        if !parameter.has_tag_name("param") {
            return Err(RouteDError::new(format!(
                "unknown {context} child {}",
                parameter.tag_name().name()
            )));
        }
        validate_attributes(
            parameter,
            &["name", "key", "value", "auxValue", "enabled"],
            "geometry param",
        )?;
        let name = parameter
            .attribute("name")
            .ok_or_else(|| RouteDError::new(format!("{context} param has no name")))?;
        let edge = crop_edge(name)
            .ok_or_else(|| RouteDError::new(format!("unknown {context} parameter {name}")))?;
        let kind = geometry_parameter_kind(parameter, None, context, keys)?;
        if kind != GeometryValueKind::Scalar {
            return Err(RouteDError::new(format!(
                "{context} parameter {name} is not a scalar crop edge"
            )));
        }
        if let Some(key) = parameter.attribute("key")
            && let Some(previous) = crop_keys.insert(key, edge)
            && previous != edge
        {
            return Err(RouteDError::new(format!(
                "opaque geometry parameter key {key} has conflicting crop edges"
            )));
        }
        let index = edge as usize;
        if seen_edges[index] {
            return Err(RouteDError::new(format!(
                "{context} has multiple {name} parameters"
            )));
        }
        seen_edges[index] = true;
        let enabled = match parameter.attribute("enabled") {
            None | Some("1") => true,
            Some("0") => false,
            Some(_) => {
                return Err(RouteDError::new(format!(
                    "{context} param enabled value is unknown"
                )));
            }
        };
        if !enabled {
            validate_inactive_geometry_parameter(parameter, context)?;
            continue;
        }
        if parameter.attribute("auxValue").is_some() {
            return Err(RouteDError::new(format!(
                "{context} parameter {name} auxiliary geometry is unproved"
            )));
        }
        if let Some(value) = parameter.attribute("value") {
            values[index] = nonnegative_crop_value(value, &format!("{context} parameter {name}"))?;
        }
        let children: Vec<_> = parameter.children().filter(Node::is_element).collect();
        if children.len() > 1
            || children
                .first()
                .is_some_and(|child| !child.has_tag_name("keyframeAnimation"))
        {
            return Err(RouteDError::new(format!(
                "unknown {context} parameter {name} child structure"
            )));
        }
        if let Some(animation) = children.first() {
            animations[index] = Some(parse_linear_crop_animation(
                *animation,
                &format!("{context} parameter {name}"),
            )?);
        }
    }
    validate_static_crop_extent(values, format, context)?;
    let mut times = animations
        .iter()
        .flatten()
        .flat_map(|animation| {
            animation
                .keyframes
                .iter()
                .map(|keyframe| keyframe.time.clone())
        })
        .collect::<Vec<_>>();
    if times.is_empty() {
        return Ok(false);
    }
    times.sort();
    times.dedup();
    for time in times {
        let mut animated_values = values;
        for (index, animation) in animations.iter().enumerate() {
            if let Some(animation) = animation {
                animated_values[index] = crop_animation_value(animation, &time, context)?;
            }
        }
        validate_static_crop_extent(
            animated_values,
            format,
            &format!("{context} keyframe at {}", rational_string(&time)),
        )?;
    }
    Ok(true)
}

fn crop_detail(
    clip: Node<'_, '_>,
    asset_format: Option<CanonicalFormat>,
    keys: &mut GeometryParameterKeys,
) -> Result<String, RouteDError> {
    let nodes = geometry_children(clip, "adjust-crop");
    if nodes.len() > 1 {
        return Err(RouteDError::new(
            "conflicting geometry has multiple adjust-crop elements",
        ));
    }
    let Some(crop) = nodes.first().copied() else {
        return Ok("none".to_string());
    };
    validate_attributes(crop, &["mode", "enabled"], "adjust-crop")?;
    let enabled = enabled_attribute(crop, "adjust-crop")?;
    let mode = crop
        .attribute("mode")
        .ok_or_else(|| RouteDError::new("adjust-crop mode is missing"))?;
    if !matches!(mode, "crop" | "trim" | "pan") {
        return Err(RouteDError::new(format!("unknown adjust-crop mode {mode}")));
    }
    let children: Vec<_> = crop.children().filter(Node::is_element).collect();
    if let Some(child) = children.iter().find(|child| {
        !matches!(
            child.tag_name().name(),
            "crop-rect" | "trim-rect" | "pan-rect"
        )
    }) {
        return Err(RouteDError::new(format!(
            "unknown adjust-crop child {}",
            child.tag_name().name()
        )));
    }
    let crop_rects: Vec<_> = children
        .iter()
        .copied()
        .filter(|child| child.has_tag_name("crop-rect"))
        .collect();
    let trim_rects: Vec<_> = children
        .iter()
        .copied()
        .filter(|child| child.has_tag_name("trim-rect"))
        .collect();
    let pan_rects: Vec<_> = children
        .iter()
        .copied()
        .filter(|child| child.has_tag_name("pan-rect"))
        .collect();
    if crop_rects.len() > 1 || trim_rects.len() > 1 || !matches!(pan_rects.len(), 0 | 2) {
        return Err(RouteDError::new(
            "Ken Burns/crop geometry has conflicting rectangle structure",
        ));
    }
    let mut last_rank = 0usize;
    for child in &children {
        let rank = match child.tag_name().name() {
            "crop-rect" => 0,
            "trim-rect" => 1,
            "pan-rect" => 2,
            _ => unreachable!(),
        };
        if rank < last_rank {
            return Err(RouteDError::new(
                "adjust-crop rectangle order does not match the FCPXML schema",
            ));
        }
        last_rank = rank;
    }
    if !enabled {
        for rect in &children {
            validate_inactive_crop_rect(*rect, rect.tag_name().name())?;
        }
        return Ok("disabled".to_string());
    }
    match mode {
        "pan" => {
            if pan_rects.len() != 2 {
                return Err(RouteDError::new(
                    "Ken Burns geometry requires exactly two pan-rect elements",
                ));
            }
            for rect in crop_rects.iter().chain(trim_rects.iter()) {
                validate_inactive_crop_rect(*rect, rect.tag_name().name())?;
            }
            for (index, rect) in pan_rects.iter().enumerate() {
                validate_inactive_crop_rect(*rect, "Ken Burns pan-rect")?;
                validate_static_crop_extent(
                    rect_values(*rect, "Ken Burns pan-rect")?,
                    asset_format,
                    if index == 0 {
                        "Ken Burns start pan-rect"
                    } else {
                        "Ken Burns end pan-rect"
                    },
                )?;
            }
            Ok("ken_burns".to_string())
        }
        "crop" => {
            for rect in trim_rects.iter().chain(pan_rects.iter()) {
                validate_inactive_crop_rect(*rect, rect.tag_name().name())?;
            }
            let animated = crop_rects
                .first()
                .map(|rect| validate_active_crop_rect(*rect, asset_format, "crop-rect", keys))
                .transpose()?
                .unwrap_or(false);
            Ok(if animated {
                "crop_animated"
            } else {
                "crop_static"
            }
            .to_string())
        }
        "trim" => {
            for rect in crop_rects.iter().chain(pan_rects.iter()) {
                validate_inactive_crop_rect(*rect, rect.tag_name().name())?;
            }
            let animated = trim_rects
                .first()
                .map(|rect| validate_active_crop_rect(*rect, asset_format, "trim-rect", keys))
                .transpose()?
                .unwrap_or(false);
            Ok(if animated {
                "trim_animated"
            } else {
                "trim_static"
            }
            .to_string())
        }
        _ => unreachable!(),
    }
}

fn geometry_preflight(
    document: &Document<'_>,
    clip: Node<'_, '_>,
) -> Result<GeometryPreflight, RouteDError> {
    for scope in geometry_scopes(clip) {
        for child in scope.children().filter(Node::is_element) {
            match child.tag_name().name() {
                "adjust-corners" => {
                    return Err(RouteDError::new(
                        "corner/perspective geometry is outside ScaleTranslate support",
                    ));
                }
                "adjust-360-transform"
                | "adjust-reorient"
                | "adjust-orientation"
                | "adjust-cinematic"
                | "adjust-stereo-3D"
                | "object-tracker" => {
                    return Err(RouteDError::new(format!(
                        "{} geometry is outside verified ScaleTranslate support",
                        child.tag_name().name()
                    )));
                }
                _ => {}
            }
        }
    }
    let asset_ref = clip_asset_ref(clip)?;
    let asset = resource_by_id(document, "asset", asset_ref)?;
    let asset_format = canonical_format(document, asset.attribute("format"), "asset")?;
    let sequence = clip.ancestors().find(|node| node.has_tag_name("sequence"));
    let sequence_format = canonical_format(
        document,
        sequence.and_then(|node| node.attribute("format")),
        "sequence",
    )?;
    let mut geometry_parameter_keys = GeometryParameterKeys::new();
    let conform = conform_detail(clip)?;
    let transform = transform_detail(clip, &mut geometry_parameter_keys)?;
    let crop = crop_detail(clip, asset_format, &mut geometry_parameter_keys)?;
    let format_detail = |format: Option<CanonicalFormat>| {
        format
            .map(CanonicalFormat::detail)
            .unwrap_or_else(|| "unknown".to_string())
    };
    Ok(GeometryPreflight {
        status: GeometryStatus::RuntimeLive,
        reasons: Vec::new(),
        detail: format!(
            "asset_format={}; sequence_format={}; source_orientation=host_applied_or_unknown; conform={conform}; transform={transform}; crop={crop}",
            format_detail(asset_format),
            format_detail(sequence_format),
        ),
    })
}

fn apply_replacements(
    input: &str,
    mut replacements: Vec<Replacement>,
) -> Result<String, RouteDError> {
    replacements.sort_by_key(|replacement| replacement.range.start);
    for window in replacements.windows(2) {
        if window[0].range.end > window[1].range.start {
            return Err(RouteDError::new("FCPXML batch patch ranges overlap"));
        }
    }
    let mut output = input.to_string();
    for replacement in replacements.into_iter().rev() {
        output.replace_range(replacement.range, &replacement.value);
    }
    Ok(output)
}

fn parameter_with_production_id<'a, 'input>(
    filter: Node<'a, 'input>,
    parameter_id: u32,
) -> Vec<Node<'a, 'input>> {
    let expected_key = format!("{PROJECT_PARAMETER_KEY_PREFIX}/{parameter_id}");
    filter
        .children()
        .filter(|node| {
            node.has_tag_name("param") && node.attribute("key") == Some(expected_key.as_str())
        })
        .collect()
}

fn validate_global_resource_ids(document: &Document<'_>) -> Result<(), RouteDError> {
    let resources: Vec<_> = document
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("resources"))
        .collect();
    if resources.len() != 1 {
        return Err(RouteDError::unsafe_structure(format!(
            "expected exactly one global resources table, found {}",
            resources.len()
        )));
    }
    let mut seen = HashMap::new();
    for resource in resources[0].children().filter(Node::is_element) {
        let Some(identifier) = resource.attribute("id") else {
            continue;
        };
        if let Some(previous_tag) = seen.insert(identifier, resource.tag_name().name()) {
            return Err(RouteDError::unsafe_structure(format!(
                "duplicate global resource id {identifier} is used by {previous_tag} and {}",
                resource.tag_name().name()
            )));
        }
    }
    Ok(())
}

fn validate_global_reserved_parameter_keys(filters: &[Node<'_, '_>]) -> Result<(), RouteDError> {
    for (index, filter) in filters.iter().enumerate() {
        let mut seen = HashSet::new();
        for parameter in filter.children().filter(|node| node.has_tag_name("param")) {
            let Some(key) = parameter.attribute("key") else {
                continue;
            };
            if reserved_project_parameter_id_from_key(key).is_some()
                && !seen.insert(key.to_string())
            {
                return Err(RouteDError::unsafe_structure(format!(
                    "NiYien occurrence {} has duplicate reserved parameter key {key}",
                    index + 1
                )));
            }
        }
    }
    Ok(())
}

fn visible_parameter_values(parameters: &super::GFRenderParameters) -> [String; 7] {
    [
        parameters.fov.to_string(),
        parameters.smoothness.to_string(),
        parameters.lens_correction.to_string(),
        parameters.horizon_lock_amount.to_string(),
        parameters.horizon_lock_roll.to_string(),
        parameters.zoom_mode.to_string(),
        parameters.overview.to_string(),
    ]
}

fn updated_project_parameters(
    input: &[u8],
    filter: Node<'_, '_>,
    occurrence: usize,
    sibling: &SiblingProject,
) -> Result<(Vec<Replacement>, String), RouteDError> {
    let mut replacements = Vec::new();
    let mut ranges = Vec::new();
    let replaced_ids: HashSet<u32> = [
        1901_u32, 1902, 1904, 1905, 1906, 1910, 1911, 1912, 1913, 1914, 1915, 1916, 1917, 1918,
        1919, 1930, 1931, 1932, 1933, 1934, 1935, 1936, 1937, 1938, 1939, 2001, 2002, 2003, 2004,
        2005, 2006, 2007,
    ]
    .into_iter()
    .collect();
    for parameter in filter.children().filter(|node| node.has_tag_name("param")) {
        let known_by_key = parameter
            .attribute("key")
            .and_then(reserved_project_parameter_id_from_key)
            .is_some_and(|id| replaced_ids.contains(&id));
        if known_by_key {
            ranges.push(parameter.range());
        }
    }
    for range in ranges {
        replacements.push(Replacement {
            range,
            value: String::new(),
        });
    }

    let identity = route_d_instance_identity(input, filter.range().start);
    let mut appended =
        encoded_project_banks(&sibling.payload, PROJECT_PARAMETER_KEY_PREFIX, &identity)?;
    appended.push_str(&format!(
        "<param name=\"Project Display Name\" key=\"{PROJECT_PARAMETER_KEY_PREFIX}/1906\" value=\"{}\"/>",
        xml_attribute_escape(&sibling.display_name)
    ));
    let values = visible_parameter_values(&sibling.parameters);
    for ((canonical_name, parameter_id), value) in VISIBLE_PARAMETER_IDS.iter().zip(values.iter()) {
        let existing = parameter_with_production_id(filter, *parameter_id);
        if existing.len() > 1 {
            return Err(RouteDError::new(format!(
                "NiYien occurrence {occurrence} has duplicate /{parameter_id} parameters"
            )));
        }
        let name = existing
            .first()
            .and_then(|node| node.attribute("name"))
            .unwrap_or(canonical_name);
        appended.push_str(&format!(
            "<param name=\"{}\" key=\"{PROJECT_PARAMETER_KEY_PREFIX}/{parameter_id}\" value=\"{}\"/>",
            xml_attribute_escape(name),
            xml_attribute_escape(value)
        ));
    }
    Ok((replacements, appended))
}

fn timing_replacements(
    document: &Document<'_>,
    input: &str,
    project: Node<'_, '_>,
    filter: Node<'_, '_>,
    occurrence: usize,
    appended: &mut String,
) -> Result<Vec<Replacement>, RouteDError> {
    let version = document
        .root_element()
        .attribute("version")
        .ok_or_else(|| RouteDError::new("FCPXML version is missing"))?;
    let resolved = resolve_clip(document, project, filter, version)?;
    let timing_parameters = parameter_with_production_id(filter, 1903);
    if timing_parameters.len() > 1 {
        return Err(RouteDError::new(format!(
            "NiYien occurrence {occurrence} has multiple Timing Payload parameters"
        )));
    }
    let bounds = Bounds {
        start: rational_string(&resolved.bounds_start),
        duration: rational_string(&resolved.duration),
    };
    let payload = TimingPayload {
        version: TIMING_PAYLOAD_VERSION,
        fcpxml_version: version.to_string(),
        occurrence,
        structure_sha256: resolved.structure_sha256,
        asset_ref: resolved.asset_ref,
        mapping: resolved.mapping,
        effect_bounds: bounds.clone(),
        input_bounds: bounds,
        render_ready_snapshot: true,
    };
    let payload = serde_json::to_vec(&payload)
        .map(|bytes| STANDARD.encode(bytes))
        .map_err(|error| RouteDError::new(format!("timing payload encoding failed: {error}")))?;
    if let Some(parameter) = timing_parameters.first() {
        let value = parameter.attribute_node("value").ok_or_else(|| {
            RouteDError::new(format!(
                "NiYien occurrence {occurrence} Timing Payload has no value"
            ))
        })?;
        Ok(vec![Replacement {
            range: attribute_value_range(input, value)?,
            value: payload,
        }])
    } else {
        appended.push_str(&format!(
            "<param name=\"Timing Payload\" key=\"{PROJECT_PARAMETER_KEY_PREFIX}/1903\" value=\"{}\"/>",
            xml_attribute_escape(&payload)
        ));
        Ok(Vec::new())
    }
}

fn skipped_report(
    occurrence: usize,
    clip_name: String,
    asset_ref: Option<String>,
    error: SiblingProjectError,
) -> BatchTargetReport {
    BatchTargetReport {
        occurrence,
        clip_name,
        asset_ref,
        media_url: error.media_url,
        expected_project_path: error.expected_path.map(|path| path.display().to_string()),
        project_display_name: error.display_name,
        action: BatchTargetAction::Skipped,
        skip_reason: Some(error.reason),
        detail: error.detail,
        geometry_status: None,
        geometry_reasons: Vec::new(),
        geometry_detail: None,
    }
}

fn local_skip_report(
    occurrence: usize,
    clip_name: String,
    asset_ref: Option<String>,
    reason: BatchSkipReason,
    detail: String,
) -> BatchTargetReport {
    BatchTargetReport {
        occurrence,
        clip_name,
        asset_ref,
        media_url: None,
        expected_project_path: None,
        project_display_name: None,
        action: BatchTargetAction::Skipped,
        skip_reason: Some(reason),
        detail,
        geometry_status: None,
        geometry_reasons: Vec::new(),
        geometry_detail: None,
    }
}

fn enrich_report_with_sibling(
    mut report: BatchTargetReport,
    sibling: &SiblingProject,
) -> BatchTargetReport {
    report.media_url = Some(sibling.media_url.clone());
    report.expected_project_path = Some(sibling.expected_path.display().to_string());
    report.project_display_name = Some(sibling.display_name.clone());
    report
}

fn unproved_project_ancestor(clip: Node<'_, '_>, project: Node<'_, '_>) -> Option<String> {
    let ancestors: Vec<_> = clip.ancestors().skip(1).collect();
    let boundary = ancestors
        .iter()
        .position(|ancestor| *ancestor == project)
        .or_else(|| {
            ancestors.iter().position(|ancestor| {
                ancestor.has_tag_name("media") && ancestor.attribute("id").is_some()
            })
        });
    let Some(boundary) = boundary else {
        return Some("unreachable project wrapper".to_string());
    };
    let path: Vec<_> = ancestors[..boundary]
        .iter()
        .map(|ancestor| ancestor.tag_name().name())
        .collect();
    if path == ["spine", "sequence"] {
        None
    } else {
        Some(
            path.first()
                .copied()
                .unwrap_or("unproved direct project wrapper")
                .to_string(),
        )
    }
}

fn plan_existing_occurrence(
    document: &Document<'_>,
    input: &str,
    project: Node<'_, '_>,
    filter: Node<'_, '_>,
    occurrence: usize,
) -> Result<TargetPatchPlan, BatchTargetReport> {
    plan_existing_occurrence_with_reader(
        document,
        input,
        project,
        filter,
        occurrence,
        &read_project_file,
    )
}

fn plan_existing_occurrence_with_reader(
    document: &Document<'_>,
    input: &str,
    project: Node<'_, '_>,
    filter: Node<'_, '_>,
    occurrence: usize,
    project_reader: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> Result<TargetPatchPlan, BatchTargetReport> {
    let clip = effect_clip_container(filter).map_err(|error| {
        local_skip_report(
            occurrence,
            format!("Occurrence {occurrence}"),
            None,
            BatchSkipReason::UnsupportedStructure,
            error.to_string(),
        )
    })?;
    let asset_ref = clip_asset_ref(clip).map_err(|error| {
        local_skip_report(
            occurrence,
            clip.attribute("name").unwrap_or("Unnamed clip").to_string(),
            None,
            BatchSkipReason::UnsupportedStructure,
            error.to_string(),
        )
    })?;
    let clip_name = clip_name(clip, asset_ref);
    if let Some(container) = unproved_project_ancestor(clip, project) {
        return Err(local_skip_report(
            occurrence,
            clip_name,
            Some(asset_ref.to_string()),
            BatchSkipReason::UnsupportedStructure,
            format!("NiYien occurrence {occurrence} is nested below unsupported {container}"),
        ));
    }
    let sibling = sibling_project_for_asset_with_reader(document, asset_ref, project_reader)
        .map_err(|error| {
            skipped_report(
                occurrence,
                clip_name.clone(),
                Some(asset_ref.to_string()),
                error,
            )
        })?;
    let geometry = geometry_preflight(document, clip).map_err(|error| {
        enrich_report_with_sibling(
            local_skip_report(
                occurrence,
                clip_name.clone(),
                Some(asset_ref.to_string()),
                BatchSkipReason::BlockedGeometry,
                error.to_string(),
            ),
            &sibling,
        )
    })?;
    let current = selected_valid_project_payload(filter, occurrence).map_err(|error| {
        enrich_report_with_sibling(
            local_skip_report(
                occurrence,
                clip_name.clone(),
                Some(asset_ref.to_string()),
                BatchSkipReason::UnsupportedStructure,
                error.to_string(),
            ),
            &sibling,
        )
    })?;
    let same_hash = current
        .as_ref()
        .and_then(|candidate| super::decode_project_payload(candidate.payload.as_bytes()).ok())
        .is_some_and(|bytes| Sha256::digest(bytes) == Sha256::digest(&sibling.bytes));

    let mut replacements = Vec::new();
    let mut appended = String::new();
    let (action, detail) = if same_hash {
        let display = parameter_with_production_id(filter, 1906);
        if display.len() > 1 {
            return Err(enrich_report_with_sibling(
                local_skip_report(
                    occurrence,
                    clip_name,
                    Some(asset_ref.to_string()),
                    BatchSkipReason::UnsupportedStructure,
                    format!("NiYien occurrence {occurrence} has duplicate /1906 parameters"),
                ),
                &sibling,
            ));
        }
        if display.is_empty() {
            appended.push_str(&format!(
                "<param name=\"Project Display Name\" key=\"{PROJECT_PARAMETER_KEY_PREFIX}/1906\" value=\"{}\"/>",
                xml_attribute_escape(&sibling.display_name)
            ));
        } else if let Some(value) = display[0].attribute_node("value")
            && value.value().is_empty()
        {
            replacements.push(Replacement {
                range: attribute_value_range(input, value).map_err(|error| {
                    enrich_report_with_sibling(
                        local_skip_report(
                            occurrence,
                            clip_name.clone(),
                            Some(asset_ref.to_string()),
                            BatchSkipReason::UnsupportedStructure,
                            error.to_string(),
                        ),
                        &sibling,
                    )
                })?,
                value: xml_attribute_escape(&sibling.display_name),
            });
        }
        (
            BatchTargetAction::TimingOnly,
            "Exact sibling matches the selected project payload; refreshed timing only".to_string(),
        )
    } else {
        let (parameter_replacements, parameters) =
            updated_project_parameters(input.as_bytes(), filter, occurrence, &sibling).map_err(
                |error| {
                    enrich_report_with_sibling(
                        local_skip_report(
                            occurrence,
                            clip_name.clone(),
                            Some(asset_ref.to_string()),
                            BatchSkipReason::UnsupportedStructure,
                            error.to_string(),
                        ),
                        &sibling,
                    )
                },
            )?;
        replacements.extend(parameter_replacements);
        appended.push_str(&parameters);
        (
            BatchTargetAction::UpdatedProject,
            "Replaced project payload and visible parameters from the exact sibling".to_string(),
        )
    };
    replacements.extend(
        timing_replacements(document, input, project, filter, occurrence, &mut appended).map_err(
            |error| {
                enrich_report_with_sibling(
                    local_skip_report(
                        occurrence,
                        clip_name.clone(),
                        Some(asset_ref.to_string()),
                        BatchSkipReason::InvalidTiming,
                        error.to_string(),
                    ),
                    &sibling,
                )
            },
        )?,
    );
    if !appended.is_empty() {
        replacements.push(
            filter_children_replacement(input, filter, appended).map_err(|error| {
                enrich_report_with_sibling(
                    local_skip_report(
                        occurrence,
                        clip_name.clone(),
                        Some(asset_ref.to_string()),
                        BatchSkipReason::UnsupportedStructure,
                        error.to_string(),
                    ),
                    &sibling,
                )
            })?,
        );
    }
    Ok(TargetPatchPlan {
        replacements,
        report: BatchTargetReport {
            occurrence,
            clip_name,
            asset_ref: Some(asset_ref.to_string()),
            media_url: Some(sibling.media_url),
            expected_project_path: Some(sibling.expected_path.display().to_string()),
            project_display_name: Some(sibling.display_name),
            action,
            skip_reason: None,
            detail,
            geometry_status: Some(geometry.status),
            geometry_reasons: geometry.reasons,
            geometry_detail: Some(geometry.detail),
        },
    })
}

fn patch_fcpxml_project_batch_with_reader(
    input: &[u8],
    project_reader: Option<&dyn Fn(&Path) -> io::Result<Vec<u8>>>,
    report_all_skipped: bool,
) -> Result<BatchRouteDPatchResult, RouteDError> {
    let input_text = std::str::from_utf8(input)
        .map_err(|error| RouteDError::new(format!("FCPXML is not UTF-8: {error}")))?;
    let document = Document::parse_with_options(
        input_text,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .map_err(|error| RouteDError::new(format!("FCPXML parse failed: {error}")))?;
    let root = document.root_element();
    if !root.has_tag_name("fcpxml") {
        return Err(RouteDError::new("document root is not fcpxml"));
    }
    let version = root
        .attribute("version")
        .ok_or_else(|| RouteDError::new("FCPXML version is missing"))?;
    if !SUPPORTED_FCPXML_VERSIONS.contains(&version) {
        return Err(RouteDError::new(format!(
            "unsupported FCPXML version {version}"
        )));
    }
    validate_global_resource_ids(&document)?;
    let projects: Vec<_> = root
        .descendants()
        .filter(|node| node.has_tag_name("project"))
        .collect();
    if projects.len() != 1 {
        return Err(RouteDError::new(format!(
            "expected exactly one project, found {}",
            projects.len()
        )));
    }
    let project = projects[0];
    let original_project_name = project
        .attribute("name")
        .ok_or_else(|| RouteDError::new("project name is missing"))?
        .to_string();
    let original_project_uid = project
        .attribute("uid")
        .ok_or_else(|| RouteDError::new("project UID is missing"))?
        .to_string();
    let effect_resources: Vec<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("effect")
                && matches!(
                    node.attribute("uid"),
                    Some(EFFECT_UUID | EFFECT_TEMPLATE_UID)
                )
        })
        .collect();
    let effect_ref = match effect_resources.as_slice() {
        [effect] => {
            let effect_ref = effect.attribute("id").ok_or_else(|| {
                RouteDError::new("production NiYien effect resource id is missing")
            })?;
            if document
                .descendants()
                .filter(|node| node.attribute("id") == Some(effect_ref))
                .count()
                != 1
            {
                return Err(RouteDError::unsafe_structure(
                    "production NiYien effect resource id is not unique",
                ));
            }
            effect_ref.to_string()
        }
        [] => {
            return Err(RouteDError::no_updateable_targets(
                "project has no production NiYien effect resource",
            ));
        }
        _ => {
            return Err(RouteDError::unsafe_structure(format!(
                "expected exactly one production NiYien effect resource, found {}",
                effect_resources.len()
            )));
        }
    };

    let mut targets = Vec::new();
    let existing_filters: Vec<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("filter-video") && node.attribute("ref") == Some(effect_ref.as_str())
        })
        .filter(|filter| {
            filter.ancestors().any(|node| node == project)
                || filter.ancestors().any(|node| node.has_tag_name("media"))
        })
        .collect();
    validate_global_reserved_parameter_keys(&existing_filters)?;
    if existing_filters.is_empty() {
        return Err(RouteDError::no_updateable_targets(
            "project has no updateable existing NiYien effect targets",
        ));
    }
    let mut replacements = Vec::new();
    for (index, filter) in existing_filters.iter().copied().enumerate() {
        let plan = match project_reader {
            Some(project_reader) => plan_existing_occurrence_with_reader(
                &document,
                input_text,
                project,
                filter,
                index + 1,
                project_reader,
            ),
            None => plan_existing_occurrence(&document, input_text, project, filter, index + 1),
        };
        match plan {
            Ok(plan) => {
                replacements.extend(plan.replacements);
                targets.push(plan.report);
            }
            Err(report) => targets.push(report),
        }
    }
    if targets
        .iter()
        .all(|target| target.action == BatchTargetAction::Skipped)
    {
        if report_all_skipped {
            return Ok(BatchRouteDPatchResult {
                xml: input.to_vec(),
                original_project_name,
                occurrence_count: existing_filters.len(),
                updated_project_count: 0,
                timing_only_count: 0,
                skipped_count: targets.len(),
                targets,
            });
        }
        return Err(RouteDError::no_updateable_targets(
            "batch has no updateable targets",
        ));
    }
    let intermediate = apply_replacements(input_text, replacements)?;
    if intermediate.as_bytes() == input {
        return Err(RouteDError::no_updateable_targets(
            "batch produced no XML changes",
        ));
    }
    let output_document = Document::parse_with_options(
        &intermediate,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .map_err(|error| RouteDError::new(format!("patched FCPXML parse failed: {error}")))?;
    let output_projects: Vec<_> = output_document
        .descendants()
        .filter(|node| node.has_tag_name("project"))
        .collect();
    if output_projects.len() != 1
        || output_projects[0].attribute("name") != Some(original_project_name.as_str())
        || output_projects[0].attribute("uid") != Some(original_project_uid.as_str())
    {
        return Err(RouteDError::new(
            "batch patch did not preserve the original project name and UID",
        ));
    }
    let updated_project_count = targets
        .iter()
        .filter(|target| target.action == BatchTargetAction::UpdatedProject)
        .count();
    let timing_only_count = targets
        .iter()
        .filter(|target| target.action == BatchTargetAction::TimingOnly)
        .count();
    let skipped_count = targets
        .iter()
        .filter(|target| target.action == BatchTargetAction::Skipped)
        .count();
    Ok(BatchRouteDPatchResult {
        xml: intermediate.into_bytes(),
        original_project_name,
        occurrence_count: existing_filters.len(),
        updated_project_count,
        timing_only_count,
        skipped_count,
        targets,
    })
}

pub fn patch_fcpxml_project_batch(input: &[u8]) -> Result<BatchRouteDPatchResult, RouteDError> {
    patch_fcpxml_project_batch_with_reader(input, None, false)
}

pub fn patch_fcpxml_project_batch_with_media_roots(
    input: &[u8],
    media_roots: &[PathBuf],
) -> Result<BatchRouteDPatchResult, RouteDError> {
    let roots = canonical_authorized_roots(media_roots)?;
    let reader = |path: &Path| read_authorized_project_file(path, &roots);
    patch_fcpxml_project_batch_with_reader(input, Some(&reader), false)
}

#[doc(hidden)]
pub fn patch_fcpxml_project_batch_with_project_reader(
    input: &[u8],
    project_reader: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> Result<BatchRouteDPatchResult, RouteDError> {
    patch_fcpxml_project_batch_with_reader(input, Some(project_reader), false)
}

#[doc(hidden)]
pub fn patch_fcpxml_project_batch_with_project_reader_report_all_skipped(
    input: &[u8],
    project_reader: &dyn Fn(&Path) -> io::Result<Vec<u8>>,
) -> Result<BatchRouteDPatchResult, RouteDError> {
    patch_fcpxml_project_batch_with_reader(input, Some(project_reader), true)
}

fn import_token(input: &[u8]) -> String {
    let mut bytes: [u8; 16] = Sha256::digest(input)[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub fn patch_fcpxml_project(
    input: &[u8],
    requested_name: Option<&str>,
) -> Result<RouteDPatchResult, RouteDError> {
    let input_text = std::str::from_utf8(input)
        .map_err(|error| RouteDError::new(format!("FCPXML is not UTF-8: {error}")))?;
    let document = Document::parse_with_options(
        input_text,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
    .map_err(|error| RouteDError::new(format!("FCPXML parse failed: {error}")))?;
    let root = document.root_element();
    if !root.has_tag_name("fcpxml") {
        return Err(RouteDError::new("document root is not fcpxml"));
    }
    let version = root
        .attribute("version")
        .ok_or_else(|| RouteDError::new("FCPXML version is missing"))?;
    if !SUPPORTED_FCPXML_VERSIONS.contains(&version) {
        return Err(RouteDError::new(format!(
            "unsupported FCPXML version {version}"
        )));
    }

    let projects: Vec<_> = root
        .descendants()
        .filter(|node| node.has_tag_name("project"))
        .collect();
    if projects.len() != 1 {
        return Err(RouteDError::new(format!(
            "expected exactly one project, found {}",
            projects.len()
        )));
    }
    let project = projects[0];
    let original_project_name = project
        .attribute("name")
        .ok_or_else(|| RouteDError::new("project name is missing"))?
        .to_string();
    let processed_project_name = requested_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{original_project_name} — Gyroflow Processed"));
    let import_token = import_token(input);

    let effect_refs: HashSet<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("effect")
                && matches!(
                    node.attribute("uid"),
                    Some(EFFECT_UUID | EFFECT_TEMPLATE_UID)
                )
        })
        .filter_map(|node| node.attribute("id").map(str::to_string))
        .collect();
    if effect_refs.is_empty() {
        return Err(RouteDError::new(
            "project has no NiYien effect resource with a production identity",
        ));
    }

    let all_filters: Vec<_> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name("filter-video")
                && node
                    .attribute("ref")
                    .is_some_and(|reference| effect_refs.contains(reference))
        })
        .collect();
    if all_filters.is_empty() {
        return Err(RouteDError::new("project has no NiYien effect occurrences"));
    }

    let mut replacements = Vec::new();
    let mut reachable_occurrences = 0;
    let mut clip_cache: HashMap<usize, ResolvedClip> = HashMap::new();
    for filter in all_filters {
        let directly_reachable = filter.ancestors().any(|node| node == project);
        let in_media = filter.ancestors().any(|node| node.has_tag_name("media"));
        if !directly_reachable && !in_media {
            continue;
        }
        reachable_occurrences += 1;
        let clip = effect_clip_container(filter)?;
        let resolved = if let Some(resolved) = clip_cache.get(&(clip.id().get() as usize)) {
            resolved
        } else {
            let resolved = resolve_clip(&document, project, filter, version)?;
            clip_cache.insert(clip.id().get() as usize, resolved);
            clip_cache.get(&(clip.id().get() as usize)).unwrap()
        };
        let has_banked_manifest = filter.children().any(|node| {
            node.has_tag_name("param")
                && matches!(
                    node.attribute("name"),
                    Some("Project Payload Manifest A" | "Project Payload Manifest B")
                )
                && node
                    .attribute("value")
                    .is_some_and(|value| !value.is_empty())
        });
        let verified_banked_prefix = has_banked_manifest
            .then(|| project_payload_key_prefix(filter, reachable_occurrences))
            .transpose()?;
        let timing_params: Vec<_> = filter
            .children()
            .filter(|node| {
                node.has_tag_name("param") && node.attribute("name") == Some("Timing Payload")
            })
            .collect();
        if timing_params.len() > 1 {
            return Err(RouteDError::new(format!(
                "NiYien occurrence {reachable_occurrences} has {} Timing Payload parameters",
                timing_params.len()
            )));
        }
        let value_attribute = timing_params
            .first()
            .map(|parameter| {
                parameter.attribute_node("value").ok_or_else(|| {
                    RouteDError::new(format!(
                        "NiYien occurrence {reachable_occurrences} Timing Payload has no value"
                    ))
                })
            })
            .transpose()?;
        let bounds = Bounds {
            start: rational_string(&resolved.bounds_start),
            duration: rational_string(&resolved.duration),
        };
        let payload = TimingPayload {
            version: TIMING_PAYLOAD_VERSION,
            fcpxml_version: version.to_string(),
            occurrence: reachable_occurrences,
            structure_sha256: resolved.structure_sha256.clone(),
            asset_ref: resolved.asset_ref.clone(),
            mapping: resolved.mapping.clone(),
            effect_bounds: bounds.clone(),
            input_bounds: bounds,
            render_ready_snapshot: true,
        };
        let payload_bytes = serde_json::to_vec(&payload).map_err(|error| {
            RouteDError::new(format!("timing payload encoding failed: {error}"))
        })?;
        let encoded_payload = STANDARD.encode(payload_bytes);
        if let Some(value_attribute) = value_attribute {
            replacements.push(Replacement {
                range: attribute_value_range(input_text, value_attribute)?,
                value: encoded_payload,
            });
        } else {
            let key_prefix = if let Some(prefix) = verified_banked_prefix {
                prefix
            } else {
                project_payload_key_prefix(filter, reachable_occurrences)?
            };
            let filter_range = filter.range();
            let closing_offset = input_text[filter_range.clone()]
                .rfind("</filter-video>")
                .map(|offset| filter_range.start + offset)
                .ok_or_else(|| {
                    RouteDError::new(format!(
                        "NiYien occurrence {reachable_occurrences} filter-video is not expandable"
                    ))
                })?;
            replacements.push(Replacement {
                range: closing_offset..closing_offset,
                value: format!(
                    "<param name=\"Timing Payload\" key=\"{key_prefix}/1903\" value=\"{}\"/>",
                    xml_attribute_escape(&encoded_payload)
                ),
            });
        }
    }
    if reachable_occurrences == 0 {
        return Err(RouteDError::new(
            "no NiYien effect occurrence is reachable from the project",
        ));
    }

    let name_attribute = project
        .attribute_node("name")
        .ok_or_else(|| RouteDError::new("project name attribute is missing"))?;
    replacements.push(Replacement {
        range: attribute_value_range(input_text, name_attribute)?,
        value: xml_attribute_escape(&processed_project_name),
    });
    let uid_attribute = project
        .attribute_node("uid")
        .ok_or_else(|| RouteDError::new("project UID attribute is missing"))?;
    replacements.push(Replacement {
        range: attribute_value_range(input_text, uid_attribute)?,
        value: import_token.clone(),
    });

    replacements.sort_by_key(|replacement| replacement.range.start);
    for window in replacements.windows(2) {
        if window[0].range.end > window[1].range.start {
            return Err(RouteDError::new("FCPXML patch ranges overlap"));
        }
    }
    let mut output = input_text.to_string();
    for replacement in replacements.into_iter().rev() {
        output.replace_range(replacement.range, &replacement.value);
    }
    Ok(RouteDPatchResult {
        xml: output.into_bytes(),
        original_project_name,
        processed_project_name,
        import_token,
        occurrence_count: reachable_occurrences,
    })
}
