// SPDX-License-Identifier: GPL-3.0-or-later
use std::sync::OnceLock;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN,
};
use windows::core::PCWSTR;

pub fn retain_for_process() -> windows::core::Result<()> {
    static RETAINED: OnceLock<windows::core::Result<()>> = OnceLock::new();
    RETAINED.get_or_init(|| {
        let mut module = HMODULE::default();
        // Describe starts wgpu workers, and rendering can start the global Rayon pool.
        // They outlive OFX Unload, so their code must survive the host's FreeLibrary.
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                PCWSTR(retain_for_process as *const () as *const u16),
                &mut module,
            )
        }
    }).clone()
}
