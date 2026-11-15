// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use gyroflow_plugin_base::lru::LruCache;
use std::{cell::RefCell, ffi::{c_char, c_void, CStr}, ptr};

// Only the host's memory and parameter suites are substituted. Tests call the
// production callbacks, parameter adapter and shared manager loading path.
#[repr(C)]
struct HostHandle {
    ptr: *mut c_void,
    bytes: Vec<u8>,
}

unsafe extern "C" fn new_handle(size: ae::sys::A_HandleSize) -> ae::sys::PF_Handle {
    let mut bytes = vec![0; size as usize];
    Box::into_raw(Box::new(HostHandle { ptr: bytes.as_mut_ptr().cast(), bytes })).cast()
}
unsafe extern "C" fn lock_handle(handle: ae::sys::PF_Handle) -> *mut c_void {
    unsafe { (*handle.cast::<HostHandle>()).ptr }
}
unsafe extern "C" fn unlock_handle(_: ae::sys::PF_Handle) {}
unsafe extern "C" fn dispose_handle(handle: ae::sys::PF_Handle) {
    unsafe { drop(Box::from_raw(handle.cast::<HostHandle>())); }
}
unsafe extern "C" fn handle_size(handle: ae::sys::PF_Handle) -> ae::sys::A_HandleSize {
    unsafe { (*handle.cast::<HostHandle>()).bytes.len() as _ }
}
unsafe extern "C" fn resize_handle(size: ae::sys::A_HandleSize, handle: *mut ae::sys::PF_Handle) -> ae::sys::PF_Err {
    let handle = unsafe { &mut *(*handle).cast::<HostHandle>() };
    handle.bytes.resize(size as usize, 0);
    handle.ptr = handle.bytes.as_mut_ptr().cast();
    0
}

#[derive(Default)]
struct HostParam {
    bytes: Option<Vec<u8>>,
    calls: Vec<(i32, i64)>,
}
thread_local! {
    static HOST_PARAM: RefCell<HostParam> = RefCell::new(HostParam::default());
}
unsafe extern "C" fn get_param(_: i32, index: i32, time: i64, out: *mut pr::sys::PrParam) -> pr::sys::prSuiteError {
    HOST_PARAM.with(|state| {
        let mut state = state.borrow_mut();
        state.calls.push((index, time));
        let Some(bytes) = state.bytes.as_mut() else { return pr::sys::suiteError_InvalidParms; };
        unsafe {
            (*out).mType = pr::sys::PrParamType_kPrParamType_PrMemoryPtr;
            (*out).__bindgen_anon_1.mMemoryPtr = bytes.as_mut_ptr().cast();
        }
        0
    })
}
unsafe extern "C" fn ptr_size(_: pr::sys::PrMemoryPtr) -> u32 {
    HOST_PARAM.with(|state| state.borrow().bytes.as_ref().unwrap().len() as u32)
}
unsafe extern "C" fn release_suite(_: *const c_char, _: i32) -> i32 { 0 }
unsafe extern "C" fn acquire_suite(name: *const c_char, _: i32, out: *mut *const c_void) -> i32 {
    let name = unsafe { CStr::from_ptr(name).to_bytes_with_nul() };
    macro_rules! provide {
        ($name:expr, $ty:ty, $value:expr) => {
            if name == $name {
                static SUITE: $ty = $value;
                unsafe { *out = ptr::addr_of!(SUITE).cast(); }
                return 0;
            }
        };
    }
    provide!(ae::sys::kPFHandleSuite, ae::sys::PF_HandleSuite1, ae::sys::PF_HandleSuite1 {
        host_new_handle: Some(new_handle), host_lock_handle: Some(lock_handle),
        host_unlock_handle: Some(unlock_handle), host_dispose_handle: Some(dispose_handle),
        host_get_handle_size: Some(handle_size), host_resize_handle: Some(resize_handle),
    });
    provide!(pr::sys::kPrSDKVideoSegmentSuite, pr::sys::PrSDKVideoSegmentSuite,
        pr::sys::PrSDKVideoSegmentSuite { GetParam: Some(get_param), ..unsafe { std::mem::zeroed() } });
    provide!(pr::sys::kPrSDKMemoryManagerSuite, pr::sys::PrSDKMemoryManagerSuite,
        pr::sys::PrSDKMemoryManagerSuite { GetPtrSize: Some(ptr_size), ..unsafe { std::mem::zeroed() } });
    provide!(pr::sys::kPrSDKGPUDeviceSuite, pr::sys::PrSDKGPUDeviceSuite, unsafe { std::mem::zeroed() });
    provide!(pr::sys::kPrSDKGPUImageProcessingSuite, pr::sys::PrSDKGPUImageProcessingSuite, unsafe { std::mem::zeroed() });
    provide!(pr::sys::kPrSDKPPixSuite, pr::sys::PrSDKPPixSuite, unsafe { std::mem::zeroed() });
    provide!(pr::sys::kPrSDKPPix2Suite, pr::sys::PrSDKPPix2Suite, unsafe { std::mem::zeroed() });
    -1
}

