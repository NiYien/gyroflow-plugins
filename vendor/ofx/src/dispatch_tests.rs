use super::*;
use ofx_sys::*;
use plugin::RawMessage;
use registry::Registry;
use std::ffi::CStr;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

struct Host {
    data: AtomicPtr<std::ffi::c_void>,
    registry: Weak<Registry>,
    nested_status: AtomicUsize,
}

struct PrivateState {
    changes: usize,
    drops: Arc<AtomicUsize>,
}
impl Drop for PrivateState {
    fn drop(&mut self) { self.drops.fetch_add(1, Ordering::SeqCst); }
}

struct Effect {
    callbacks: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
}

impl Execute for Effect {
    fn execute(&self, context: &PluginContext, action: &mut Action) -> Result<Int> {
        match *action {
            Action::Load => OK,
            Action::CreateInstance(ref mut effect) => {
                effect.set_instance_data(PrivateState { changes: 0, drops: self.drops.clone() })?;
                OK
            }
            Action::Render(ref mut effect, _) => {
                let mut state = effect.get_instance_data::<PrivateState>()?;
                let properties: EffectInstance = effect.properties()?;
                let mut status: ParamHandle<String> = ParamHandle::new(
                    properties.handle() as OfxParamHandle, properties.handle(),
                    context.suites.property(), context.suites.parameter());
                status.set_label("ready")?;
                state.changes += 1;
                OK
            }
            Action::InstanceChanged(ref mut effect, _) => {
                self.callbacks.fetch_add(1, Ordering::SeqCst);
                effect.get_instance_data::<PrivateState>()?.changes += 1;
                OK
            }
            _ => REPLY_DEFAULT,
        }
    }
}

unsafe extern "C" fn get_properties(effect: OfxImageEffectHandle, out: *mut OfxPropertySetHandle) -> OfxStatus {
    *out = effect as OfxPropertySetHandle;
    eOfxStatus_OK
}

unsafe extern "C" fn get_pointer(props: OfxPropertySetHandle, _: *const i8, _: i32, out: *mut *mut std::ffi::c_void) -> OfxStatus {
    *out = (*(props as *const Host)).data.load(Ordering::SeqCst);
    eOfxStatus_OK
}

unsafe extern "C" fn set_pointer(props: OfxPropertySetHandle, _: *const i8, _: i32, value: *mut std::ffi::c_void) -> OfxStatus {
    (*(props as *const Host)).data.store(value, Ordering::SeqCst);
    eOfxStatus_OK
}

unsafe extern "C" fn get_string(_: OfxPropertySetHandle, name: *const i8, _: i32, out: *mut *const i8) -> OfxStatus {
    *out = if CStr::from_ptr(name).to_bytes() == b"OfxPropChangeReason" {
        b"OfxChangePluginEdited\0".as_ptr() as *const i8
    } else { b"Status\0".as_ptr() as *const i8 };
    eOfxStatus_OK
}

fn dispatch(registry: &Registry, host: &Host, action: &[u8]) -> Result<Int> {
    registry.dispatch("probe", RawMessage::MainEntry {
        action: action.as_ptr() as *const i8,
        handle: host as *const Host as VoidPtr,
        in_args: host as *const Host as OfxPropertySetHandle,
        out_args: host as *const Host as OfxPropertySetHandle,
    })
}

unsafe extern "C" fn set_string_property(props: OfxPropertySetHandle, _: *const i8, _: i32, _: *const i8) -> OfxStatus {
    let host = &*(props as *const Host);
    let registry = host.registry.upgrade().unwrap();
    let result = dispatch(&registry, host, b"OfxActionInstanceChanged\0");
    let preferences = dispatch(&registry, host, b"OfxImageEffectActionGetClipPreferences\0");
    if matches!(result, Ok(status) if status == eOfxStatus_OK)
        && matches!(preferences, Ok(status) if status == eOfxStatus_ReplyDefault) {
        host.nested_status.fetch_add(1, Ordering::SeqCst);
        eOfxStatus_OK
    } else { eOfxStatus_Failed }
}

unsafe extern "C" fn fetch_suite(_: OfxPropertySetHandle, name: *const i8, _: i32) -> *const std::ffi::c_void {
    static PROPERTY: OnceLock<OfxPropertySuiteV1> = OnceLock::new();
    static IMAGE: OnceLock<OfxImageEffectSuiteV1> = OnceLock::new();
    static EMPTY: [usize; 64] = [0; 64];
    match CStr::from_ptr(name).to_bytes() {
        b"OfxPropertySuite" => PROPERTY.get_or_init(|| {
            let mut suite: OfxPropertySuiteV1 = std::mem::zeroed();
            suite.propGetPointer = Some(get_pointer);
            suite.propSetPointer = Some(set_pointer);
            suite.propGetString = Some(get_string);
            suite.propSetString = Some(set_string_property);
            suite
        }) as *const _ as *const std::ffi::c_void,
        b"OfxImageEffectSuite" => IMAGE.get_or_init(|| {
            let mut suite: OfxImageEffectSuiteV1 = std::mem::zeroed();
            suite.getPropertySet = Some(get_properties);
            suite
        }) as *const _ as *const std::ffi::c_void,
        _ => EMPTY.as_ptr() as *const std::ffi::c_void,
    }
}

unsafe extern "C" fn unused_set_host(_: *mut OfxHost) {}
unsafe extern "C" fn unused_entry(_: *const i8, _: VoidPtr, _: OfxPropertySetHandle, _: OfxPropertySetHandle) -> Int { 0 }

#[test]
fn shared_suite_snapshots_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Suites>();
    check::<PluginContext>();
}

#[test]
fn dispatch_handles_sync_status_reentry_and_destroys_the_real_type() {
    let callbacks = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new();
    registry.add("probe", "probe", ApiVersion(1), PluginVersion(1, 0),
        Box::new(Effect { callbacks: callbacks.clone(), drops: drops.clone() }),
        unused_set_host, unused_entry);
    let registry = Arc::new(registry);
    let host = Box::new(Host { data: AtomicPtr::new(ptr::null_mut()), registry: Arc::downgrade(&registry),
        nested_status: AtomicUsize::new(0) });
    registry.dispatch("probe", RawMessage::SetHost { host: OfxHost {
        host: &*host as *const Host as OfxPropertySetHandle, fetchSuite: Some(fetch_suite),
    }}).unwrap();
    assert_eq!(dispatch(&registry, &host, b"OfxActionLoad\0").unwrap(), eOfxStatus_OK);
    dispatch(&registry, &host, b"OfxActionCreateInstance\0").unwrap();
    dispatch(&registry, &host, b"OfxImageEffectActionRender\0").unwrap();
    assert_eq!(host.nested_status.load(Ordering::SeqCst), 1);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    dispatch(&registry, &host, b"OfxActionInstanceChanged\0").unwrap();
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    dispatch(&registry, &host, b"OfxActionDestroyInstance\0").unwrap();
    assert!(host.data.load(Ordering::SeqCst).is_null());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
