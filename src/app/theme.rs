//! Colors that depend on the light or dark theme (D52).
//!
//! egui follows the Windows app theme. The status colors were chosen for the
//! dark theme and are too pale on a light background, and egui's light text is
//! a mid gray. The light colors have at least 4.5:1 contrast to the panel
//! background (WCAG AA for normal text); the dark ones are unchanged.
//!
//! The search match and the keyboard focus use the Windows accent color
//! (D53), read once at start.

use std::sync::OnceLock;

use eframe::egui::{self, Color32, Theme, ThemePreference};

use crate::config::ThemeChoice;

/// Status colors for text, borders and icons in the current theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub amber: Color32,
    pub red: Color32,
    pub blue: Color32,
    pub green: Color32,
    /// The Windows accent color, in a shade that stands out on the panel:
    /// the search match and the keyboard focus (D53).
    pub accent: Color32,
}

const DARK: Palette = Palette {
    amber: Color32::from_rgb(235, 160, 0),
    red: Color32::from_rgb(215, 60, 50),
    blue: Color32::from_rgb(90, 170, 255),
    green: Color32::from_rgb(60, 190, 80),
    accent: DEFAULT_ACCENT,
};

const LIGHT: Palette = Palette {
    amber: Color32::from_rgb(160, 90, 0),
    red: Color32::from_rgb(185, 30, 25),
    blue: Color32::from_rgb(0, 95, 200),
    green: Color32::from_rgb(20, 125, 50),
    accent: DEFAULT_ACCENT,
};

/// The Windows default accent color (blue), if it cannot be read.
const DEFAULT_ACCENT: Color32 = Color32::from_rgb(0, 120, 212);

/// WCAG 2.1 minimum contrast for user interface components (1.4.11).
const MIN_UI_CONTRAST: f64 = 3.0;

impl Palette {
    pub fn of(ui: &egui::Ui) -> Self {
        Self::for_dark_mode(ui.visuals().dark_mode)
    }

    pub fn for_dark_mode(dark: bool) -> Self {
        let (on_light, on_dark) = *ACCENTS.get_or_init(|| {
            let shades = accent_shades();
            (
                pick_accent(&shades, false, egui::Visuals::light().panel_fill),
                pick_accent(&shades, true, egui::Visuals::dark().panel_fill),
            )
        });
        if dark {
            Palette {
                accent: on_dark,
                ..DARK
            }
        } else {
            Palette {
                accent: on_light,
                ..LIGHT
            }
        }
    }
}

/// The accent for the light and for the dark panel.
static ACCENTS: OnceLock<(Color32, Color32)> = OnceLock::new();

/// The accent shades from darkest to lightest: AccentDark3 … Accent (index
/// 3) … AccentLight3.
type Shades = [Color32; 7];

/// The accent itself if it stands out enough on `panel`, else the first
/// darker (light theme) or lighter (dark theme) shade that does.
fn pick_accent(shades: &Shades, dark: bool, panel: Color32) -> Color32 {
    let order: [usize; 4] = if dark { [3, 4, 5, 6] } else { [3, 2, 1, 0] };
    order
        .iter()
        .map(|&i| shades[i])
        .find(|&c| contrast(c, panel) >= MIN_UI_CONTRAST)
        .unwrap_or(shades[order[3]])
}

/// The accent palette of the user's Windows personalization
/// (`UISettings::GetColorValue`, Windows 10 1507+); the default blue if it
/// cannot be read.
#[cfg(windows)]
fn accent_shades() -> Shades {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let read = || -> windows::core::Result<Shades> {
        let settings = UISettings::new()?;
        let mut shades = [DEFAULT_ACCENT; 7];
        let types = [
            UIColorType::AccentDark3,
            UIColorType::AccentDark2,
            UIColorType::AccentDark1,
            UIColorType::Accent,
            UIColorType::AccentLight1,
            UIColorType::AccentLight2,
            UIColorType::AccentLight3,
        ];
        for (shade, kind) in shades.iter_mut().zip(types) {
            let c = settings.GetColorValue(kind)?;
            *shade = Color32::from_rgb(c.R, c.G, c.B);
        }
        Ok(shades)
    };
    read().unwrap_or_else(|err| {
        tracing::warn!("cannot read the Windows accent color, using the default: {err}");
        [DEFAULT_ACCENT; 7]
    })
}

#[cfg(not(windows))]
fn accent_shades() -> Shades {
    [DEFAULT_ACCENT; 7]
}

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

/// WCAG contrast ratio.
fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
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

    #[test]
    fn light_palette_is_readable() {
        let panel = egui::Visuals::light().panel_fill;
        for color in [LIGHT.amber, LIGHT.red, LIGHT.blue, LIGHT.green] {
            let ratio = contrast(color, panel);
            assert!(ratio >= 4.5, "{color:?} on {panel:?}: {ratio:.2}");
        }
    }

    #[test]
    fn light_weak_text_is_readable() {
        let panel = egui::Visuals::light().panel_fill;
        assert!(contrast(Color32::from_gray(95), panel) >= 4.5);
    }

    #[test]
    fn default_accent_is_used_as_is_in_both_themes() {
        let shades = [DEFAULT_ACCENT; 7];
        for dark in [false, true] {
            let panel = if dark {
                egui::Visuals::dark().panel_fill
            } else {
                egui::Visuals::light().panel_fill
            };
            assert_eq!(pick_accent(&shades, dark, panel), DEFAULT_ACCENT);
        }
    }

    #[test]
    fn pale_or_dark_accents_step_to_a_visible_shade() {
        // A yellow accent: too pale on the light panel, fine on the dark one.
        let shades = [
            Color32::from_rgb(90, 70, 0),
            Color32::from_rgb(130, 100, 0),
            Color32::from_rgb(170, 130, 0),
            Color32::from_rgb(255, 200, 0),
            Color32::from_rgb(255, 215, 80),
            Color32::from_rgb(255, 230, 140),
            Color32::from_rgb(255, 240, 190),
        ];
        let light = egui::Visuals::light().panel_fill;
        let dark = egui::Visuals::dark().panel_fill;
        let on_light = pick_accent(&shades, false, light);
        assert_eq!(on_light, shades[2]);
        assert!(contrast(on_light, light) >= MIN_UI_CONTRAST);
        assert_eq!(pick_accent(&shades, true, dark), shades[3]);
        // A dark accent on the dark panel steps to a lighter shade.
        let navy = [
            Color32::from_rgb(0, 0, 40),
            Color32::from_rgb(0, 0, 60),
            Color32::from_rgb(0, 10, 80),
            Color32::from_rgb(0, 20, 110),
            Color32::from_rgb(40, 70, 170),
            Color32::from_rgb(90, 130, 230),
            Color32::from_rgb(150, 180, 255),
        ];
        let on_dark = pick_accent(&navy, true, dark);
        assert!(contrast(on_dark, dark) >= MIN_UI_CONTRAST, "{on_dark:?}");
    }
}