fn with_ae_host(f: impl FnOnce()) {
    let suite = ae::sys::SPBasicSuite { AcquireSuite: Some(acquire_suite), ReleaseSuite: Some(release_suite), ..unsafe { std::mem::zeroed() } };
    let _pica = ae::PicaBasicSuite::from_sp_basic_suite_raw(&suite);
    f();
}
fn with_premiere(f: impl FnOnce(&mut ParamHandler<'_, '_>)) {
    let suite = pr::sys::SPBasicSuite { AcquireSuite: Some(acquire_suite), ReleaseSuite: Some(release_suite), ..unsafe { std::mem::zeroed() } };
    let _pica = pr::PicaBasicSuite::from_sp_basic_suite_raw(&suite);
    HOST_PARAM.with(|state| *state.borrow_mut() = HostParam::default());
    let mut instance: pr::sys::PrGPUFilterInstance = unsafe { std::mem::zeroed() };
    let filter = pr::GpuFilterData {
        instance_ptr: &mut instance,
        gpu_device_suite: pr::suites::GPUDevice::new().unwrap(),
        gpu_image_processing_suite: pr::suites::GPUImageProcessing::new().unwrap(),
        memory_manager_suite: pr::suites::MemoryManager::new().unwrap(),
        ppix_suite: pr::suites::PPix::new().unwrap(), ppix2_suite: pr::suites::PPix2::new().unwrap(),
        video_segment_suite: pr::suites::VideoSegment::new().unwrap(), gpu_info: unsafe { std::mem::zeroed() },
    };
    let mut render: pr::sys::PrGPUFilterRenderParams = unsafe { std::mem::zeroed() };
    render.inClipTime = 12345;
    let map = [Params::ProjectPath, Params::LoadedProject, Params::LoadedLens, Params::LoadedPreset]
        .into_iter().enumerate().map(|(i, p)| (p, ae::ParamMapInfo { index: i + 1, type_: ae::ParamType::ArbitraryData })).collect();
    param_index_for_type(Params::ProjectPath, Some(map));
    let mut params = ParamHandler {
        inner: ParamsInner::Premiere((&filter, pr::RenderParams::from_raw(&render))),
        stored: Arc::new(RwLock::new(StoredParams::default())),
    };
    f(&mut params);
}

fn callback(raw: &mut ae::sys::PF_ArbParamsExtra) -> Result<(), ae::Error> {
    ArbString::callback(&ae::ArbParamsExtra::from_raw(raw))
}
fn extra(function: u32) -> ae::sys::PF_ArbParamsExtra {
    ae::sys::PF_ArbParamsExtra { which_function: function as _, ..unsafe { std::mem::zeroed() } }
}

#[test]
fn string_flattening_preserves_old_projects() {
    #[derive(Serialize)]
    struct OldArbString { val: String }
    for text in [String::new(), "E:/照片/工程🎥.gyroflow".into(), "{\"gyro\":\"数据\"}".repeat(10000)] {
        let bytes = ArbString::encode(&text);
        assert_eq!(bytes, bincode::serde::encode_to_vec(&OldArbString { val: text.clone() }, bincode::config::legacy()).unwrap());
        assert_eq!(bytes, bincode::serde::encode_to_vec(&text, bincode::config::legacy()).unwrap());
        assert_eq!(ArbString::decode(&bytes), Some(text.as_bytes()));
        let mut padded = bytes.clone();
        padded.extend_from_slice(&[0; 8]);
        assert_eq!(ArbString::decode(&padded), Some(text.as_bytes()));
        assert_eq!(ArbString::decode(&bytes[..bytes.len() - 1]), None);
    }
    assert_eq!(ArbString::encode(""), ArbString::EMPTY.to_le_bytes());
    assert_eq!(ArbString::decode(&[1, 2]), None);
    assert_eq!(ArbString::decode(&u64::MAX.to_le_bytes()), None);
}

