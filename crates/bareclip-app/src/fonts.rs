//! BareClip's UI font. Hanken Grotesk is used because its default digits are all
//! the same width, which stops timecodes jittering. egui cannot request tabular
//! digits through OpenType features, so this has to be true of the font itself.

use std::{collections::BTreeMap, sync::Arc};

use eframe::egui::{
    self, FontData, FontDefinitions, FontFamily, FontId, FontTweak, TextStyle,
    epaint::text::VariationCoords,
};

const HANKEN_GROTESK: &[u8] = include_bytes!("../../../assets/fonts/HankenGrotesk-Variable.ttf");
const REGULAR: &str = "hanken-grotesk-regular";
const SEMIBOLD: &str = "hanken-grotesk-semibold";
const STRONG_FAMILY: &str = "strong";

const REGULAR_WEIGHT: f32 = 400.0;
const SEMIBOLD_WEIGHT: f32 = 600.0;

fn weighted(weight: f32) -> Arc<FontData> {
    Arc::new(FontData::from_static(HANKEN_GROTESK).tweak(FontTweak {
        coords: VariationCoords::new([(b"wght", weight)]),
        ..Default::default()
    }))
}

fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(REGULAR.to_owned(), weighted(REGULAR_WEIGHT));
    fonts
        .font_data
        .insert(SEMIBOLD.to_owned(), weighted(SEMIBOLD_WEIGHT));

    // egui's own fonts stay as fallbacks for characters Hanken Grotesk does not have.
    let fallbacks = fonts.families[&FontFamily::Proportional].clone();
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .expect("egui defines a proportional family")
        .insert(0, REGULAR.to_owned());

    let mut strong = vec![SEMIBOLD.to_owned()];
    strong.extend(fallbacks);
    fonts
        .families
        .insert(FontFamily::Name(STRONG_FAMILY.into()), strong);
    fonts
}

fn text_styles() -> BTreeMap<TextStyle, FontId> {
    let regular = |size| FontId::new(size, FontFamily::Proportional);
    BTreeMap::from([
        (TextStyle::Small, regular(11.0)),
        (TextStyle::Body, regular(13.0)),
        (TextStyle::Button, regular(13.0)),
        (
            TextStyle::Heading,
            FontId::new(20.0, FontFamily::Name(STRONG_FAMILY.into())),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ])
}

/// Semibold text at a given size.
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG_FAMILY.into()))
}

/// Windows fonts that cover writing systems Hanken Grotesk does not.
const WINDOWS_FONTS: &str = "C:\\Windows\\Fonts";

/// The Windows font file needed to show a character, if it needs one at all.
fn fallback_file_for(character: char) -> Option<&'static str> {
    match u32::from(character) {
        // Devanagari through Sinhala, used across South Asia
        0x0900..=0x0DFF => Some("Nirmala.ttc"),
        // Thai and Lao
        0x0E00..=0x0EFF => Some("LeelawUI.ttf"),
        // Hangul
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Some("malgun.ttf"),
        // Kana
        0x3040..=0x30FF => Some("YuGothR.ttc"),
        // Chinese characters, also used in Japanese
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Some("msyh.ttc"),
        // Greek, Cyrillic, Hebrew, Arabic and others
        0x0370..=0x08FF => Some("segoeui.ttf"),
        _ => None,
    }
}

/// The font files needed to show some text, each listed once.
pub fn fallback_files_for(text: &str) -> Vec<&'static str> {
    let mut files = Vec::new();
    for file in text.chars().filter_map(fallback_file_for) {
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
}

