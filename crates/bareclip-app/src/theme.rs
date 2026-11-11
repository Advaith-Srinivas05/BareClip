//! BareClip's light and dark themes. Colours match `assets/brand/tokens.json`.

use eframe::egui::{self, Color32, CornerRadius, Shadow, Stroke, Theme, ThemePreference, Visuals};

use crate::{
    components::{ActionButton, Kind},
    icons::{self, Icon},
};

// The timeline will use the clip, playhead and status colours.
#[cfg_attr(not(test), allow(dead_code))]
pub struct Palette {
    pub bg_app: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_text: Color32,
    pub selection: Color32,
    pub selection_text: Color32,
    pub playhead: Color32,
    pub playhead_outline: Color32,
    pub stage: Color32,
    pub clip_video: Color32,
    pub clip_video_text: Color32,
    pub clip_audio: Color32,
    pub clip_audio_text: Color32,
    pub clip_overlay: Color32,
    pub clip_overlay_text: Color32,
    /// Things from the content library.
    pub clip_shape: Color32,
    pub clip_shape_text: Color32,
    pub danger: Color32,
    pub success: Color32,
}

const fn hex(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

pub const DARK: Palette = Palette {
    bg_app: hex(0x111111),
    surface: hex(0x1A1918),
    raised: hex(0x262422),
    border: hex(0x403E3E),
    border_strong: hex(0x77726C),
    text: hex(0xF1EEE9),
    text_muted: hex(0xABA69F),
    accent: hex(0xC9A06A),
    accent_hover: hex(0xD9B98C),
    accent_text: hex(0x111111),
    selection: hex(0x806448),
    selection_text: hex(0xFFFFFF),
    playhead: hex(0xC9A06A),
    playhead_outline: hex(0x111111),
    stage: hex(0x000000),
    clip_video: hex(0x806448),
    clip_video_text: hex(0xFFFFFF),
    clip_audio: hex(0x68635E),
    clip_audio_text: hex(0xFFFFFF),
    clip_overlay: hex(0xC9A06A),
    clip_overlay_text: hex(0x111111),
    clip_shape: hex(0x7FA39A),
    clip_shape_text: hex(0x111111),
    danger: hex(0xE8836F),
    success: hex(0x9CBF7F),
};

pub const LIGHT: Palette = Palette {
    bg_app: hex(0xECEAE6),
    surface: hex(0xF3F2EF),
    raised: hex(0xFFFFFF),
    border: hex(0xCFCBC4),
    border_strong: hex(0x857F76),
    text: hex(0x1A1918),
    text_muted: hex(0x57534E),
    accent: hex(0x806448),
    accent_hover: hex(0x664E37),
    accent_text: hex(0xFFFFFF),
    selection: hex(0xC9A06A),
    selection_text: hex(0x111111),
    playhead: hex(0xC9A06A),
    playhead_outline: hex(0x111111),
    stage: hex(0x111111),
    clip_video: hex(0x806448),
    clip_video_text: hex(0xFFFFFF),
    clip_audio: hex(0x403E3E),
    clip_audio_text: hex(0xFFFFFF),
    clip_overlay: hex(0xA07F55),
    clip_overlay_text: hex(0x111111),
    clip_shape: hex(0x5E8A80),
    clip_shape_text: hex(0x111111),
    danger: hex(0xA93B2A),
    success: hex(0x47702F),
};

/// How wide each theme's button is, and all of them together with the spaces between.
const THEME_BUTTON_WIDTH: f32 = 84.0;
const THEME_BUTTON_GAP: f32 = 4.0;
pub const THEME_PICKER_WIDTH: f32 = THEME_BUTTON_WIDTH * 3.0 + THEME_BUTTON_GAP * 2.0;
const THEME_CHOICES: [(ThemePreference, Icon, &str); 3] = [
    (ThemePreference::System, icons::MONITOR, "System"),
    (ThemePreference::Light, icons::SUN, "Light"),
    (ThemePreference::Dark, icons::MOON, "Dark"),
];

/// How long panels take to slide and menus to fade, kept short so nothing feels slow.
const ANIMATION_SECONDS: f32 = 0.15;

/// The colours of the theme currently showing.
pub fn palette(ctx: &egui::Context) -> &'static Palette {
    match ctx.theme() {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

/// Registers both themes. The app follows the Windows setting until the user picks one.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, Theme::Dark));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, Theme::Light));
    ctx.all_styles_mut(|style| style.animation_time = ANIMATION_SECONDS);
}

/// Shows the theme choices, side by side in a row the caller has laid out.
pub fn theme_picker(ui: &mut egui::Ui, palette: &Palette) {
    let current = ui.ctx().options(|options| options.theme_preference);
    ui.spacing_mut().item_spacing.x = THEME_BUTTON_GAP;
    for &(choice, icon, label) in &THEME_CHOICES {
        let button = ActionButton::new(palette, Kind::Ghost, label)
            .icon(icon)
            .small()
            .width(THEME_BUTTON_WIDTH)
            .selected(current == choice);
        if ui.add(button).clicked() {
            ui.ctx().set_theme(choice);
        }
    }
}

const SHADOW_ALPHA: u8 = 70;

fn visuals(palette: &Palette, theme: Theme) -> Visuals {
    let mut visuals = match theme {
        Theme::Dark => Visuals::dark(),
        Theme::Light => Visuals::light(),
    };

    visuals.panel_fill = palette.surface;
    visuals.window_fill = palette.raised;
    visuals.window_stroke = Stroke::new(1.0, palette.border);
    visuals.extreme_bg_color = palette.bg_app;
    visuals.faint_bg_color = palette.raised;
    visuals.code_bg_color = palette.raised;
    visuals.hyperlink_color = palette.accent;
    visuals.warn_fg_color = palette.accent;
    visuals.error_fg_color = palette.danger;
    visuals.selection.bg_fill = palette.selection;
    visuals.selection.stroke = Stroke::new(1.0, palette.selection_text);

    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.menu_corner_radius = CornerRadius::same(8);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 22,
        spread: 0,
        color: Color32::from_black_alpha(SHADOW_ALPHA),
    };
    visuals.window_shadow = shadow;
    visuals.popup_shadow = shadow;

    let corner_radius = CornerRadius::same(6);
    let widgets = &mut visuals.widgets;

    widgets.noninteractive.bg_fill = palette.surface;
    widgets.noninteractive.weak_bg_fill = palette.surface;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);

    for state in [&mut widgets.inactive, &mut widgets.open] {
        state.bg_fill = palette.raised;
        state.weak_bg_fill = palette.raised;
        state.bg_stroke = Stroke::new(1.0, palette.border_strong);
        state.fg_stroke = Stroke::new(1.0, palette.text);
        state.corner_radius = corner_radius;
    }

    widgets.hovered.bg_fill = palette.raised;
    widgets.hovered.weak_bg_fill = palette.raised;
    widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent);
    widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.hovered.corner_radius = corner_radius;

    // egui also uses this text colour for all strong text, so it has to stay readable
    // on panels. That rules out filling pressed controls with the accent.
    widgets.active.bg_fill = palette.raised;
    widgets.active.weak_bg_fill = palette.raised;
    widgets.active.bg_stroke = Stroke::new(2.0, palette.accent);
    widgets.active.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.active.corner_radius = corner_radius;

    visuals
}