#[test]
fn arbitrary_callbacks_keep_copies_independent_and_roundtrip_saved_data() {
    with_ae_host(|| {
        let mut original = ptr::null_mut();
        let mut raw = extra(ae::sys::PF_Arbitrary_NEW_FUNC as _);
        raw.u.new_func_params.arbPH = &mut original;
        callback(&mut raw).unwrap();
        assert_eq!(ArbString::read(original), "");
        assert!(ArbString::write(&mut original, "中文工程🎥").unwrap());
        assert!(!ArbString::write(&mut original, "中文工程🎥").unwrap());
        let mut copy = ptr::null_mut();
        raw = extra(ae::sys::PF_Arbitrary_COPY_FUNC as _);
        raw.u.copy_func_params.src_arbH = original;
        raw.u.copy_func_params.dst_arbPH = &mut copy;
        callback(&mut raw).unwrap();
        // Premiere can also copy the handle's bytes without the arbitrary callback.
        let byte_copy = unsafe { new_handle(handle_size(original)) };
        unsafe { ptr::copy_nonoverlapping(lock_handle(original).cast::<u8>(), lock_handle(byte_copy).cast::<u8>(), handle_size(original) as usize); }
        ArbString::write(&mut original, &"replaced".repeat(1000)).unwrap();
        for handle in [copy, byte_copy] { assert_eq!(ArbString::read(handle), "中文工程🎥"); }
        let mut flat_size = 0;
        raw = extra(ae::sys::PF_Arbitrary_FLAT_SIZE_FUNC as _);
        raw.u.flat_size_func_params.arbH = copy;
        raw.u.flat_size_func_params.flat_data_sizePLu = &mut flat_size;
        callback(&mut raw).unwrap();
        let mut flat = vec![0; flat_size as usize];
        raw = extra(ae::sys::PF_Arbitrary_FLATTEN_FUNC as _);
        raw.u.flatten_func_params.arbH = copy;
        raw.u.flatten_func_params.buf_sizeLu = flat_size;
        raw.u.flatten_func_params.flat_dataPV = flat.as_mut_ptr().cast();
        callback(&mut raw).unwrap();
        assert_eq!(flat, ArbString::encode("中文工程🎥"));
        raw.u.flatten_func_params.buf_sizeLu = flat_size - 1;
        assert_eq!(callback(&mut raw), Err(ae::Error::InvalidParms));
        let mut restored = ptr::null_mut();
        raw = extra(ae::sys::PF_Arbitrary_UNFLATTEN_FUNC as _);
        raw.u.unflatten_func_params.flat_dataPV = flat.as_ptr().cast();
        raw.u.unflatten_func_params.buf_sizeLu = flat_size;
        raw.u.unflatten_func_params.arbPH = &mut restored;
        callback(&mut raw).unwrap();
        assert_eq!(ArbString::read(restored), "中文工程🎥");
        raw.u.unflatten_func_params.buf_sizeLu = flat_size - 1;
        assert_eq!(callback(&mut raw), Err(ae::Error::InternalStructDamaged));
        let mut interpolated = ptr::null_mut();
        raw = extra(ae::sys::PF_Arbitrary_INTERP_FUNC as _);
        raw.u.interp_func_params.left_arbH = copy;
        raw.u.interp_func_params.right_arbH = original;
        raw.u.interp_func_params.tF = 0.5;
        raw.u.interp_func_params.interpPH = &mut interpolated;
        callback(&mut raw).unwrap();
        assert_eq!(ArbString::read(interpolated), "中文工程🎥");
        let mut comparison: ae::sys::PF_ArbCompareResult = ae::sys::PF_ArbCompare_NOT_EQUAL as _;
        raw = extra(ae::sys::PF_Arbitrary_COMPARE_FUNC as _);
        raw.u.compare_func_params.a_arbH = copy;
        raw.u.compare_func_params.b_arbH = restored;
        raw.u.compare_func_params.compareP = &mut comparison;
        callback(&mut raw).unwrap();
        assert_eq!(comparison, ae::sys::PF_ArbCompare_EQUAL as ae::sys::PF_ArbCompareResult);
        let mut print_size = 0;
        raw = extra(ae::sys::PF_Arbitrary_PRINT_SIZE_FUNC as _);
        raw.u.print_size_func_params.arbH = copy;
        raw.u.print_size_func_params.print_sizePLu = &mut print_size;
        callback(&mut raw).unwrap();
        let mut printed = vec![0; print_size as usize];
        raw = extra(ae::sys::PF_Arbitrary_PRINT_FUNC as _);
        raw.u.print_func_params.arbH = copy;
        raw.u.print_func_params.print_sizeLu = print_size;
        raw.u.print_func_params.print_bufferPC = printed.as_mut_ptr().cast();
        callback(&mut raw).unwrap();
        assert_eq!(CStr::from_bytes_with_nul(&printed).unwrap().to_str().unwrap(), serde_json::to_string("中文工程🎥").unwrap());
        let mut scanned = ptr::null_mut();
        raw = extra(ae::sys::PF_Arbitrary_SCAN_FUNC as _);
        raw.u.scan_func_params.bufPC = printed.as_ptr().cast();
        raw.u.scan_func_params.bytes_to_scanLu = print_size;
        raw.u.scan_func_params.arbPH = &mut scanned;
        callback(&mut raw).unwrap();
        assert_eq!(ArbString::read(scanned), "中文工程🎥");
        for handle in [original, copy, byte_copy, restored, interpolated, scanned] {
            raw = extra(ae::sys::PF_Arbitrary_DISPOSE_FUNC as _);
            raw.u.dispose_func_params.arbH = handle;
            callback(&mut raw).unwrap();
        }
    });
}

