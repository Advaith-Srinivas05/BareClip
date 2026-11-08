//! Which encoders get tried, and a hidden switch to leave some out.
//!
//! Setting `BARECLIP_SKIP_ENCODERS` to a list such as `nvenc` or `nvenc,amf,mf` makes BareClip
//! act as if those encoders weren't there. It exists so that a machine with an NVIDIA card can
//! run every export the way a machine without one would. Each word is a whole encoder name, such
//! as `h264_amf`, the part after its codec, such as `amf`, or the name without `lib`, such as
//! `openh264`.

use std::{
    env,
    sync::atomic::{AtomicU8, Ordering},
};

pub const SKIP_VARIABLE: &str = "BARECLIP_SKIP_ENCODERS";

/// Whose graphics chip an encoder runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipMaker {
    Nvidia,
    Amd,
    Intel,
}

impl ChipMaker {
    /// Who made a chip, from the PCI vendor number Windows reports for it.
    pub fn from_vendor(vendor: u32) -> Option<Self> {
        match vendor {
            0x10de => Some(Self::Nvidia),
            0x1002 | 0x1022 => Some(Self::Amd),
            0x8086 => Some(Self::Intel),
            _ => None,
        }
    }

    fn of_encoder(encoder: &str) -> Option<Self> {
        if encoder.ends_with("_nvenc") {
            Some(Self::Nvidia)
        } else if encoder.ends_with("_amf") {
            Some(Self::Amd)
        } else if encoder.ends_with("_qsv") {
            Some(Self::Intel)
        } else {
            None
        }
    }

    const fn code(maker: Option<Self>) -> u8 {
        match maker {
            None => 0,
            Some(Self::Nvidia) => 1,
            Some(Self::Amd) => 2,
            Some(Self::Intel) => 3,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Nvidia),
            2 => Some(Self::Amd),
            3 => Some(Self::Intel),
            _ => None,
        }
    }
}

static FOLLOWED: AtomicU8 = AtomicU8::new(ChipMaker::code(None));

/// Makes exports use only this maker's graphics encoders, plus those that need no graphics chip,
/// because someone who picks a chip in Settings doesn't expect another chip to wake up for an
/// export. `None` tries every encoder, which is the Automatic setting.
pub fn follow_chip(maker: Option<ChipMaker>) {
    FOLLOWED.store(ChipMaker::code(maker), Ordering::Relaxed);
}

/// Whether following this maker's chip leaves this encoder in.
fn follows(maker: Option<ChipMaker>, encoder: &str) -> bool {
    maker.is_none_or(|maker| ChipMaker::of_encoder(encoder).is_none_or(|own| own == maker))
}

/// Whether a word in the switch names this encoder.
fn names(word: &str, encoder: &str) -> bool {
    encoder == word
        || encoder
            .strip_suffix(word)
            .is_some_and(|rest| rest.ends_with('_'))
        || encoder.strip_prefix("lib") == Some(word)
}

/// Whether the switch's value leaves this encoder out.
pub(crate) fn is_skipped_by(list: &str, encoder: &str) -> bool {
    list.split([',', ';', ' '])
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .any(|word| names(&word.to_ascii_lowercase(), encoder))
}

/// The encoders in the order given, less any the switch or the chip picked in Settings leaves out.
pub(crate) fn usable<'a>(encoders: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let list = env::var(SKIP_VARIABLE).unwrap_or_default();
    let followed = ChipMaker::from_code(FOLLOWED.load(Ordering::Relaxed));
    encoders
        .into_iter()
        .filter(|encoder| !is_skipped_by(&list, encoder) && follows(followed, encoder))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_keeps_every_encoder() {
        for encoder in [
            "h264_nvenc",
            "h264_amf",
            "h264_qsv",
            "h264_mf",
            "libopenh264",
        ] {
            assert!(follows(None, encoder), "{encoder}");
        }
    }

    #[test]
    fn a_picked_chip_keeps_its_own_encoders_and_those_that_need_no_chip() {
        let kept: Vec<&str> = [
            "h264_nvenc",
            "h264_amf",
            "h264_qsv",
            "h264_mf",
            "libopenh264",
        ]
        .into_iter()
        .filter(|encoder| follows(Some(ChipMaker::Amd), encoder))
        .collect();

        assert_eq!(kept, ["h264_amf", "h264_mf", "libopenh264"]);
    }

    #[test]
    fn makers_are_known_by_their_vendor_numbers() {
        assert_eq!(ChipMaker::from_vendor(0x10de), Some(ChipMaker::Nvidia));
        assert_eq!(ChipMaker::from_vendor(0x1002), Some(ChipMaker::Amd));
        assert_eq!(ChipMaker::from_vendor(0x8086), Some(ChipMaker::Intel));
        assert_eq!(ChipMaker::from_vendor(0x1414), None);
    }

    #[test]
    fn a_family_name_leaves_out_every_codec_of_it() {
        assert!(is_skipped_by("nvenc", "h264_nvenc"));
        assert!(is_skipped_by("nvenc", "hevc_nvenc"));
        assert!(!is_skipped_by("nvenc", "h264_amf"));
    }

    #[test]
    fn several_can_be_named_in_any_case_with_spaces() {
        let list = " NVENC, amf;mf ";

        assert!(is_skipped_by(list, "h264_nvenc"));
        assert!(is_skipped_by(list, "h264_amf"));
        assert!(is_skipped_by(list, "hevc_mf"));
        assert!(!is_skipped_by(list, "h264_qsv"));
        assert!(!is_skipped_by(list, "libopenh264"));
    }

    #[test]
    fn whole_names_and_names_without_lib_work_too() {
        assert!(is_skipped_by("h264_amf", "h264_amf"));
        assert!(!is_skipped_by("h264_amf", "hevc_amf"));
        assert!(is_skipped_by("openh264", "libopenh264"));
    }

    #[test]
    fn a_word_must_match_a_whole_part_of_the_name() {
        // `mf` must not catch encoders that merely end in those letters.
        assert!(!is_skipped_by("mf", "h264_amf"));
        assert!(!is_skipped_by("264", "libopenh264"));
        assert!(!is_skipped_by("", "h264_nvenc"));
    }
}
