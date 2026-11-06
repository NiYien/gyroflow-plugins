// SPDX-License-Identifier: GPL-3.0-or-later
// The wrapper marker is an explicit local-candidate gate, not a support claim.
use ofx::*;
use std::ffi::c_void;

pub(super) fn fragment_sampling_policy() -> gyroflow_plugin_base::FragmentSamplingPolicy {
    use gyroflow_plugin_base::FragmentSamplingPolicy;
    if cfg!(feature = "integer-fragment-sampling") {
        FragmentSamplingPolicy::IntegerCoordinates
    } else {
        FragmentSamplingPolicy::CoreDefault
    }
}

pub(super) fn apply_fragment_sampling(stab: &gyroflow_plugin_base::StabilizationManager,
    buffers: &gyroflow_plugin_base::Buffers) -> Result<()> {
    fragment_sampling_policy().apply(stab, buffers).map_err(|reason| {
        log::error!(target: "jianying_ofx", "{reason}");
        Error::InvalidValue
    })
}

pub(super) fn candidate_gate(macos: bool, host: &str, marker: Option<i32>, metal: &str) -> bool {
    macos && host == "com.lemon.lvpro" && marker == Some(1) && metal == "true"
}

pub(super) fn enabled(context: &PluginContext) -> bool {
    if !cfg!(target_os = "macos") { return false; }
    let host = context.get_host();
    if host.get_name().as_deref().ok() != Some("com.lemon.lvpro") { return false; }
    let marker: Result<i32> = RawGetter::get_at(&host, c"NiYienJianyingMacMetalAdapter".as_ptr(), 0);
    candidate_gate(cfg!(target_os = "macos"), &host.get_name().unwrap_or_default(), marker.ok(),
        &host.get_metal_render_supported().unwrap_or_default())
}

pub(super) fn source_provider_enabled(context: &PluginContext) -> bool {
    if !enabled(context) { return false; }
    let marker: Result<i32> = RawGetter::get_at(&context.get_host(), c"NiYienJianyingMacSourceProviderV1".as_ptr(), 0);
    marker.ok() == Some(1)
}

fn configure_cuda_property(status: i32, dimension: i32, set_false: impl FnOnce() -> Result<()>) -> Result<()> {
    match (status, dimension) {
        (0, 1) => set_false(),
        // Only the descriptor property set establishes whether this optional declaration exists.
        (3, _) => Ok(()),
        _ => {
            log::error!(target: "jianying_ofx", "Mac CUDA descriptor dimension failed: status={status} dimension={dimension}");
            Err(if status == 0 { Error::InvalidValue } else { Error::from(status) })
        }
    }
}

pub(super) fn configure_optional_cuda(context: &PluginContext, descriptor: &mut EffectDescriptor) -> Result<()> {
    let host = context.get_host();
    // The host getter may supply a default even when a property is absent; keep all raw evidence separate.
    let host_suite = unsafe { host.suite().as_ref() }.ok_or(Error::InvalidSuite)?;
    let get = host_suite.propGetString.ok_or(Error::SuiteNotInitialized)?;
    let dimension = host_suite.propGetDimension.ok_or(Error::SuiteNotInitialized)?;
    let mut value: *const std::ffi::c_char = std::ptr::null();
    let host_get_status = unsafe { get(host.handle(), c"OfxImageEffectPropCudaRenderSupported".as_ptr(), 0, &mut value) };
    let host_value = if host_get_status == 0 && !value.is_null() {
        unsafe { std::ffi::CStr::from_ptr(value) }.to_string_lossy().into_owned()
    } else { "(unavailable)".to_owned() };
    let mut host_dimension = 0;
    let host_dimension_status = unsafe { dimension(host.handle(), c"OfxImageEffectPropCudaRenderSupported".as_ptr(), &mut host_dimension) };
    let descriptor_suite = unsafe { descriptor.suite().as_ref() }.ok_or(Error::InvalidSuite)?;
    let descriptor_get_dimension = descriptor_suite.propGetDimension.ok_or(Error::SuiteNotInitialized)?;
    let mut descriptor_dimension = 0;
    let descriptor_dimension_status = unsafe { descriptor_get_dimension(descriptor.handle(),
        c"OfxImageEffectPropCudaRenderSupported".as_ptr(), &mut descriptor_dimension) };
    log::info!(target: "jianying_ofx", "Mac CUDA declaration evidence: host_get_status={host_get_status} host_value={host_value:?} host_dimension_status={host_dimension_status} host_dimension={host_dimension} descriptor_dimension_status={descriptor_dimension_status} descriptor_dimension={descriptor_dimension}");
    configure_cuda_property(descriptor_dimension_status, descriptor_dimension,
        || descriptor.set_cuda_render_supported("false"))
}

