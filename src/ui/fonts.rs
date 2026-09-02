//! Embedded font setup — extends egui's default font fallback chain with the
//! full monochrome Noto Emoji font (`assets/fonts/NotoEmoji-Regular.ttf`,
//! OFL — see `assets/fonts/OFL-NotoEmoji.txt`).
//!
//! egui's built-in chain (Ubuntu-Light + a ~900-glyph NotoEmoji *subset* +
//! emoji-icon-font) has narrow emoji coverage: user-picked action/category
//! icons such as 🧹 (U+1F9F9), 🏗 (U+1F3D7), 🤖 (U+1F916) or 🧪 (U+1F9EA)
//! are in none of the built-in fonts and rendered as tofu (empty boxes) in
//! the Actions and Préférences views. Appending the full Noto Emoji font
//! *last* keeps every glyph the default chain already renders (fallback is
//! first-hit-wins, so existing glyphs are unaffected) while filling the
//! gaps for the rest of the emoji range.
//!
//! Note this fixes emoji only: non-emoji symbols (e.g. the small triangles
//! ▸/▾ U+25B8/25BE, ▲/▼ U+25B2/25BC, or the plain arrows ↑/↓ U+2191/2193)
//! are covered by *no* font in the chain, before or after this change — UI
//! code must stick to covered glyphs (⏵/⏷, ⬆/⬇, ★/☆…) for hard-coded
//! labels.

/// Name under which the embedded font is registered in `FontDefinitions`.
const FONT_NAME: &str = "noto-emoji-full";
#[cfg(target_os = "macos")]
const SYSTEM_FONT_NAME: &str = "devtoolbox-system";

static NOTO_EMOJI: &[u8] = include_bytes!("../../assets/fonts/NotoEmoji-Regular.ttf");

/// egui's default `FontDefinitions` with the full Noto Emoji font appended
/// as the last fallback of both the `Proportional` and `Monospace` families.
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    #[cfg(target_os = "macos")]
    if let Some(bytes) = macos_system_font() {
        fonts.font_data.insert(
            SYSTEM_FONT_NAME.to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, SYSTEM_FONT_NAME.to_owned());
    }
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(NOTO_EMOJI)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(FONT_NAME.to_owned());
    }
    fonts
}

#[cfg(any(target_os = "macos", test))]
fn valid_sfnt(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x00, 0x01, 0x00, 0x00])
        || bytes.starts_with(b"OTTO")
        || bytes.starts_with(b"true")
}

#[cfg(target_os = "macos")]
fn macos_system_font() -> Option<Vec<u8>> {
    [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSRounded.ttf",
    ]
    .iter()
    .find_map(|path| std::fs::read(path).ok().filter(|bytes| valid_sfnt(bytes)))
}

/// Install the extended font chain on the given context. Called once from
/// `EguiApp::new` with `cc.egui_ctx`.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the "pictos manquants" (tofu) report: every glyph
    /// listed here must be covered by the extended chain.
    ///
    /// Caveat on the glyph list: epaint's `Fonts::has_glyph` has a known
    /// false negative (a `TODO(emilk)` in epaint 0.35) for any glyph that
    /// resolves to the same face as the replacement character '◻' (U+25FB) —
    /// in the default chain that face is the built-in NotoEmoji subset. So
    /// glyphs that subset covers (⬆⬇, 📬, 🚀, 🎵, 🔄…) render fine but
    /// report `false` here and are deliberately NOT listed. The list below
    /// only holds glyphs whose covering face is a different one: the
    /// embedded full Noto Emoji (the emoji this module exists to fix) and
    /// emoji-icon-font (hard-coded UI controls).
    #[test]
    fn extended_font_chain_covers_ui_glyphs_and_config_icons() {
        let mut fonts = egui::epaint::text::Fonts::new(
            egui::epaint::text::TextOptions::default(),
            font_definitions(),
        );
        let font_id = egui::FontId::proportional(16.0);

        // Hard-coded UI control glyphs (egui_app.rs): group expand toggle
        // and favorite stars — all covered by emoji-icon-font.
        let ui_glyphs = "⏵⏷★☆";
        // Config icons resolving outside the builtin NotoEmoji subset:
        // 🧪🤖🏗🧹 were tofu and only the embedded full Noto Emoji covers
        // them; 🖥⚙🛠 come from emoji-icon-font.
        let fixed_icons = "🧪🤖🏗🧹🖥⚙🛠";

        for glyph in ui_glyphs.chars().chain(fixed_icons.chars()) {
            assert!(
                fonts.has_glyph(&font_id, glyph),
                "glyph {glyph:?} (U+{:04X}) is not covered by the font chain",
                glyph as u32
            );
        }
    }

    #[test]
    fn invalid_or_unreadable_system_font_keeps_the_fallback_contract() {
        assert!(!valid_sfnt(b"not a font"));
        assert!(valid_sfnt(&[0, 1, 0, 0, 0]));
        let fonts = font_definitions();
        assert!(fonts.font_data.contains_key(FONT_NAME));
    }
}
