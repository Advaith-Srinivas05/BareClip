//! Which encoders get tried, and a hidden switch to leave some out.
//!
//! Setting `BARECLIP_SKIP_ENCODERS` to a list such as `nvenc` or `nvenc,amf,mf` makes BareClip
//! act as if those encoders weren't there. It exists so that a machine with an NVIDIA card can
//! run every export the way a machine without one would. Each word is a whole encoder name, such
//! as `h264_amf`, the part after its codec, such as `amf`, or the name without `lib`, such as
//! `openh264`.

use std::env;

pub const SKIP_VARIABLE: &str = "BARECLIP_SKIP_ENCODERS";

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

/// The encoders in the order given, less any the switch leaves out.
pub(crate) fn usable<'a>(encoders: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let list = env::var(SKIP_VARIABLE).unwrap_or_default();
    encoders
        .into_iter()
        .filter(|encoder| !is_skipped_by(&list, encoder))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