pub(super) fn framebuffer_inverted(context: &PluginContext) -> Result<bool> {
    let value: i32 = RawGetter::get_at(&context.get_host(), c"NiYienJianyingMacFramebufferInverted".as_ptr(), 0)?;
    match value { 0 => Ok(false), 1 => Ok(true), _ => Err(Error::InvalidValue) }
}

fn disabled(value: Result<i32>) -> bool {
    // The pinned binding maps absent optional properties to UnknownError.
    matches!(value, Ok(0) | Err(Error::UnknownError))
}

pub(super) fn render_flags_valid(metal: Result<i32>, gl: Result<i32>, cl: Result<i32>,
    cuda: Result<i32>, texture: Result<i32>, queue: *mut c_void) -> bool {
    matches!(metal, Ok(1)) && matches!(gl, Ok(0)) && disabled(cl) && disabled(cuda)
        && disabled(texture) && !queue.is_null()
}

pub(super) fn render_queue(args: &RenderInArgs) -> Result<*mut c_void> {
    let queue = args.get_metal_command_queue()? as *mut c_void;
    if !render_flags_valid(
        RawGetter::get_at(args, c"OfxImageEffectPropMetalEnabled".as_ptr(), 0),
        RawGetter::get_at(args, c"OfxImageEffectPropOpenGLEnabled".as_ptr(), 0),
        RawGetter::get_at(args, c"OfxImageEffectPropOpenCLEnabled".as_ptr(), 0),
        RawGetter::get_at(args, c"OfxImageEffectPropCudaEnabled".as_ptr(), 0),
        RawGetter::get_at(args, c"OfxImageEffectPropMetalTextureEnabled".as_ptr(), 0), queue) {
        log::error!(target: "jianying_ofx", "Mac candidate requires standard Metal buffers and one non-null host queue");
        return Err(Error::InvalidValue);
    }
    Ok(queue)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ImageLayout { pub width: i32, pub height: i32, pub stride: i32 }

pub(super) fn rgba8_layout(bounds: RectI, stride: i32, depth: BitDepth, components: ImageComponent) -> Result<ImageLayout> {
    let width = bounds.x2.checked_sub(bounds.x1).ok_or(Error::InvalidValue)?;
    let height = bounds.y2.checked_sub(bounds.y1).ok_or(Error::InvalidValue)?;
    if bounds.x1 != 0 || bounds.y1 != 0 || width <= 0 || height <= 0 || stride <= 0
        || depth != BitDepth::Byte || components != ImageComponent::RGBA
        || width.checked_mul(4).is_none_or(|minimum| stride < minimum) {
        return Err(Error::InvalidValue);
    }
    Ok(ImageLayout { width, height, stride })
}

pub(super) fn image_layout(image: &Image) -> Result<ImageLayout> {
    let bounds = image.get_bounds()?;
    let rod = image.get_region_of_definition()?;
    if [bounds.x1, bounds.y1, bounds.x2, bounds.y2] != [rod.x1, rod.y1, rod.x2, rod.y2]
        || image.get_data()?.is_null() { return Err(Error::InvalidValue); }
    rgba8_layout(bounds, image.get_row_bytes()?, image.get_pixel_depth()?, image.get_components()?)
}

pub(super) type MetalCopy = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void,
    i32, i32, i32, i32, i32) -> i32;

fn copy_buffers(copy: MetalCopy, source: *mut c_void, output: *mut c_void, queue: *mut c_void,
    src: ImageLayout, dst: ImageLayout) -> Result<()> {
    if source.is_null() || output.is_null() || queue.is_null() || src.width != dst.width || src.height != dst.height {
        return Err(Error::InvalidValue);
    }
    // The wrapper owns resource validation and queue completion; no CPU address is accessed here.
    let status = unsafe { copy(source, output, queue, src.width, src.height, src.stride, dst.stride, 4) };
    if status != 0 {
        log::error!(target: "jianying_ofx", "Mac Metal passthrough failed: status={status}");
        return Err(Error::InvalidValue);
    }
    Ok(())
}

pub(super) fn copy_images(context: &PluginContext, source: &Image, output: &Image, queue: *mut c_void) -> Result<i32> {
    let src = image_layout(source)?;
    let dst = image_layout(output)?;
    let pointer: *mut c_void = RawGetter::get_at(&context.get_host(), c"NiYienJianyingMacMetalCopyV1".as_ptr(), 0)?;
    if pointer.is_null() { return Err(Error::InvalidValue); }
    // Only the independently packaged wrapper advertises this exact C ABI callback.
    let copy: MetalCopy = unsafe { std::mem::transmute(pointer) };
    copy_buffers(copy, source.get_data()? as *mut c_void, output.get_data()? as *mut c_void, queue, src, dst)?;
    OK
}

