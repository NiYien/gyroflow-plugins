// SPDX-License-Identifier: GPL-3.0-or-later

//! Parser and validator for the vertical-timeline sizing field stored in a DaVinci Resolve DRT export.
//!
//! The DRT is a zip whose `MediaPool/Master/MpFolder.xml` holds, per timeline, a `FieldsBlob`
//! (hex of a Qt `QDataStream` `QVariantMap`). Its `SequenceSetup` byte array carries the
//! resolution and the two mismatch modes that the scripting API does not expose for vertical timelines.

use std::collections::HashMap;
use std::io::{ Cursor, Read };

const MP_FOLDER_ENTRY: &str = "MediaPool/Master/MpFolder.xml";
const SETUP_MIN_LEN: usize = 116;
const OFF_WIDTH: usize = 44;
const OFF_HEIGHT: usize = 52;
const OFF_HORIZONTAL: usize = 96;
const OFF_VERTICAL: usize = 112;

/// Values the scripting API reports for the same timeline, used to cross-check the DRT.
pub struct ApiSizingReading<'a> {
    pub timeline_name: &'a str,
    pub width: usize,
    pub height: usize,
    pub horizontal_mode: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrtSizing {
    pub width: usize,
    pub height: usize,
    pub horizontal_raw: i32,
    pub vertical_raw: i32,
    pub effective_mode: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DrtRejection {
    Zip(String),
    TimelineNotFound,
    TimelineAmbiguous(usize),
    BlobDecode(String),
    SetupMissing,
    SetupTooShort(usize),
    ResolutionMismatch { drt: (usize, usize), api: (usize, usize) },
    HorizontalMismatch { drt: i32, api: String },
    VerticalOutOfRange(i32),
    NotVertical,
}

impl DrtRejection {
    pub fn reason(&self) -> String {
        match self {
            Self::Zip(m) => format!("zip({m})"),
            Self::TimelineNotFound => "timeline-not-found".into(),
            Self::TimelineAmbiguous(n) => format!("timeline-ambiguous({n})"),
            Self::BlobDecode(m) => format!("blob-decode({m})"),
            Self::SetupMissing => "setup-missing".into(),
            Self::SetupTooShort(n) => format!("setup-too-short({n})"),
            Self::ResolutionMismatch { drt, api } => format!("resolution-mismatch(drt={}x{} api={}x{})", drt.0, drt.1, api.0, api.1),
            Self::HorizontalMismatch { drt, api } => format!("horizontal-mismatch(drt={drt} api={api})"),
            Self::VerticalOutOfRange(v) => format!("vertical-out-of-range({v})"),
            Self::NotVertical => "not-vertical".into(),
        }
    }
}

/// Maps the raw SequenceSetup mismatch encoding to the scripting API mode names.
pub fn decode_mode(raw: i32) -> Option<&'static str> {
    match raw {
        -1 => Some("scaleToFit"),
        0 => Some("centerCrop"),
        1 => Some("scaleToCrop"),
        2 => Some("stretch"),
        _ => None,
    }
}

/// Reads the effective input sizing of a vertical timeline from a DRT, validated against the API reading.
pub fn effective_input_sizing(drt: &[u8], reading: &ApiSizingReading) -> Result<DrtSizing, DrtRejection> {
    let xml = read_folder_xml(drt)?;
    let hex = find_fields_blob(&xml, reading.timeline_name)?;
    let blob = decode_hex(&hex).map_err(DrtRejection::BlobDecode)?;
    let setup = extract_setup(&blob)?;
    if setup.len() < SETUP_MIN_LEN {
        return Err(DrtRejection::SetupTooShort(setup.len()));
    }
    let be = |off: usize| i32::from_be_bytes([setup[off], setup[off + 1], setup[off + 2], setup[off + 3]]);
    let width = be(OFF_WIDTH).max(0) as usize;
    let height = be(OFF_HEIGHT).max(0) as usize;
    let horizontal_raw = be(OFF_HORIZONTAL);
    let vertical_raw = be(OFF_VERTICAL);

    if (width, height) != (reading.width, reading.height) {
        return Err(DrtRejection::ResolutionMismatch { drt: (width, height), api: (reading.width, reading.height) });
    }
    if decode_mode(horizontal_raw) != Some(reading.horizontal_mode) {
        return Err(DrtRejection::HorizontalMismatch { drt: horizontal_raw, api: reading.horizontal_mode.to_string() });
    }
    let effective_mode = decode_mode(vertical_raw).ok_or(DrtRejection::VerticalOutOfRange(vertical_raw))?;
    if height <= width {
        return Err(DrtRejection::NotVertical);
    }
    Ok(DrtSizing { width, height, horizontal_raw, vertical_raw, effective_mode })
}