/// Loads any extra fonts needed to show this text, such as a file name in another
/// script. They load only when needed, because they are large and slow startup.
pub fn load_fallbacks_for(ctx: &egui::Context, text: &str, loaded: &mut Vec<&'static str>) {
    let wanted: Vec<&'static str> = fallback_files_for(text)
        .into_iter()
        .filter(|file| !loaded.contains(file))
        .collect();
    if wanted.is_empty() {
        return;
    }

    let mut fonts = definitions();
    loaded.extend(wanted);
    for file in loaded.iter() {
        let path = std::path::Path::new(WINDOWS_FONTS).join(file);
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        fonts
            .font_data
            .insert((*file).to_owned(), Arc::new(FontData::from_owned(bytes)));
        for family in [
            FontFamily::Proportional,
            FontFamily::Name(STRONG_FAMILY.into()),
        ] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.push((*file).to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
    ctx.all_styles_mut(|style| style.text_styles = text_styles());
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Color32, Pos2, Rect, vec2};

    const LICENSE: &str = include_str!("../../../assets/fonts/OFL.txt");

    fn with_fonts<R>(read: impl FnOnce(&egui::Context) -> R) -> R {
        let ctx = egui::Context::default();
        install(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        // Fonts set on one frame take effect on the next.
        ctx.run_ui(input(), |_| {}).textures_delta.clear();
        ctx.begin_pass(input());
        let result = read(&ctx);
        ctx.end_pass().textures_delta.clear();
        result
    }

    fn width(ctx: &egui::Context, text: &str, font: &FontId) -> f32 {
        ctx.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    }

    fn body() -> FontId {
        text_styles()[&TextStyle::Body].clone()
    }

    fn heading() -> FontId {
        text_styles()[&TextStyle::Heading].clone()
    }

    #[test]
    fn all_digits_have_the_same_width() {
        with_fonts(|ctx| {
            for font in [body(), heading()] {
                let zero = width(ctx, "0", &font);
                for digit in "123456789".chars() {
                    let digit_width = width(ctx, &digit.to_string(), &font);
                    assert!(
                        (digit_width - zero).abs() < 0.01,
                        "{digit} is {digit_width} wide and 0 is {zero} wide in {font:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn timecodes_keep_the_same_width_as_they_count() {
        with_fonts(|ctx| {
            let font = heading();
            let narrowest = width(ctx, "00:00:11.11", &font);
            for timecode in ["00:00:55.17", "12:34:56.78", "88:88:88.88"] {
                let current = width(ctx, timecode, &font);
                assert!(
                    (current - narrowest).abs() < 0.01,
                    "{timecode} changed width from {narrowest} to {current}"
                );
            }
        });
    }

    #[test]
    fn heading_is_heavier_than_body_text() {
        with_fonts(|ctx| {
            let size = 40.0;
            let regular = FontId::new(size, FontFamily::Proportional);
            let strong = FontId::new(size, FontFamily::Name(STRONG_FAMILY.into()));
            let sample = "Gameplay highlights";

            assert!(width(ctx, sample, &strong) > width(ctx, sample, &regular));
        });
    }

    #[test]
    fn hanken_grotesk_is_the_first_choice_in_both_families() {
        let fonts = definitions();

        assert_eq!(fonts.families[&FontFamily::Proportional][0], REGULAR);
        assert_eq!(
            fonts.families[&FontFamily::Name(STRONG_FAMILY.into())][0],
            SEMIBOLD
        );
    }

    #[test]
    fn fallback_fonts_are_kept() {
        let defaults = FontDefinitions::default();
        let fonts = definitions();
        let original = &defaults.families[&FontFamily::Proportional];

        assert!(original.iter().all(|font| {
            fonts.families[&FontFamily::Proportional].contains(font)
                && fonts.families[&FontFamily::Name(STRONG_FAMILY.into())].contains(font)
        }));
    }

    #[test]
    fn plain_english_needs_no_extra_fonts() {
        assert!(fallback_files_for("ranked-match (final) 2026.mkv").is_empty());
    }

    #[test]
    fn other_scripts_ask_for_a_font_that_covers_them() {
        assert_eq!(
            fallback_files_for("\u{0935}\u{093f}\u{0921}.mp4"),
            ["Nirmala.ttc"]
        );
        assert_eq!(fallback_files_for("\u{3042}.mp4"), ["YuGothR.ttc"]);
        assert_eq!(fallback_files_for("\u{d55c}.mp4"), ["malgun.ttf"]);
        assert_eq!(fallback_files_for("\u{4e2d}.mp4"), ["msyh.ttc"]);
        assert_eq!(fallback_files_for("\u{0416}.mp4"), ["segoeui.ttf"]);
    }

    #[test]
    fn each_font_is_listed_once_even_with_mixed_text() {
        assert_eq!(
            fallback_files_for("\u{0416}\u{0417} \u{3042}\u{3043}"),
            ["segoeui.ttf", "YuGothR.ttc"]
        );
    }

    #[test]
    fn extra_fonts_are_only_loaded_once() {
        let ctx = egui::Context::default();
        let mut loaded = Vec::new();

        load_fallbacks_for(&ctx, "\u{0416}", &mut loaded);
        load_fallbacks_for(&ctx, "\u{0417}", &mut loaded);

        assert_eq!(loaded, ["segoeui.ttf"]);
    }

    #[test]
    fn font_licence_ships_with_the_font() {
        assert!(LICENSE.contains("SIL OPEN FONT LICENSE"));
        assert!(LICENSE.contains("Hanken Grotesk"));
    }
}
