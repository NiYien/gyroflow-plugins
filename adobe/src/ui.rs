use super::*;
use std::sync::OnceLock;

pub struct PngImage {
    pub width: usize,
    pub height: usize,
    pub line_size: usize,
    pub data: Vec<u8>,
}
impl PngImage {
    pub fn new(png_bytes: &[u8]) -> Self {
        let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut data = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut data).unwrap();
        if data.len() != info.buffer_size() {
            data.resize(info.buffer_size(), 0)
        }
        Self::rgba_to_bgra(&mut data);
        Self {
            width: info.width as _,
            height: info.height as _,
            line_size: info.line_size as _,
            data,
        }
    }
    pub fn rgba_to_bgra(data: &mut [u8]) {
        for chunk in data.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
    }
}

pub fn draw(_in_data: &ae::InData, params: &mut ae::Parameters<Params>, event: &mut ae::EventExtra, inst: &mut CrossThreadInstance) -> Result<(), ae::Error> {
    if event.effect_area() == ae::EffectArea::Control {
        let current_frame = event.current_frame();

        let drawbot = event.context_handle().drawing_reference()?;
        let supplier = drawbot.supplier()?;
        let surface = drawbot.surface()?;

        // Fill the background
        static BG_COLOR: OnceLock<ae::drawbot::ColorRgba> = OnceLock::new();
        surface.paint_rect(BG_COLOR.get_or_init(acquire_background_color), &ae::drawbot::RectF32 {
            left:   current_frame.left     as f32,
            top:    current_frame.top      as f32,
            width:  current_frame.width()  as f32,
            height: current_frame.height() as f32 + 1.0,
        })?;

        // Draw logo
        if event.param_index() == params.index(Params::Logo).unwrap_or_default() {
            if gyroflow_plugin_base::update_notice::available() == Some(true) {
                let bg = BG_COLOR.get_or_init(acquire_background_color);
                let light = bg.red * 0.2126 + bg.green * 0.7152 + bg.blue * 0.0722 > 0.5;
                let accent = if light { ae::drawbot::ColorRgba { red: 0.50, green: 0.29, blue: 0.03, alpha: 1.0 } }
                    else { ae::drawbot::ColorRgba { red: 0.95, green: 0.76, blue: 0.40, alpha: 1.0 } };
                surface.paint_rect(&accent, &drawbot::RectF32 {
                    left: current_frame.left as f32, top: current_frame.top as f32,
                    width: 3.0, height: current_frame.height() as f32,
                })?;
                let title_font = supplier.new_default_font(supplier.default_font_size()?)?;
                let body_font = supplier.new_default_font((supplier.default_font_size()? * 0.9).max(11.0))?;
                let title_brush = supplier.new_brush(&accent)?;
                let text = if light { 0.12 } else { 0.88 };
                let text_brush = supplier.new_brush(&ae::drawbot::ColorRgba { red: text, green: text, blue: text, alpha: 1.0 })?;
                let width = (current_frame.width() as f32 - 16.0).max(1.0);
                let origin = drawbot::PointF32 { x: current_frame.left as f32 + 8.0, y: current_frame.top as f32 + 13.0 };
                surface.draw_string(&title_brush, &title_font, gyroflow_plugin_base::t!("update.available"), &origin,
                    drawbot::TextAlignment::Left, drawbot::TextTruncation::End, width)?;
                let host = if _in_data.is_premiere() { "Adobe Premiere Pro" } else { "Adobe After Effects" };
                let reminder = gyroflow_plugin_base::update_notice::close_host_message(host);
                surface.draw_string(&text_brush, &body_font, &reminder,
                    &drawbot::PointF32 { x: origin.x, y: current_frame.top as f32 + 29.0 },
                    drawbot::TextAlignment::Left, drawbot::TextTruncation::End, width)?;
                let button = update_button_rect(current_frame);
                let button_bg = if light { 0.84 } else { 0.27 };
                surface.paint_rect(&ae::drawbot::ColorRgba { red: button_bg, green: button_bg, blue: button_bg, alpha: 1.0 }, &button)?;
                surface.draw_string(&text_brush, &body_font, gyroflow_plugin_base::t!("update.open_app"),
                    &drawbot::PointF32 { x: button.left + button.width / 2.0, y: button.top + 15.0 },
                    drawbot::TextAlignment::Center, drawbot::TextTruncation::End, button.width - 8.0)?;
            } else {
            static PNG: OnceLock<PngImage> = OnceLock::new();
            let png = PNG.get_or_init(|| PngImage::new(include_bytes!("../logo_white.png")));

            if let Ok(img) = supplier.new_image_from_buffer(png.width, png.height, png.line_size, drawbot::PixelLayout::Bgra32Straight, &png.data) {
                let origin = drawbot::PointF32 {
                    x: current_frame.left as f32 + (current_frame.width() as f32 - png.width as f32) / 2.0,
                    y: current_frame.top as f32,
                };
                surface.draw_image(&img, &origin, 1.0)?;
            }
            }
        }

        if event.param_index() == params.index(Params::TranslationStatus).unwrap_or_default() {
            let status = get_str2(inst, Params::TranslationStatus).unwrap_or_default();
            let font = supplier.new_default_font(supplier.default_font_size()? * 0.9)?;
            let brush = supplier.new_brush(&ae::drawbot::ColorRgba { red: 0.8, green: 0.8, blue: 0.8, alpha: 1.0 })?;
            let origin = ae::drawbot::PointF32 { x: current_frame.left as f32, y: current_frame.top as f32 + 10.0 };
            surface.draw_string(&brush, &font, &status, &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
        }

        // Draw status
        if event.param_index() == params.index(Params::Status).unwrap_or_default() {
            let status = get_str(params, inst, Params::Status)?;
            let proj   = get_str2(inst, Params::LoadedProject).unwrap_or("---".to_owned());
            let lens   = get_str2(inst, Params::LoadedLens).unwrap_or("---".to_owned());
            let preset = get_str2(inst, Params::LoadedPreset).unwrap_or_default();

            if !status.is_empty() {
                let font = supplier.new_default_font(supplier.default_font_size()? * 0.9)?;
                // Pick the colour from the status's meaning, not its display text. The status
                // string is localized (set via t!()), so comparing against English literals like
                // "OK" always fell through to the red branch in non-English locales (status showed
                // red during normal operation). Compare against the same localized values instead.
                let text_color = if status == gyroflow_plugin_base::t!("status.ok") {
                    ae::drawbot::ColorRgba { red: 0.22, green: 0.86, blue: 0.1, alpha: 1.0 } // Green
                } else if status == gyroflow_plugin_base::t!("status.calculating") {
                    ae::drawbot::ColorRgba { red: 0.92, green: 0.57, blue: 0.08, alpha: 1.0 } // Yellow
                } else if status == gyroflow_plugin_base::t!("status.project_not_loaded") || status == "---" {
                    ae::drawbot::ColorRgba { red: 0.8, green: 0.8, blue: 0.8, alpha: 1.0 } // White
                } else {
                    ae::drawbot::ColorRgba { red: 0.95, green: 0.15, blue: 0.15, alpha: 1.0 } // Red
                };
                let string_brush = supplier.new_brush(&text_color)?;
                let mut origin = ae::drawbot::PointF32 {
                    x: current_frame.left as f32,
                    y: current_frame.top as f32 + 10.0,
                };

                surface.draw_string(&string_brush, &font, &status, &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;

                let string_brush = supplier.new_brush(&ae::drawbot::ColorRgba { red: 0.8, green: 0.8, blue: 0.8, alpha: 1.0 })?;
                origin.y += 15.0;
                surface.draw_string(&string_brush, &font, &format!("Project: {proj}"),  &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
                origin.y += 15.0;
                surface.draw_string(&string_brush, &font, &format!("Lens: {lens}"),     &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
                if !preset.is_empty() {
                    origin.y += 15.0;
                    surface.draw_string(&string_brush, &font, &format!("Preset: {preset}"), &origin, ae::drawbot::TextAlignment::Left, ae::drawbot::TextTruncation::None, 0.0)?;
                }
            }
        }
    }
    event.set_event_out_flags(ae::EventOutFlags::HANDLED_EVENT);

    Ok(())
}

pub fn acquire_background_color() -> ae::drawbot::ColorRgba {
    const MAX_SHORT_COLOR: f32 = 65535.0;
    const INV_SIXTY_FIVE_K: f32 = 1.0 / MAX_SHORT_COLOR;

    let bg = ae::pf::suites::App::new()
        .and_then(|x| x.bg_color())
        .unwrap_or(ae::sys::PF_App_Color { red: 9830, green: 9830, blue: 9830 });
    ae::drawbot::ColorRgba {
        red:   bg.red   as f32 * INV_SIXTY_FIVE_K,
        green: bg.green as f32 * INV_SIXTY_FIVE_K,
        blue:  bg.blue  as f32 * INV_SIXTY_FIVE_K,
        alpha: 1.0,
    }
}

fn update_button_rect(frame: ae::Rect) -> drawbot::RectF32 {
    drawbot::RectF32 {
        left: frame.left as f32 + 8.0,
        top: frame.top as f32 + 36.0,
        width: (frame.width() as f32 - 16.0).max(1.0),
        height: (frame.height() as f32 - 37.0).clamp(0.0, 23.0),
    }
}

fn update_button_hit(frame: ae::Rect, point: ae::Point) -> bool {
    let button = update_button_rect(frame);
    point.h as f32 >= button.left && (point.h as f32) < button.left + button.width
        && point.v as f32 >= button.top && (point.v as f32) < button.top + button.height
}

pub fn handle_update_event(params: &ae::Parameters<Params>, event: &mut ae::EventExtra) -> bool {
    if !matches!(event.event(), ae::Event::Click(_) | ae::Event::Keydown(_) | ae::Event::AdjustCursor(_)) {
        return false;
    }
    if event.window_type() != ae::WindowType::Effect || event.effect_area() != ae::EffectArea::Control
        || params.index(Params::Logo) != Some(event.param_index())
        || gyroflow_plugin_base::update_notice::available() != Some(true) {
        return false;
    }
    let activate = match event.event() {
        ae::Event::Click(info) => update_button_hit(event.current_frame(), info.screen_point()),
        ae::Event::Keydown(info) => {
            let key = info.as_ref().keycode;
            let code = key & 0xffff;
            if key & (1 << 31) != 0 { code == u32::from(b' ') }
            else { [ae::sys::PF_ControlCode_Space, ae::sys::PF_ControlCode_Return, ae::sys::PF_ControlCode_Enter].contains(&(code as _)) }
        }
        ae::Event::AdjustCursor(_) if update_button_hit(event.current_frame(), event.screen_point()) => {
            event.set_cursor(ae::CursorType::FingerPointer);
            event.set_event_out_flags(ae::EventOutFlags::HANDLED_EVENT);
            return true;
        }
        _ => false,
    };
    if activate {
        GyroflowPluginBase::open_gyroflow(None);
        event.set_event_out_flags(ae::EventOutFlags::HANDLED_EVENT | ae::EventOutFlags::NEVER_UPDATE);
    }
    activate
}

#[cfg(test)]
mod update_ui_tests {
    use super::*;

    #[test]
    fn only_the_visible_button_is_clickable() {
        let frame = ae::Rect { left: 100, top: 200, right: 350, bottom: 260 };
        assert!(update_button_hit(frame, ae::Point { h: 120, v: 245 }));
        assert!(!update_button_hit(frame, ae::Point { h: 120, v: 215 }));
        assert!(!update_button_hit(frame, ae::Point { h: 120, v: 229 }));
        assert!(!update_button_hit(frame, ae::Point { h: 350, v: 245 }));
        assert!(!update_button_hit(frame, ae::Point { h: 120, v: 260 }));
    }
}

fn get_str(params: &ae::Parameters<Params>, inst: &mut CrossThreadInstance, key: Params) -> Result<String, ae::Error> {
    let mut value = params.get(key)?.as_arbitrary()?.value::<ArbString>().map(|x| x.get().to_owned()).unwrap_or_default();
    if value.is_empty() {
        if let Ok(v2) = get_str2(inst, key) {
            value = v2;
        }
    }
    Ok(value)
}
fn get_str2(inst: &mut CrossThreadInstance, key: Params) -> Result<String, ae::Error> {
    let _self = inst.get().unwrap();
    if let Some(_self) = _self.try_read_for(std::time::Duration::from_millis(300)) {
        if let Some(v) = _self.stored.read().pending_params_str.get(&key) {
            return Ok(v.clone());
        }
    };
    Err(ae::Error::InvalidParms)
}