#[cfg(test)]
pub mod tests {
    use super::*;

    const TOKENS: &str = include_str!("../../../assets/brand/tokens.json");
    const TEXT_MINIMUM: f64 = 4.5;
    const SHAPE_MINIMUM: f64 = 3.0;

    fn themes() -> [(&'static str, Palette); 2] {
        [("dark", DARK), ("light", LIGHT)]
    }

    fn roles(palette: &Palette) -> Vec<(&'static str, Color32)> {
        vec![
            ("bg_app", palette.bg_app),
            ("surface", palette.surface),
            ("raised", palette.raised),
            ("border", palette.border),
            ("border_strong", palette.border_strong),
            ("text", palette.text),
            ("text_muted", palette.text_muted),
            ("accent", palette.accent),
            ("accent_hover", palette.accent_hover),
            ("accent_text", palette.accent_text),
            ("selection", palette.selection),
            ("selection_text", palette.selection_text),
            ("playhead", palette.playhead),
            ("playhead_outline", palette.playhead_outline),
            ("stage", palette.stage),
            ("clip_video", palette.clip_video),
            ("clip_video_text", palette.clip_video_text),
            ("clip_audio", palette.clip_audio),
            ("clip_audio_text", palette.clip_audio_text),
            ("clip_overlay", palette.clip_overlay),
            ("clip_overlay_text", palette.clip_overlay_text),
            ("clip_shape", palette.clip_shape),
            ("clip_shape_text", palette.clip_shape_text),
            ("danger", palette.danger),
            ("success", palette.success),
        ]
    }

    fn token_section(name: &str) -> &'static str {
        let start = TOKENS
            .find(&format!("\"{name}\": {{"))
            .expect("section exists");
        let length = TOKENS[start..].find('}').expect("section ends");
        &TOKENS[start..start + length]
    }

