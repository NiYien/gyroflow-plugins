extern crate ofx;

use ofx::*;

mod gyroflow;
mod fuscript;
mod drt_sizing;
mod drt_state;
mod jianying_protocol;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod jianying_autoload;
mod pending_host_values;

// Report the compiled sampling selection without initializing a host or GPU.
#[unsafe(no_mangle)]
pub extern "C" fn gyroflow_ofx_fragment_sampling_policy_v1() -> u32 {
    u32::from(cfg!(feature = "integer-fragment-sampling"))
}

register_modules!(gyroflow);