#[test]
fn premiere_string_errors_are_returned_with_the_parameter_name() {
    with_premiere(|params| {
        let error = params.get_string(Params::ProjectPath).unwrap_err().to_string();
        assert!(error.contains("ProjectPath") && error.contains("InvalidParms"), "{error}");
        assert_eq!(HOST_PARAM.with(|s| s.borrow().calls.clone()), vec![(0, 12345)]);
        HOST_PARAM.with(|s| s.borrow_mut().bytes = Some(vec![1, 2]));
        assert!(params.get_string(Params::ProjectPath).unwrap_err().to_string().contains("InvalidParms"));
        HOST_PARAM.with(|s| s.borrow_mut().bytes = Some(ArbString::encode("E:/中文/工程.gyroflow")));
        assert_eq!(params.get_string(Params::ProjectPath).unwrap(), "E:/中文/工程.gyroflow");
    });
}

#[test]
fn loaded_names_stay_local_through_shared_manager_cache_hits() {
    with_premiere(|params| {
        for p in [Params::LoadedProject, Params::LoadedLens, Params::LoadedPreset] {
            assert_eq!(params.get_string(p).unwrap(), "");
        }
        params.set_string(Params::ProjectPath, "cached.gyroflow").unwrap();
        params.set_string(Params::InstanceId, "instance").unwrap();
        let context = Arc::new(LoadedContext {
            original_video_size: (3840, 2160), original_output_size: (3840, 2160),
            original_project_rotation: Some(0.0), original_project_trim_ranges: Vec::new(),
            container_media_rotation: Some(0), num_frames: 1, fps: 25.0,
            has_motion: true, loaded: true, loaded_project: "中文工程.gyroflow".into(),
            loaded_lens: "NiYien 镜头".into(), loaded_preset: None,
        });
        let manager = Arc::new(StabilizationManager::default());
        let cache = parking_lot::Mutex::new(LruCache::new(std::num::NonZeroUsize::new(2).unwrap()));
        cache.lock().put("cached.gyroflowfalseinstance".into(), LoadedManager { manager: manager.clone(), context });
        let mut instance = GyroflowPluginBaseInstance { reload_values_from_project: false, ..Default::default() };
        assert!(Arc::ptr_eq(&instance.stab_manager(params, &cache, (3840, 2160), false).unwrap(), &manager));
        assert_eq!(params.get_string(Params::LoadedProject).unwrap(), "中文工程.gyroflow");
        assert_eq!(params.get_string(Params::LoadedLens).unwrap(), "NiYien 镜头");
        assert_eq!(params.get_string(Params::LoadedPreset).unwrap(), "");
        params.stored.write().pending_params_str.remove(&Params::LoadedProject);
        instance.stab_manager(params, &cache, (3840, 2160), false).unwrap();
        assert_eq!(params.get_string(Params::LoadedProject).unwrap(), "中文工程.gyroflow");
        assert!(HOST_PARAM.with(|s| s.borrow().calls.is_empty()), "display names must not query Premiere");
    });
}

#[test]
fn ae_display_names_and_pending_strings_need_no_host_parameter() {
    let mut host_params = ae::Parameters::new();
    let mut params = ParamHandler { inner: ParamsInner::Ae(&mut host_params), stored: Arc::new(RwLock::new(StoredParams::default())) };
    for p in [Params::LoadedProject, Params::LoadedLens, Params::LoadedPreset] {
        assert_eq!(params.get_string(p).unwrap(), "");
        params.set_string(p, "本地名称").unwrap();
        assert_eq!(params.get_string(p).unwrap(), "本地名称");
    }
    params.set_string(Params::ProjectPath, "E:/工程.gyroflow").unwrap();
    assert_eq!(params.get_string(Params::ProjectPath).unwrap(), "E:/工程.gyroflow");
}