    fn hex_string(color: Color32) -> String {
        format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
    }

    fn luminance(color: Color32) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    /// The colour seen when a see-through colour sits on a solid one.
    pub fn over(top: Color32, bottom: Color32) -> Color32 {
        let rest = 255 - u16::from(top.a());
        let mix = |top: u8, bottom: u8| {
            (u16::from(top) + (u16::from(bottom) * rest + 127) / 255).min(255) as u8
        };
        Color32::from_rgb(
            mix(top.r(), bottom.r()),
            mix(top.g(), bottom.g()),
            mix(top.b(), bottom.b()),
        )
    }

    pub fn contrast(a: Color32, b: Color32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn contrast_matches_known_values() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.001);
        assert!((contrast(hex(0xC9A06A), hex(0x111111)) - 7.84).abs() < 0.01);
    }

    #[test]
    fn palettes_match_the_brand_tokens() {
        for (name, palette) in themes() {
            let section = token_section(name);
            for (role, color) in roles(&palette) {
                let expected = format!("\"{role}\": \"{}\"", hex_string(color));
                assert!(section.contains(&expected), "{name} {expected} differs");
            }
        }
    }

    #[test]
    fn every_token_exists_in_the_palette() {
        for (name, palette) in themes() {
            let tokens = token_section(name).matches("\": \"#").count();
            assert_eq!(tokens, roles(&palette).len(), "{name} role count differs");
        }
    }

    #[test]
    fn logo_colours_are_used_in_both_themes() {
        let logo = token_section("logo");
        for (name, palette) in themes() {
            let colors: Vec<String> = roles(&palette)
                .iter()
                .map(|(_, color)| hex_string(*color))
                .collect();
            for logo_color in ["#111111", "#403E3E", "#806448", "#C9A06A"] {
                assert!(logo.contains(logo_color));
                assert!(
                    colors.iter().any(|color| color == logo_color),
                    "{name} does not use {logo_color}"
                );
            }
        }
    }

    #[test]
    fn text_is_readable_in_both_themes() {
        for (name, p) in themes() {
            let pairs = [
                ("text on app", p.text, p.bg_app),
                ("text on panel", p.text, p.surface),
                ("text on raised", p.text, p.raised),
                ("muted on app", p.text_muted, p.bg_app),
                ("muted on panel", p.text_muted, p.surface),
                ("muted on raised", p.text_muted, p.raised),
                ("accent on app", p.accent, p.bg_app),
                ("accent on panel", p.accent, p.surface),
                ("accent on raised", p.accent, p.raised),
                ("button", p.accent_text, p.accent),
                ("button hover", p.accent_text, p.accent_hover),
                ("selection", p.selection_text, p.selection),
                ("video clip", p.clip_video_text, p.clip_video),
                ("audio clip", p.clip_audio_text, p.clip_audio),
                ("overlay clip", p.clip_overlay_text, p.clip_overlay),
                ("library clip", p.clip_shape_text, p.clip_shape),
                ("danger on panel", p.danger, p.surface),
                ("danger on raised", p.danger, p.raised),
                ("success on panel", p.success, p.surface),
                ("success on raised", p.success, p.raised),
            ];
            for (label, foreground, background) in pairs {
                let ratio = contrast(foreground, background);
                assert!(ratio >= TEXT_MINIMUM, "{name} {label} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn controls_and_clips_stand_out_in_both_themes() {
        for (name, p) in themes() {
            let pairs = [
                ("outline on app", p.border_strong, p.bg_app),
                ("outline on panel", p.border_strong, p.surface),
                ("outline on raised", p.border_strong, p.raised),
                ("video clip", p.clip_video, p.bg_app),
                ("audio clip", p.clip_audio, p.bg_app),
                ("overlay clip", p.clip_overlay, p.bg_app),
                ("library clip", p.clip_shape, p.bg_app),
            ];
            for (label, shape, background) in pairs {
                let ratio = contrast(shape, background);
                assert!(ratio >= SHAPE_MINIMUM, "{name} {label} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn playhead_stays_visible_over_everything_on_the_timeline() {
        for (name, p) in themes() {
            let backgrounds = [
                ("timeline", p.bg_app),
                ("video clip", p.clip_video),
                ("audio clip", p.clip_audio),
                ("overlay clip", p.clip_overlay),
                ("library clip", p.clip_shape),
            ];
            for (label, background) in backgrounds {
                let ratio =
                    contrast(p.playhead, background).max(contrast(p.playhead_outline, background));
                assert!(
                    ratio >= SHAPE_MINIMUM,
                    "{name} playhead on {label} is {ratio:.2}:1"
                );
            }
        }
    }

    #[test]
    fn strong_text_is_readable_in_both_themes() {
        for (name, palette, theme) in [
            ("dark", &DARK, Theme::Dark),
            ("light", &LIGHT, Theme::Light),
        ] {
            let strong = visuals(palette, theme).strong_text_color();
            for background in [palette.surface, palette.raised, palette.bg_app] {
                let ratio = contrast(strong, background);
                assert!(ratio >= TEXT_MINIMUM, "{name} strong text is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn stage_stays_dark_in_both_themes() {
        for (name, palette) in themes() {
            assert!(
                luminance(palette.stage) < 0.01,
                "{name} stage is too bright"
            );
        }
    }

    #[test]
    fn themes_set_the_right_mode() {
        assert!(visuals(&DARK, Theme::Dark).dark_mode);
        assert!(!visuals(&LIGHT, Theme::Light).dark_mode);
    }

    #[test]
    fn a_see_through_colour_mixes_with_what_is_under_it() {
        assert_eq!(over(Color32::TRANSPARENT, hex(0x123456)), hex(0x123456));
        assert_eq!(over(hex(0x123456), Color32::WHITE), hex(0x123456));
        let half = Color32::WHITE.gamma_multiply(0.5);
        let mixed = over(half, Color32::BLACK);
        assert!((i16::from(mixed.r()) - i16::from(half.r())).abs() <= 1);
    }

    #[test]
    fn every_theme_choice_has_its_own_symbol() {
        let mut names: Vec<&str> = THEME_CHOICES.iter().map(|(_, icon, _)| icon.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), THEME_CHOICES.len());
    }

    #[test]
    fn install_registers_both_themes() {
        let ctx = egui::Context::default();
        install(&ctx);

        ctx.set_theme(Theme::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, DARK.surface);
        ctx.set_theme(Theme::Light);
        assert_eq!(ctx.global_style().visuals.panel_fill, LIGHT.surface);
        assert_eq!(ctx.global_style().animation_time, ANIMATION_SECONDS);
    }
}
