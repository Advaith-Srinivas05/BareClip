//! The fonts titles can use: the free ones that ship with BareClip, and whatever is on this PC.

use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard, OnceLock},
};

use cosmic_text::{FontSystem, SwashCache, fontdb};

/// Fonts that ship with BareClip. Their licence is in `assets/licenses/TitleFonts-OFL.txt`.
const SHIPPED: [&[u8]; 21] = [
    include_bytes!("../../../assets/fonts/HankenGrotesk-Variable.ttf"),
    include_bytes!("../../../assets/fonts/titles/Abel.ttf"),
    include_bytes!("../../../assets/fonts/titles/AbrilFatface.ttf"),
    include_bytes!("../../../assets/fonts/titles/AlegreyaSans.ttf"),
    include_bytes!("../../../assets/fonts/titles/AlfaSlabOne.ttf"),
    include_bytes!("../../../assets/fonts/titles/Allura.ttf"),
    include_bytes!("../../../assets/fonts/titles/Arimo.ttf"),
    include_bytes!("../../../assets/fonts/titles/DMSans.ttf"),
    include_bytes!("../../../assets/fonts/titles/Raleway.ttf"),
    include_bytes!("../../../assets/fonts/titles/Rubik.ttf"),
    include_bytes!("../../../assets/fonts/titles/SourceSans3.ttf"),
    include_bytes!("../../../assets/fonts/titles/SpaceMono.ttf"),
    include_bytes!("../../../assets/fonts/titles/Sriracha.ttf"),
    include_bytes!("../../../assets/fonts/titles/Staatliches.ttf"),
    include_bytes!("../../../assets/fonts/titles/SueEllenFrancisco.ttf"),
    include_bytes!("../../../assets/fonts/titles/Tajawal.ttf"),
    include_bytes!("../../../assets/fonts/titles/VampiroOne.ttf"),
    include_bytes!("../../../assets/fonts/titles/VinaSans.ttf"),
    include_bytes!("../../../assets/fonts/titles/WorkSans.ttf"),
    include_bytes!("../../../assets/fonts/titles/Yesteryear.ttf"),
    include_bytes!("../../../assets/fonts/titles/Zeyada.ttf"),
];

/// Which fonts can be chosen, each list in order of name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontList {
    /// These look the same on every PC.
    pub shipped: Vec<String>,
    /// Only here, so a project using one can look different on another PC.
    pub on_this_pc: Vec<String>,
}

pub(crate) struct Engine {
    pub(crate) system: FontSystem,
    pub(crate) glyphs: SwashCache,
    list: FontList,
}

fn family_of(face: &fontdb::FaceInfo) -> Option<&str> {
    face.families.first().map(|(name, _)| name.as_str())
}

impl Engine {
    fn new() -> Self {
        let mut database = fontdb::Database::new();
        for font in SHIPPED {
            database.load_font_data(font.to_vec());
        }
        let shipped: BTreeSet<String> = database
            .faces()
            .filter_map(family_of)
            .map(str::to_owned)
            .collect();
        database.load_system_fonts();
        let on_this_pc: BTreeSet<String> = database
            .faces()
            .filter_map(family_of)
            // Windows lists sideways versions of some fonts under a name starting with @.
            .filter(|name| !name.starts_with('@') && !shipped.contains(*name))
            .map(str::to_owned)
            .collect();
        let locale = std::env::var("LANG").unwrap_or_else(|_| "en-US".to_owned());
        Self {
            system: FontSystem::new_with_locale_and_db(locale, database),
            glyphs: SwashCache::new(),
            list: FontList {
                shipped: shipped.into_iter().collect(),
                on_this_pc: on_this_pc.into_iter().collect(),
            },
        }
    }
}

/// Fonts are found the first time a title needs one, so a project with no titles never pays for
/// it. It took about 60 ms with 193 fonts on the PC.
pub(crate) fn engine() -> MutexGuard<'static, Engine> {
    static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Mutex::new(Engine::new()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// Which fonts can be chosen.
pub fn font_list() -> FontList {
    engine().list.clone()
}

/// Whether a font can be used on this PC.
pub fn has_font(name: &str) -> bool {
    let engine = engine();
    engine.list.shipped.iter().any(|font| font == name)
        || engine.list.on_this_pc.iter().any(|font| font == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_font_loads_and_has_a_name() {
        let list = font_list();
        assert_eq!(list.shipped.len(), SHIPPED.len(), "{:?}", list.shipped);
        for name in [
            "Abel",
            "Abril Fatface",
            "Alfa Slab One",
            "DM Sans",
            "Hanken Grotesk",
            "Rubik",
            "Space Mono",
            "Staatliches",
            "Zeyada",
        ] {
            assert!(list.shipped.iter().any(|font| font == name), "{name}");
        }
    }

    #[test]
    fn a_font_is_only_listed_in_one_place() {
        let list = font_list();
        for font in &list.on_this_pc {
            assert!(!list.shipped.contains(font), "{font}");
            assert!(!font.starts_with('@'), "{font}");
        }
    }

    #[test]
    fn the_font_titles_start_in_is_there() {
        assert!(has_font(bareclip_core::title::DEFAULT_FONT));
        assert!(!has_font("No Font Is Called This"));
    }
}