fn read_folder_xml(drt: &[u8]) -> Result<String, DrtRejection> {
    let mut archive = zip::ZipArchive::new(Cursor::new(drt)).map_err(|e| DrtRejection::Zip(e.to_string()))?;
    let mut file = archive.by_name(MP_FOLDER_ENTRY).map_err(|e| DrtRejection::Zip(e.to_string()))?;
    let mut xml = String::new();
    file.read_to_string(&mut xml).map_err(|e| DrtRejection::Zip(e.to_string()))?;
    Ok(xml)
}

fn find_fields_blob(xml: &str, timeline_name: &str) -> Result<String, DrtRejection> {
    // Resolve emits `ListMgt::Tag` element names, which namespace-aware parsers reject.
    let prefix_re = regex::Regex::new(r"(</?)([A-Za-z0-9_]+)::").unwrap();
    let normalized = prefix_re.replace_all(xml, "${1}${2}__");
    let doc = roxmltree::Document::parse(&normalized).map_err(|e| DrtRejection::Zip(e.to_string()))?;

    let matches: Vec<_> = doc.descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Sm2Timeline")
        .filter(|n| child(*n, "Name").and_then(|name| name.text()) == Some(timeline_name))
        .collect();
    match matches.len() {
        0 => return Err(DrtRejection::TimelineNotFound),
        1 => {}
        n => return Err(DrtRejection::TimelineAmbiguous(n)),
    }
    child(matches[0], "Sequence")
        .and_then(|s| child(s, "Sm2Sequence"))
        .and_then(|s| child(s, "FieldsBlob"))
        .and_then(|b| b.text())
        .map(|t| t.to_string())
        .ok_or(DrtRejection::SetupMissing)
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == tag)
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if digits.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    let nibble = |b: u8| (b as char).to_digit(16).map(|d| d as u8).ok_or_else(|| format!("invalid hex digit {:?}", b as char));
    digits.chunks(2).map(|p| Ok(nibble(p[0])? << 4 | nibble(p[1])?)).collect()
}

/// Minimal big-endian QDataStream reader.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.data.len()).ok_or_else(|| "unexpected end of data".to_string())?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, String> { Ok(self.take(1)?[0]) }
    fn u32(&mut self) -> Result<u32, String> { Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    fn string(&mut self) -> Result<String, String> {
        let len = self.u32()?;
        if len == u32::MAX {
            return Ok(String::new());
        }
        let bytes = self.take(len as usize)?;
        if bytes.len() % 2 != 0 {
            return Err("odd UTF-16 byte length".into());
        }
        let units: Vec<u16> = bytes.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        Ok(String::from_utf16_lossy(&units))
    }
    /// Reads a variant; returns the byte array payload when it is one.
    fn variant(&mut self) -> Result<Option<Vec<u8>>, String> {
        let ty = self.u32()?;
        let _null = self.u8()?;
        match ty {
            1 => { self.take(1)?; }
            2 | 3 => { self.take(4)?; }
            4 | 5 | 6 => { self.take(8)?; }
            8 => {
                let count = self.u32()?;
                for _ in 0..count { self.string()?; self.variant()?; }
            }
            9 => {
                let count = self.u32()?;
                for _ in 0..count { self.variant()?; }
            }
            10 => { self.string()?; }
            11 => {
                let count = self.u32()?;
                for _ in 0..count { self.string()?; }
            }
            12 => {
                let len = self.u32()?;
                if len == u32::MAX {
                    return Ok(Some(Vec::new()));
                }
                return Ok(Some(self.take(len as usize)?.to_vec()));
            }
            other => return Err(format!("unsupported QVariant type {other}")),
        }
        Ok(None)
    }
}