pub(super) fn source_timestamp_us(time: f64, source_fps: f64, speed: f64) -> Result<i64> {
    if !time.is_finite() || !source_fps.is_finite() || source_fps <= 0.0 || !speed.is_finite() || speed <= 0.0 {
        return Err(Error::InvalidValue);
    }
    // Mac starts with the observed standard OFX floating frame clock. Windows rounding stays separate.
    let timestamp = time / source_fps * speed * 1_000_000.0;
    if !timestamp.is_finite() || timestamp < i64::MIN as f64 || timestamp >= i64::MAX as f64 {
        return Err(Error::InvalidValue);
    }
    Ok(timestamp.round() as i64)
}

pub(super) fn parameter_frame(time: gyroflow_plugin_base::TimeType, host_fps: f64) -> f64 {
    use gyroflow_plugin_base::TimeType;
    match time {
        TimeType::Frame(x) => x,
        TimeType::FrameOrMicrosecond((_, Some(us))) | TimeType::Microseconds(us) => us as f64 / 1_000_000.0 * host_fps,
        TimeType::Milliseconds(ms) => ms / 1_000.0 * host_fps,
        other => super::frame_from_timetype(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn mac_jianying_known_cuda_capability_is_disabled_and_setter_errors_propagate() {
        let written = std::cell::Cell::new(None);
        assert!(configure_cuda_property(0, 1, || { written.set(Some(false)); Ok(()) }).is_ok());
        assert_eq!(written.get(), Some(false));
        assert!(matches!(configure_cuda_property(0, 1, || Err(Error::InvalidSuite)), Err(Error::InvalidSuite)));
    }

    #[test]
    fn mac_jianying_absent_cuda_capability_does_not_call_setter() {
        let called = std::cell::Cell::new(false);
        assert!(configure_cuda_property(3, 0, || { called.set(true); Err(Error::InvalidValue) }).is_ok());
        assert!(!called.get());
    }

    #[test]
    fn mac_jianying_cuda_descriptor_getter_failures_are_not_treated_as_absence() {
        let called = std::cell::Cell::new(false);
        for status in [-1, 1, 2, 4, 8, 9, 11, 14] {
            assert!(configure_cuda_property(status, 0, || { called.set(true); Ok(()) }).is_err());
        }
        assert!(configure_cuda_property(0, 0, || { called.set(true); Ok(()) }).is_err());
        assert!(!called.get());
    }

    #[test]
    fn mac_jianying_cuda_descriptor_requires_one_declared_value() {
        let called = std::cell::Cell::new(false);
        for dimension in [-1, 0, 2] {
            assert!(configure_cuda_property(0, dimension, || { called.set(true); Ok(()) }).is_err());
        }
        assert!(!called.get());
    }

    #[test]
    fn mac_sampling_selection_is_explicit_and_default_compatible() {
        use gyroflow_plugin_base::FragmentSamplingPolicy;
        let expected = if cfg!(feature = "integer-fragment-sampling") {
            FragmentSamplingPolicy::IntegerCoordinates
        } else {
            FragmentSamplingPolicy::CoreDefault
        };
        assert_eq!(fragment_sampling_policy(), expected);
        assert!(FragmentSamplingPolicy::CoreDefault.supported_by(true, 0));
    }

    #[test]
    fn mac_jianying_candidate_requires_wrapper_and_metal() {
        assert!(candidate_gate(true, "com.lemon.lvpro", Some(1), "true"));
        for case in [(false, "com.lemon.lvpro", Some(1), "true"), (true, "com.lemon.lvpro", None, "true"),
            (true, "com.lemon.lvpro", Some(0), "true"), (true, "com.capcut", Some(1), "true"),
            (true, "com.lemon.lvpro", Some(1), "false")] {
            assert!(!candidate_gate(case.0, case.1, case.2, case.3));
        }
    }

    #[test]
    fn mac_jianying_render_rejects_other_backends_and_missing_queue() {
        let queue = 3usize as *mut c_void;
        assert!(render_flags_valid(Ok(1), Ok(0), Ok(0), Ok(0), Err(Error::UnknownError), queue));
        assert!(render_flags_valid(Ok(1), Ok(0), Err(Error::UnknownError), Err(Error::UnknownError), Ok(0), queue));
        assert!(!render_flags_valid(Ok(0), Ok(0), Ok(0), Ok(0), Ok(0), queue));
        assert!(!render_flags_valid(Ok(1), Ok(1), Ok(0), Ok(0), Ok(0), queue));
        assert!(!render_flags_valid(Ok(1), Ok(0), Ok(1), Ok(0), Ok(0), queue));
        assert!(!render_flags_valid(Ok(1), Ok(0), Ok(0), Ok(1), Ok(0), queue));
        assert!(!render_flags_valid(Ok(1), Ok(0), Ok(0), Ok(0), Ok(1), queue));
        assert!(!render_flags_valid(Ok(1), Err(Error::UnknownError), Ok(0), Ok(0), Ok(0), queue));
        assert!(!render_flags_valid(Ok(1), Ok(0), Ok(0), Ok(0), Err(Error::InvalidValue), queue));
        assert!(!render_flags_valid(Ok(1), Ok(0), Ok(0), Ok(0), Ok(0), std::ptr::null_mut()));
    }

    #[test]
    fn mac_jianying_only_accepts_zero_origin_rgba8_and_positive_stride() {
        let bounds = RectI { x1: 0, y1: 0, x2: 640, y2: 360 };
        assert_eq!(rgba8_layout(bounds, 2560, BitDepth::Byte, ImageComponent::RGBA).unwrap(),
            ImageLayout { width: 640, height: 360, stride: 2560 });
        for stride in [0, -2560, 2559] { assert!(rgba8_layout(bounds, stride, BitDepth::Byte, ImageComponent::RGBA).is_err()); }
        assert!(rgba8_layout(RectI { x1: 1, ..bounds }, 2560, BitDepth::Byte, ImageComponent::RGBA).is_err());
        assert!(rgba8_layout(bounds, 2560, BitDepth::Float, ImageComponent::RGBA).is_err());
        assert!(rgba8_layout(bounds, 2560, BitDepth::Byte, ImageComponent::RGB).is_err());
    }

    static COPY_CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn good_copy(src: *mut c_void, dst: *mut c_void, queue: *mut c_void,
        width: i32, height: i32, src_stride: i32, dst_stride: i32, bpp: i32) -> i32 {
        COPY_CALLS.fetch_add(1, Ordering::SeqCst);
        if (src as usize, dst as usize, queue as usize, width, height, src_stride, dst_stride, bpp)
            == (1, 2, 3, 640, 360, 2560, 2816, 4) { 0 } else { 8 }
    }
    unsafe extern "C" fn failed_copy(_: *mut c_void, _: *mut c_void, _: *mut c_void,
        _: i32, _: i32, _: i32, _: i32, _: i32) -> i32 { 9 }

    #[test]
    fn mac_jianying_passthrough_uses_exact_callback_abi_and_stops_on_failure() {
        let src = ImageLayout { width: 640, height: 360, stride: 2560 };
        let dst = ImageLayout { stride: 2816, ..src };
        COPY_CALLS.store(0, Ordering::SeqCst);
        assert!(copy_buffers(good_copy, 1usize as _, 2usize as _, 3usize as _, src, dst).is_ok());
        assert_eq!(COPY_CALLS.load(Ordering::SeqCst), 1);
        assert!(copy_buffers(good_copy, 1usize as _, 2usize as _, 3usize as _, src,
            ImageLayout { height: 359, ..dst }).is_err());
        assert_eq!(COPY_CALLS.load(Ordering::SeqCst), 1);
        assert!(copy_buffers(failed_copy, 1usize as _, 2usize as _, 3usize as _, src, dst).is_err());
    }

    #[test]
    fn mac_jianying_frame_clock_preserves_fractional_ofx_frames() {
        assert_eq!(source_timestamp_us(30.5, 30.0, 1.0).unwrap(), 1_016_667);
        assert_eq!(source_timestamp_us(1.1, 60.0, 1.0).unwrap(), 18_333);
        assert_eq!(source_timestamp_us(15.0, 30.0, 2.0).unwrap(), 1_000_000);
        for fps in [0.0, -1.0, f64::NAN] { assert!(source_timestamp_us(1.0, fps, 1.0).is_err()); }
        assert!(source_timestamp_us(f64::INFINITY, 30.0, 1.0).is_err());
        assert_eq!(parameter_frame(gyroflow_plugin_base::TimeType::Frame(30.5), 30.0), 30.5);
        assert_eq!(parameter_frame(gyroflow_plugin_base::TimeType::Microseconds(1_500_000), 30.0), 45.0);
    }
}
