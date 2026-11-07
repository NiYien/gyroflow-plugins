// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{Buffers, KernelParamsFlags, StabilizationManager};
#[cfg(any(target_os = "macos", test))]
use crate::BufferSource;

// Keep the existing ABI usable with formal cores that predate this optional bit.
pub const INTEGER_FRAGMENT_COORDINATES_BIT: i32 = 1 << 14;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FragmentSamplingPolicy {
    #[default]
    CoreDefault,
    IntegerCoordinates,
}

impl FragmentSamplingPolicy {
    pub fn name(self) -> &'static str {
        match self {
            Self::CoreDefault => "core-default",
            Self::IntegerCoordinates => "integer-coordinates",
        }
    }

    pub fn supported_by(self, macos: bool, known_flags: i32) -> bool {
        self == Self::CoreDefault
            || macos && known_flags & INTEGER_FRAGMENT_COORDINATES_BIT != 0
    }

    pub fn is_supported(self) -> bool {
        self.supported_by(cfg!(target_os = "macos"), KernelParamsFlags::all().bits())
    }

    pub fn apply(self, stab: &StabilizationManager, _buffers: &Buffers) -> Result<(), &'static str> {
        if !self.is_supported() {
            return Err("The selected fragment sampling policy is unavailable in this core/platform");
        }
        #[cfg(target_os = "macos")]
        if self == Self::IntegerCoordinates
            && (!matches!(_buffers.input.data, BufferSource::MetalBuffer { .. })
                || !matches!(_buffers.output.data, BufferSource::MetalBuffer { .. })) {
            return Err("Integer fragment sampling requires two Metal buffers");
        }
        let flag = KernelParamsFlags::from_bits_truncate(INTEGER_FRAGMENT_COORDINATES_BIT);
        // Reuse the existing flags cache key and remove a prior opt-in on a policy change.
        stab.stabilization.write().kernel_flags.set(flag, self == Self::IntegerCoordinates);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BufferDescription;

    #[test]
    fn core_default_does_not_require_optional_core_capabilities() {
        for macos in [false, true] {
            for flags in [0, (1 << 14) - 1, 1 << 14, 1 << 15] {
                assert!(FragmentSamplingPolicy::CoreDefault.supported_by(macos, flags));
            }
        }
    }

    #[test]
    fn integer_policy_requires_explicit_platform_and_capability() {
        let policy = FragmentSamplingPolicy::IntegerCoordinates;
        assert!(!policy.supported_by(false, 1 << 14));
        assert!(!policy.supported_by(true, (1 << 14) - 1));
        assert!(!policy.supported_by(true, 1 << 15));
        assert!(policy.supported_by(true, 1 << 14));
    }

    fn descriptor(data: BufferSource) -> BufferDescription {
        BufferDescription { size: (1, 1, 4), rect: None, data, rotation: None,
            texture_copy: false, post_affine: None, flip_h: false, flip_v: false }
    }

    #[test]
    fn default_restores_core_sampling_without_changing_other_flags() {
        let stab = StabilizationManager::default();
        let all = KernelParamsFlags::all();
        stab.stabilization.write().kernel_flags = all.clone();
        let mut input = [0u8; 4];
        let mut output = [0u8; 4];
        let buffers = Buffers { input: descriptor(BufferSource::Cpu { buffer: &mut input }),
            output: descriptor(BufferSource::Cpu { buffer: &mut output }) };
        FragmentSamplingPolicy::CoreDefault.apply(&stab, &buffers).unwrap();
        assert_eq!(stab.stabilization.read().kernel_flags.bits(),
            all.bits() & !INTEGER_FRAGMENT_COORDINATES_BIT);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn integer_policy_rejects_non_macos_without_changing_flags() {
        let stab = StabilizationManager::default();
        let original = stab.stabilization.read().kernel_flags.bits();
        let mut input = [0u8; 4];
        let mut output = [0u8; 4];
        let buffers = Buffers { input: descriptor(BufferSource::Cpu { buffer: &mut input }),
            output: descriptor(BufferSource::Cpu { buffer: &mut output }) };
        assert!(FragmentSamplingPolicy::IntegerCoordinates.apply(&stab, &buffers).is_err());
        assert_eq!(stab.stabilization.read().kernel_flags.bits(), original);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn integer_rejection_leaves_manager_flags_unchanged() {
        let stab = StabilizationManager::default();
        let original = stab.stabilization.read().kernel_flags.bits();
        let mut input = [0u8; 4];
        let buffers = Buffers { input: descriptor(BufferSource::Cpu { buffer: &mut input }),
            output: descriptor(BufferSource::MetalBuffer { buffer: 2usize as _, command_queue: 3usize as _ }) };
        assert!(FragmentSamplingPolicy::IntegerCoordinates.apply(&stab, &buffers).is_err());
        assert_eq!(stab.stabilization.read().kernel_flags.bits(), original);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn explicit_integer_policy_uses_the_existing_core_bit_only() {
        let stab = StabilizationManager::default();
        let original = stab.stabilization.read().kernel_flags.bits();
        let buffers = Buffers {
            input: descriptor(BufferSource::MetalBuffer { buffer: 1usize as _, command_queue: 3usize as _ }),
            output: descriptor(BufferSource::MetalBuffer { buffer: 2usize as _, command_queue: 3usize as _ }) };
        let policy = FragmentSamplingPolicy::IntegerCoordinates;
        assert_eq!(policy.apply(&stab, &buffers).is_ok(), policy.is_supported());
        assert_eq!(stab.stabilization.read().kernel_flags.bits(),
            original | if policy.is_supported() { INTEGER_FRAGMENT_COORDINATES_BIT } else { 0 });
    }
}
