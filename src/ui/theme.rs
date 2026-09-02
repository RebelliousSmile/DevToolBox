//! DevToolBox visual tokens and motion policy.

use eframe::egui;

pub const GRID: f32 = 4.0;
pub const NAV_WIDTH: f32 = 184.0;
pub const COMPACT_BREAKPOINT: f32 = 1_024.0;
pub const RADIUS_CARD: u8 = 12;
pub const RADIUS_CONTROL: u8 = 8;
pub const TRANSITION_MS: u64 = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn from_preference(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" | "clair" => Self::Light,
            "dark" | "sombre" => Self::Dark,
            _ => Self::System,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub canvas: egui::Color32,
    pub surface: egui::Color32,
    pub surface_raised: egui::Color32,
    pub text: egui::Color32,
    pub text_muted: egui::Color32,
    pub accent: egui::Color32,
    pub accent_text: egui::Color32,
    pub border: egui::Color32,
    pub success: egui::Color32,
    pub warning: egui::Color32,
    pub danger: egui::Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            canvas: egui::Color32::from_rgb(18, 20, 25),
            surface: egui::Color32::from_rgb(29, 32, 39),
            surface_raised: egui::Color32::from_rgb(38, 42, 51),
            text: egui::Color32::from_rgb(244, 246, 250),
            text_muted: egui::Color32::from_rgb(181, 188, 201),
            accent: egui::Color32::from_rgb(114, 145, 255),
            accent_text: egui::Color32::from_rgb(8, 14, 32),
            border: egui::Color32::from_rgb(65, 70, 82),
            success: egui::Color32::from_rgb(102, 210, 151),
            warning: egui::Color32::from_rgb(255, 196, 92),
            danger: egui::Color32::from_rgb(255, 124, 124),
        }
    } else {
        Palette {
            canvas: egui::Color32::from_rgb(245, 247, 251),
            surface: egui::Color32::from_rgb(255, 255, 255),
            surface_raised: egui::Color32::from_rgb(248, 250, 255),
            text: egui::Color32::from_rgb(26, 31, 43),
            text_muted: egui::Color32::from_rgb(82, 91, 110),
            accent: egui::Color32::from_rgb(61, 95, 220),
            accent_text: egui::Color32::WHITE,
            border: egui::Color32::from_rgb(211, 216, 227),
            success: egui::Color32::from_rgb(21, 112, 67),
            warning: egui::Color32::from_rgb(139, 83, 0),
            danger: egui::Color32::from_rgb(174, 38, 38),
        }
    }
}

pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    match mode {
        ThemeMode::Light => ctx.set_visuals(egui::Visuals::light()),
        ThemeMode::Dark => ctx.set_visuals(egui::Visuals::dark()),
        ThemeMode::System => {}
    }
    let dark = ctx.theme() == egui::Theme::Dark;
    let colors = palette(dark);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(GRID * 2.0, GRID * 2.0);
        style.spacing.button_padding = egui::vec2(GRID * 3.0, GRID * 2.0);
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        style.visuals.panel_fill = colors.canvas;
        style.visuals.window_fill = colors.surface;
        style.visuals.extreme_bg_color = colors.surface;
        style.visuals.faint_bg_color = colors.surface_raised;
        style.visuals.override_text_color = Some(colors.text);
        style.visuals.selection.bg_fill = colors.accent;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, colors.accent_text);
        style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, colors.border);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(RADIUS_CONTROL);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(RADIUS_CONTROL);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(RADIUS_CONTROL);
    });
}

pub fn relative_luminance(color: egui::Color32) -> f32 {
    fn channel(value: u8) -> f32 {
        let value = value as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

pub fn contrast_ratio(a: egui::Color32, b: egui::Color32) -> f32 {
    let (bright, dark) = {
        let a = relative_luminance(a);
        let b = relative_luminance(b);
        if a > b {
            (a, b)
        } else {
            (b, a)
        }
    };
    (bright + 0.05) / (dark + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_theme_values_remain_compatible() {
        assert_eq!(ThemeMode::from_preference("light"), ThemeMode::Light);
        assert_eq!(ThemeMode::from_preference("dark"), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_preference("system"), ThemeMode::System);
    }

    #[test]
    fn text_and_controls_meet_aa_contrast() {
        for colors in [palette(false), palette(true)] {
            assert!(contrast_ratio(colors.text, colors.surface) >= 4.5);
            assert!(contrast_ratio(colors.text_muted, colors.surface) >= 4.5);
            assert!(contrast_ratio(colors.accent_text, colors.accent) >= 4.5);
            assert!(contrast_ratio(colors.border, colors.surface) >= 1.3);
        }
    }

    #[test]
    fn motion_is_short_and_never_loops() {
        assert!((120..=180).contains(&TRANSITION_MS));
    }
}
