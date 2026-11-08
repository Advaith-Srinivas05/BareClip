//! Which graphics chip to draw on, when the person has picked one in Settings.

use std::sync::Mutex;

/// A graphics chip, by the numbers its maker gave it. These stay the same when a driver changes
/// the name it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuId {
    pub vendor: u32,
    pub device: u32,
}

impl GpuId {
    pub fn of(info: &wgpu::AdapterInfo) -> Self {
        Self {
            vendor: info.vendor,
            device: info.device,
        }
    }

    /// Reads the form the hidden `BARECLIP_GPU` switch takes, such as `10de:2520`.
    pub fn parse(text: &str) -> Option<Self> {
        let (vendor, device) = text.trim().split_once(':')?;
        Some(Self {
            vendor: u32::from_str_radix(vendor.trim(), 16).ok()?,
            device: u32::from_str_radix(device.trim(), 16).ok()?,
        })
    }
}

/// Windows' software stand-in counts as an adapter, but is never a chip anyone would pick.
pub fn is_real_chip(info: &wgpu::AdapterInfo) -> bool {
    info.device_type != wgpu::DeviceType::Cpu
}

/// Which of these is the wanted chip, if it's still in this PC.
pub fn find_gpu(adapters: &[wgpu::AdapterInfo], wanted: GpuId) -> Option<usize> {
    adapters
        .iter()
        .position(|info| is_real_chip(info) && GpuId::of(info) == wanted)
}

/// The chip to use when none was picked or the picked one is gone: a separate graphics card
/// before one built into the processor, as asking Windows for the fast chip does.
pub fn automatic_gpu(adapters: &[wgpu::AdapterInfo]) -> Option<usize> {
    let of_type = |kind| adapters.iter().position(|info| info.device_type == kind);
    of_type(wgpu::DeviceType::DiscreteGpu)
        .or_else(|| of_type(wgpu::DeviceType::IntegratedGpu))
        .or_else(|| adapters.iter().position(is_real_chip))
}

static CHOSEN: Mutex<Option<GpuId>> = Mutex::new(None);

/// Sets the chip drawing happens on. It has to be set before the first export or layered preview,
/// because the GPU is only opened once in a run.
pub fn choose_gpu(chip: Option<GpuId>) {
    if let Ok(mut chosen) = CHOSEN.lock() {
        *chosen = chip;
    }
}

pub(crate) fn chosen() -> Option<GpuId> {
    CHOSEN.lock().ok().and_then(|chosen| *chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(name: &str, kind: wgpu::DeviceType, vendor: u32, device: u32) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: name.to_owned(),
            vendor,
            device,
            ..wgpu::AdapterInfo::new(kind, wgpu::Backend::Dx12)
        }
    }

    fn this_laptop() -> Vec<wgpu::AdapterInfo> {
        vec![
            chip(
                "AMD Radeon",
                wgpu::DeviceType::IntegratedGpu,
                0x1002,
                0x1638,
            ),
            chip(
                "NVIDIA RTX 3060",
                wgpu::DeviceType::DiscreteGpu,
                0x10de,
                0x2520,
            ),
            chip("Microsoft Basic", wgpu::DeviceType::Cpu, 0x1414, 0x8c),
        ]
    }

    #[test]
    fn a_picked_chip_is_found_by_its_numbers_not_its_place() {
        let wanted = GpuId {
            vendor: 0x1002,
            device: 0x1638,
        };

        assert_eq!(find_gpu(&this_laptop(), wanted), Some(0));
    }

    #[test]
    fn a_chip_no_longer_there_is_not_found() {
        let gone = GpuId {
            vendor: 0x10de,
            device: 0x1f08,
        };

        assert_eq!(find_gpu(&this_laptop(), gone), None);
    }

    #[test]
    fn windows_software_stand_in_can_never_be_picked() {
        let software = GpuId {
            vendor: 0x1414,
            device: 0x8c,
        };

        assert_eq!(find_gpu(&this_laptop(), software), None);
    }

    #[test]
    fn automatic_prefers_a_graphics_card() {
        assert_eq!(automatic_gpu(&this_laptop()), Some(1));
        let no_card = &this_laptop()[..1];
        assert_eq!(automatic_gpu(no_card), Some(0));
        assert_eq!(automatic_gpu(&this_laptop()[2..]), None);
    }

    #[test]
    fn the_hidden_switch_reads_two_hex_numbers() {
        assert_eq!(
            GpuId::parse(" 10DE:2520 "),
            Some(GpuId {
                vendor: 0x10de,
                device: 0x2520
            })
        );
        assert_eq!(GpuId::parse("nvidia"), None);
        assert_eq!(GpuId::parse("10de:zz"), None);
    }
}
