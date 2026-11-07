//! Colors that depend on the light or dark theme (D52).
//!
//! egui follows the Windows app theme. The status colors were chosen for the
//! dark theme and are too pale on a light background, and egui's light text is
//! a mid gray. The light colors have at least 4.5:1 contrast to the panel
//! background (WCAG AA for normal text); the dark ones are unchanged.

use eframe::egui::{self, Color32, Theme, ThemePreference};

use crate::config::ThemeChoice;

/// Status colors for text, borders and icons in the current theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub amber: Color32,
    pub red: Color32,
    pub blue: Color32,
    pub green: Color32,
    /// The search match and keyboard focus (D53).
    pub teal: Color32,
}

const DARK: Palette = Palette {
    amber: Color32::from_rgb(235, 160, 0),
    red: Color32::from_rgb(215, 60, 50),
    blue: Color32::from_rgb(90, 170, 255),
    green: Color32::from_rgb(60, 190, 80),
    teal: Color32::from_rgb(40, 200, 190),
};

const LIGHT: Palette = Palette {
    amber: Color32::from_rgb(160, 90, 0),
    red: Color32::from_rgb(185, 30, 25),
    blue: Color32::from_rgb(0, 95, 200),
    green: Color32::from_rgb(20, 125, 50),
    teal: Color32::from_rgb(0, 115, 115),
};

impl Palette {
    pub fn of(ui: &egui::Ui) -> Self {
        Self::for_dark_mode(ui.visuals().dark_mode)
    }

    pub fn for_dark_mode(dark: bool) -> Self {
        if dark { DARK } else { LIGHT }
    }
}

/// The theme the user chose; `System` follows the Windows app theme.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::System => ThemePreference::System,
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::Dark => ThemePreference::Dark,
    });
}

/// Darker text and links for egui's light theme (the dark theme stays as
/// is), and the chosen theme.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    let mut visuals = egui::Visuals::light();
    let text = Color32::from_gray(30);
    visuals.widgets.noninteractive.fg_stroke.color = text;
    visuals.widgets.inactive.fg_stroke.color = text;
    // Default: the text color at 60 % alpha, about 2.5:1 on the panel.
    visuals.weak_text_color = Some(Color32::from_gray(95));
    visuals.hyperlink_color = LIGHT.blue;
    visuals.warn_fg_color = LIGHT.amber;
    visuals.error_fg_color = LIGHT.red;
    ctx.set_visuals_of(Theme::Light, visuals);
    apply(ctx, choice);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(c: Color32) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn light_palette_is_readable() {
        let panel = egui::Visuals::light().panel_fill;
        for color in [LIGHT.amber, LIGHT.red, LIGHT.blue, LIGHT.green, LIGHT.teal] {
            let ratio = contrast(color, panel);
            assert!(ratio >= 4.5, "{color:?} on {panel:?}: {ratio:.2}");
        }
    }

    #[test]
    fn light_weak_text_is_readable() {
        let panel = egui::Visuals::light().panel_fill;
        assert!(contrast(Color32::from_gray(95), panel) >= 4.5);
    }
}