fn extract_setup(blob: &[u8]) -> Result<Vec<u8>, DrtRejection> {
    let fail = DrtRejection::BlobDecode;
    let mut r = Reader { data: blob, pos: 0 };
    let _version = r.u32().map_err(fail)?;
    let count = r.u32().map_err(DrtRejection::BlobDecode)?;
    let mut found: HashMap<String, Vec<u8>> = HashMap::new();
    for _ in 0..count {
        let key = r.string().map_err(DrtRejection::BlobDecode)?;
        if let Some(bytes) = r.variant().map_err(DrtRejection::BlobDecode)? {
            found.insert(key, bytes);
        }
    }
    found.remove("SequenceSetup").ok_or(DrtRejection::SetupMissing)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::Write;

    pub enum QtValue { Int(i32), Str(String), Bytes(Vec<u8>), Unknown(u32) }

    pub fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/testdata/drt_sizing/{name}.setup.bin", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    fn put_str(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        out.extend((units.len() as u32).to_be_bytes());
        out.extend(units);
    }

    // QDataStream: u32 BE version, u32 BE count, then per entry: QString (u32 BE byte length + UTF-16BE),
    // QVariant (u32 BE type, u8 null flag 0, payload).
    pub fn qt_fields(entries: &[(&str, QtValue)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(1u32.to_be_bytes());
        out.extend((entries.len() as u32).to_be_bytes());
        for (key, value) in entries {
            put_str(&mut out, key);
            match value {
                QtValue::Int(v) => { out.extend(2u32.to_be_bytes()); out.push(0); out.extend(v.to_be_bytes()); }
                QtValue::Str(s) => { out.extend(10u32.to_be_bytes()); out.push(0); put_str(&mut out, s); }
                QtValue::Bytes(b) => { out.extend(12u32.to_be_bytes()); out.push(0); out.extend((b.len() as u32).to_be_bytes()); out.extend(b); }
                QtValue::Unknown(t) => { out.extend(t.to_be_bytes()); out.push(0); }
            }
        }
        out
    }

    /// `timelines` holds (xml-escaped name, fields-blob hex) pairs.
    pub fn synthetic_drt(timelines: &[(&str, &str)]) -> Vec<u8> {
        let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Sm2MpFolder DbId="f"><MediaVec>"#);
        for (name, hex) in timelines {
            xml.push_str(&format!(
                r#"<Element><Sm2MpTimelineClip DbId="c"><Name>{name}</Name><TimelineSharedHandle><Sm2Timeline DbId="t"><Name>{name}</Name><Sequence><Sm2Sequence DbId="s"><FieldsBlob>{hex}</FieldsBlob><pLmVerTable><ListMgt::LmVersionTable><Locals/></ListMgt::LmVersionTable></pLmVerTable></Sm2Sequence></Sequence></Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element>"#
            ));
        }
        xml.push_str("</MediaVec></Sm2MpFolder>");
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("MediaPool/Master/MpFolder.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
        zw.finish().unwrap().into_inner()
    }

    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn blob_hex(setup: Vec<u8>) -> String {
        hex(&qt_fields(&[("Thumbnail", QtValue::Bytes(vec![1, 2, 3])), ("SequenceSetup", QtValue::Bytes(setup)), ("FirstMainBusFormat", QtValue::Int(2))]))
    }
    fn drt_for(fx: &str, name: &str) -> Vec<u8> { synthetic_drt(&[(name, &blob_hex(fixture(fx)))]) }
    fn reading<'a>(name: &'a str, w: usize, h: usize, api: &'a str) -> ApiSizingReading<'a> {
        ApiSizingReading { timeline_name: name, width: w, height: h, horizontal_mode: api }
    }

    #[test]
    fn decodes_every_recorded_state() {
        for (fx, api, eff, h, v) in [
            ("p0_crop_crop", "scaleToCrop", "scaleToCrop", 1, 1),
            ("e12_stretch_crop", "stretch", "scaleToCrop", 2, 1),
            ("e21_centercrop_crop", "centerCrop", "scaleToCrop", 0, 1),
            ("e31_fit_crop", "scaleToFit", "scaleToCrop", -1, 1),
            ("b3_fit_stretch", "scaleToFit", "stretch", -1, 2),
            ("r1_fit_centercrop", "scaleToFit", "centerCrop", -1, 0),
        ] {
            let got = effective_input_sizing(&drt_for(fx, "Timeline A"), &reading("Timeline A", 1080, 1920, api)).unwrap();
            assert_eq!((got.effective_mode, got.horizontal_raw, got.vertical_raw, got.width, got.height), (eff, h, v, 1080, 1920));
        }
    }

    #[test]
    fn decode_mode_table() {
        assert_eq!([-1, 0, 1, 2, 3].map(decode_mode), [Some("scaleToFit"), Some("centerCrop"), Some("scaleToCrop"), Some("stretch"), None]);
    }

    #[test]
    fn rejects_landscape_as_not_vertical() {
        let r = effective_input_sizing(&drt_for("e11_landscape_stretch", "Timeline A"), &reading("Timeline A", 1920, 1080, "stretch"));
        assert_eq!(r, Err(DrtRejection::NotVertical));
    }

    #[test]
    fn rejects_resolution_mismatch() {
        let r = effective_input_sizing(&drt_for("p0_crop_crop", "Timeline A"), &reading("Timeline A", 1920, 1080, "scaleToCrop"));
        assert_eq!(r, Err(DrtRejection::ResolutionMismatch { drt: (1080, 1920), api: (1920, 1080) }));
    }

    #[test]
    fn rejects_horizontal_mismatch() {
        let r = effective_input_sizing(&drt_for("e31_fit_crop", "Timeline A"), &reading("Timeline A", 1080, 1920, "scaleToCrop"));
        assert_eq!(r, Err(DrtRejection::HorizontalMismatch { drt: -1, api: "scaleToCrop".into() }));
    }

    #[test]
    fn rejects_vertical_out_of_range() {
        let mut setup = fixture("p0_crop_crop");
        setup[112..116].copy_from_slice(&7i32.to_be_bytes());
        let drt = synthetic_drt(&[("Timeline A", &blob_hex(setup))]);
        assert_eq!(effective_input_sizing(&drt, &reading("Timeline A", 1080, 1920, "scaleToCrop")), Err(DrtRejection::VerticalOutOfRange(7)));
    }

    #[test]
    fn rejects_unknown_timeline_name() {
        let r = effective_input_sizing(&drt_for("p0_crop_crop", "Other"), &reading("Timeline A", 1080, 1920, "scaleToCrop"));
        assert_eq!(r, Err(DrtRejection::TimelineNotFound));
    }

    #[test]
    fn rejects_duplicate_timeline_name() {
        let h = blob_hex(fixture("p0_crop_crop"));
        let drt = synthetic_drt(&[("Timeline A", &h), ("Timeline A", &h)]);
        assert_eq!(effective_input_sizing(&drt, &reading("Timeline A", 1080, 1920, "scaleToCrop")), Err(DrtRejection::TimelineAmbiguous(2)));
    }

    #[test]
    fn rejects_unknown_qvariant_type() {
        let h = hex(&qt_fields(&[("X", QtValue::Unknown(99)), ("SequenceSetup", QtValue::Bytes(fixture("p0_crop_crop")))]));
        let r = effective_input_sizing(&synthetic_drt(&[("Timeline A", &h)]), &reading("Timeline A", 1080, 1920, "scaleToCrop"));
        assert!(matches!(r, Err(DrtRejection::BlobDecode(_))), "{r:?}");
    }

    #[test]
    fn rejects_truncated_setup() {
        let drt = synthetic_drt(&[("Timeline A", &blob_hex(vec![0; 100]))]);
        assert_eq!(effective_input_sizing(&drt, &reading("Timeline A", 1080, 1920, "scaleToCrop")), Err(DrtRejection::SetupTooShort(100)));
    }

    #[test]
    fn rejects_missing_setup() {
        let h = hex(&qt_fields(&[("Thumbnail", QtValue::Bytes(vec![1])), ("Name", QtValue::Str("x".into()))]));
        let r = effective_input_sizing(&synthetic_drt(&[("Timeline A", &h)]), &reading("Timeline A", 1080, 1920, "scaleToCrop"));
        assert_eq!(r, Err(DrtRejection::SetupMissing));
    }

    #[test]
    fn rejects_corrupt_zip() {
        let r = effective_input_sizing(b"not a zip", &reading("Timeline A", 1080, 1920, "scaleToCrop"));
        assert!(matches!(r, Err(DrtRejection::Zip(_))), "{r:?}");
    }

    #[test]
    fn rejects_invalid_hex() {
        let r = effective_input_sizing(&synthetic_drt(&[("Timeline A", "zz")]), &reading("Timeline A", 1080, 1920, "scaleToCrop"));
        assert!(matches!(r, Err(DrtRejection::BlobDecode(_))), "{r:?}");
    }

    #[test]
    fn matches_cjk_timeline_name() {
        let r = effective_input_sizing(&drt_for("p0_crop_crop", "竖屏 时间线"), &reading("竖屏 时间线", 1080, 1920, "scaleToCrop"));
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn matches_xml_entity_name() {
        let r = effective_input_sizing(&drt_for("p0_crop_crop", "A &amp; B"), &reading("A & B", 1080, 1920, "scaleToCrop"));
        assert!(r.is_ok(), "{r:?}");
    }

    #[test]
    fn rejection_reasons_are_kebab_case() {
        assert_eq!(DrtRejection::ResolutionMismatch { drt: (1080, 1920), api: (1920, 1080) }.reason(), "resolution-mismatch(drt=1080x1920 api=1920x1080)");
        assert_eq!(DrtRejection::HorizontalMismatch { drt: -1, api: "scaleToCrop".into() }.reason(), "horizontal-mismatch(drt=-1 api=scaleToCrop)");
        assert_eq!(DrtRejection::TimelineAmbiguous(2).reason(), "timeline-ambiguous(2)");
        assert_eq!(DrtRejection::VerticalOutOfRange(7).reason(), "vertical-out-of-range(7)");
        assert_eq!(DrtRejection::NotVertical.reason(), "not-vertical");
        assert_eq!(DrtRejection::SetupTooShort(100).reason(), "setup-too-short(100)");
    }
}
