//! Three-layer design tokens (`Primitive -> Semantic -> Component`) for QuotaGlass v0.5.0.
//! Supports `Theme::System | Light | Dark` x `GlassMode::LiquidGlass | Clear | Frosted`
//! plus near-solid opaque fallback when Windows transparency (`os_glass`) is disabled.

use crate::settings::{GlassMode, Theme};
use iced::border::{self, Border};
use iced::font;
use iced::widget::{button, container, rule, scrollable, text_input, toggler};
use iced::{Background, Color, Font, Shadow, Vector};
use quotaglass_core::Service;

/// Crisp Windows UI font used across every text role in QuotaGlass.
pub const UI_FONT: Font = Font::with_name("Segoe UI");

pub const fn ui_font(weight: font::Weight) -> Font {
    Font {
        weight,
        ..UI_FONT
    }
}

/// Layer 1: Primitive design constants (raw brand hues, semantic palette, radii, spacing, type).
pub mod primitives {
    use iced::Color;

    // Fixed provider brand accents.
    pub const CLAUDE_ACCENT: Color = Color::from_rgb(
        0xd9 as f32 / 255.0,
        0x77 as f32 / 255.0,
        0x57 as f32 / 255.0,
    );
    pub const CODEX_ACCENT: Color = Color::from_rgb(
        0x10 as f32 / 255.0,
        0xa3 as f32 / 255.0,
        0x7f as f32 / 255.0,
    );
    pub const ANTIGRAVITY_ACCENT: Color = Color::from_rgb(
        0x4c as f32 / 255.0,
        0x8d as f32 / 255.0,
        0xf6 as f32 / 255.0,
    );

    // Semantic status colors (WCAG AA across bright and dark translucent glass).
    pub const WARN_DARK: Color = Color::from_rgb(
        0xff as f32 / 255.0,
        0xbe as f32 / 255.0,
        0x42 as f32 / 255.0,
    );
    pub const WARN_LIGHT: Color = Color::from_rgb(
        0x7d as f32 / 255.0,
        0x42 as f32 / 255.0,
        0x00 as f32 / 255.0,
    );
    pub const DANGER_DARK: Color = Color::from_rgb(
        0xff as f32 / 255.0,
        0xa6 as f32 / 255.0,
        0xa6 as f32 / 255.0,
    );
    pub const DANGER_LIGHT: Color = Color::from_rgb(
        0xa6 as f32 / 255.0,
        0x19 as f32 / 255.0,
        0x21 as f32 / 255.0,
    );
    pub const BURN_BADGE_BG: Color = Color::from_rgb(
        0xc4 as f32 / 255.0,
        0x24 as f32 / 255.0,
        0x2a as f32 / 255.0,
    );

    // Neutrals.
    pub const INK_WHITE: Color = Color::WHITE;
    pub const INK_BLACK: Color = Color::from_rgb(
        0x11 as f32 / 255.0,
        0x11 as f32 / 255.0,
        0x11 as f32 / 255.0,
    );

    // Surface base RGBs.
    pub const DARK_LIQUID_RGB: (u8, u8, u8) = (18, 22, 30);
    pub const LIGHT_LIQUID_RGB: (u8, u8, u8) = (242, 246, 252);
    pub const DARK_CLEAR_RGB: (u8, u8, u8) = (12, 15, 20);
    pub const LIGHT_CLEAR_RGB: (u8, u8, u8) = (248, 250, 255);
    pub const DARK_FROSTED_RGB: (u8, u8, u8) = (20, 22, 28);
    pub const LIGHT_FROSTED_RGB: (u8, u8, u8) = (246, 246, 244);
    pub const DARK_SOLID_RGB: (u8, u8, u8) = (18, 20, 25);
    pub const LIGHT_SOLID_RGB: (u8, u8, u8) = (246, 246, 244);

    // Distinct glass alphas.
    pub const ALPHA_CLEAR_DARK: f32 = 0.05;
    pub const ALPHA_CLEAR_LIGHT: f32 = 0.05;
    pub const ALPHA_LIQUID_DARK: f32 = 0.36;
    pub const ALPHA_LIQUID_LIGHT: f32 = 0.40;
    pub const ALPHA_FROSTED_DARK: f32 = 0.80;
    pub const ALPHA_FROSTED_LIGHT: f32 = 0.78;
    pub const ALPHA_SOLID_DARK: f32 = 0.96;
    pub const ALPHA_SOLID_LIGHT: f32 = 0.97;

    // Radii & spacing. Both PILL_RADIUS and POPOVER_RADIUS match DWM DWMWCP_ROUND (8 logical px).
    pub const PILL_RADIUS: f32 = 8.0;
    pub const POPOVER_RADIUS: f32 = 8.0;
    pub const CARD_RADIUS: f32 = 10.0;
    pub const CONTROL_RADIUS: f32 = 7.0;
    pub const CHIP_RADIUS: f32 = 999.0;

    pub const SPACE_XS: f32 = 4.0;
    pub const SPACE_SM: f32 = 8.0;
    pub const SPACE_MD: f32 = 12.0;
    pub const SPACE_LG: f32 = 16.0;

    // Type scale (12px hard floor).
    pub const TEXT_XS: f32 = 12.0;
    pub const TEXT_SM: f32 = 13.0;
    pub const TEXT_BASE: f32 = 14.0;
    pub const TEXT_MD: f32 = 15.0;
    pub const TEXT_LG: f32 = 18.0;

    // Fitts's Law minimum interactive target height.
    pub const TARGET_MIN_H: f32 = 38.0;
    pub const POPOVER_WIDTH: f32 = 412.0;
    pub const ACCOUNT_LIST_MAX_H: f32 = 460.0;
}

use primitives as p;

pub const fn rgba8(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color::from_rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a)
}

pub fn resolve_theme_dark(theme: Theme, system_dark: bool) -> bool {
    match theme {
        Theme::System => system_dark,
        Theme::Light => false,
        Theme::Dark => true,
    }
}

/// Layer 2: Semantic tokens resolved for active brightness, glass mode, and OS transparency state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SemanticTokens {
    pub is_dark: bool,
    pub glass_mode: GlassMode,
    pub os_glass: bool,
    pub surface_base: Color,
    pub surface_card: Color,
    pub surface_card_hover: Color,
    pub surface_card_expanded: Color,
    pub surface_control: Color,
    pub surface_control_hover: Color,
    pub surface_control_pressed: Color,
    pub surface_tab_active: Color,
    pub rim_specular: Color,
    pub border_subtle: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_disabled: Color,
    pub best_bg: Color,
    pub best_ink: Color,
    pub primary_bg: Color,
    pub primary_ink: Color,
    pub track_bg: Color,
    pub tick_line: Color,
    pub pace_marker: Color,
    pub warn: Color,
    pub warn_soft_bg: Color,
    pub danger: Color,
    pub danger_soft_bg: Color,
    pub danger_hover_bg: Color,
    pub burn_bg: Color,
    pub burn_ink: Color,
    pub shimmer_base: Color,
    pub shimmer_highlight: Color,
    pub grip_dot: Color,
    pub shadow: Shadow,
}

impl SemanticTokens {
    pub fn resolve(theme: Theme, glass_mode: GlassMode, system_dark: bool, os_glass: bool) -> Self {
        let is_dark = resolve_theme_dark(theme, system_dark);
        let (surface_base, rim_specular) = if !os_glass {
            if is_dark {
                (
                    rgba8(
                        p::DARK_SOLID_RGB.0,
                        p::DARK_SOLID_RGB.1,
                        p::DARK_SOLID_RGB.2,
                        p::ALPHA_SOLID_DARK,
                    ),
                    rgba8(255, 255, 255, 0.14),
                )
            } else {
                (
                    rgba8(
                        p::LIGHT_SOLID_RGB.0,
                        p::LIGHT_SOLID_RGB.1,
                        p::LIGHT_SOLID_RGB.2,
                        p::ALPHA_SOLID_LIGHT,
                    ),
                    rgba8(0, 0, 0, 0.10),
                )
            }
        } else {
            match (is_dark, glass_mode) {
                (true, GlassMode::LiquidGlass) => (
                    rgba8(
                        p::DARK_LIQUID_RGB.0,
                        p::DARK_LIQUID_RGB.1,
                        p::DARK_LIQUID_RGB.2,
                        p::ALPHA_LIQUID_DARK,
                    ),
                    rgba8(255, 255, 255, 0.28),
                ),
                (false, GlassMode::LiquidGlass) => (
                    rgba8(
                        p::LIGHT_LIQUID_RGB.0,
                        p::LIGHT_LIQUID_RGB.1,
                        p::LIGHT_LIQUID_RGB.2,
                        p::ALPHA_LIQUID_LIGHT,
                    ),
                    rgba8(255, 255, 255, 0.80),
                ),
                (true, GlassMode::Clear) => (
                    rgba8(
                        p::DARK_CLEAR_RGB.0,
                        p::DARK_CLEAR_RGB.1,
                        p::DARK_CLEAR_RGB.2,
                        p::ALPHA_CLEAR_DARK,
                    ),
                    rgba8(255, 255, 255, 0.42),
                ),
                (false, GlassMode::Clear) => (
                    rgba8(
                        p::LIGHT_CLEAR_RGB.0,
                        p::LIGHT_CLEAR_RGB.1,
                        p::LIGHT_CLEAR_RGB.2,
                        p::ALPHA_CLEAR_LIGHT,
                    ),
                    rgba8(255, 255, 255, 0.88),
                ),
                (true, GlassMode::Frosted) => (
                    rgba8(
                        p::DARK_FROSTED_RGB.0,
                        p::DARK_FROSTED_RGB.1,
                        p::DARK_FROSTED_RGB.2,
                        p::ALPHA_FROSTED_DARK,
                    ),
                    rgba8(255, 255, 255, 0.22),
                ),
                (false, GlassMode::Frosted) => (
                    rgba8(
                        p::LIGHT_FROSTED_RGB.0,
                        p::LIGHT_FROSTED_RGB.1,
                        p::LIGHT_FROSTED_RGB.2,
                        p::ALPHA_FROSTED_LIGHT,
                    ),
                    rgba8(0, 0, 0, 0.14),
                ),
            }
        };

        if is_dark {
            let (card, card_hover, card_expanded) = if !os_glass || glass_mode == GlassMode::Frosted {
                (
                    rgba8(255, 255, 255, 0.05),
                    rgba8(255, 255, 255, 0.085),
                    rgba8(255, 255, 255, 0.10),
                )
            } else if glass_mode == GlassMode::Clear {
                (
                    rgba8(12, 14, 20, 0.75),
                    rgba8(18, 21, 28, 0.81),
                    rgba8(22, 26, 35, 0.85),
                )
            } else {
                (
                    rgba8(14, 18, 26, 0.62),
                    rgba8(20, 25, 35, 0.69),
                    rgba8(24, 30, 42, 0.74),
                )
            };
            Self {
                is_dark,
                glass_mode,
                os_glass,
                surface_base,
                surface_card: card,
                surface_card_hover: card_hover,
                surface_card_expanded: card_expanded,
                surface_control: rgba8(255, 255, 255, 0.11),
                surface_control_hover: rgba8(255, 255, 255, 0.18),
                surface_control_pressed: rgba8(255, 255, 255, 0.24),
                surface_tab_active: rgba8(255, 255, 255, 0.22),
                rim_specular,
                border_subtle: rgba8(255, 255, 255, 0.16),
                text_primary: p::INK_WHITE,
                text_secondary: rgba8(232, 236, 244, 1.0),
                text_muted: rgba8(206, 213, 226, 1.0),
                text_disabled: rgba8(152, 160, 174, 0.80),
                best_bg: p::INK_WHITE,
                best_ink: p::INK_BLACK,
                primary_bg: p::INK_WHITE,
                primary_ink: p::INK_BLACK,
                track_bg: rgba8(255, 255, 255, 0.18),
                tick_line: rgba8(16, 18, 24, 0.65),
                pace_marker: p::INK_WHITE,
                warn: p::WARN_DARK,
                warn_soft_bg: rgba8(255, 190, 66, 0.22),
                danger: p::DANGER_DARK,
                danger_soft_bg: rgba8(118, 22, 28, 0.86),
                danger_hover_bg: rgba8(156, 28, 36, 0.94),
                burn_bg: p::BURN_BADGE_BG,
                burn_ink: p::INK_WHITE,
                shimmer_base: rgba8(255, 255, 255, 0.10),
                shimmer_highlight: rgba8(255, 255, 255, 0.22),
                grip_dot: rgba8(255, 255, 255, 0.82),
                shadow: Shadow {
                    color: rgba8(0, 0, 0, 0.26),
                    offset: Vector::new(0.0, 2.0),
                    blur_radius: 10.0,
                },
            }
        } else {
            let (card, card_hover, card_expanded) = if !os_glass || glass_mode == GlassMode::Frosted {
                (
                    rgba8(255, 255, 255, 0.80),
                    rgba8(255, 255, 255, 0.88),
                    rgba8(255, 255, 255, 0.94),
                )
            } else if glass_mode == GlassMode::Clear {
                (
                    rgba8(255, 255, 255, 0.80),
                    rgba8(255, 255, 255, 0.87),
                    rgba8(255, 255, 255, 0.91),
                )
            } else {
                (
                    rgba8(255, 255, 255, 0.68),
                    rgba8(255, 255, 255, 0.78),
                    rgba8(255, 255, 255, 0.84),
                )
            };
            Self {
                is_dark,
                glass_mode,
                os_glass,
                surface_base,
                surface_card: card,
                surface_card_hover: card_hover,
                surface_card_expanded: card_expanded,
                surface_control: rgba8(0, 0, 0, 0.08),
                surface_control_hover: rgba8(0, 0, 0, 0.14),
                surface_control_pressed: rgba8(0, 0, 0, 0.20),
                surface_tab_active: rgba8(0, 0, 0, 0.15),
                rim_specular,
                border_subtle: rgba8(0, 0, 0, 0.18),
                text_primary: p::INK_BLACK,
                text_secondary: rgba8(32, 36, 44, 1.0),
                text_muted: rgba8(62, 68, 78, 1.0),
                text_disabled: rgba8(102, 108, 118, 0.82),
                best_bg: p::INK_BLACK,
                best_ink: p::INK_WHITE,
                primary_bg: p::INK_BLACK,
                primary_ink: p::INK_WHITE,
                track_bg: rgba8(0, 0, 0, 0.15),
                tick_line: rgba8(246, 246, 244, 0.82),
                pace_marker: p::INK_BLACK,
                warn: p::WARN_LIGHT,
                warn_soft_bg: rgba8(125, 66, 0, 0.16),
                danger: p::DANGER_LIGHT,
                danger_soft_bg: rgba8(252, 232, 234, 0.90),
                danger_hover_bg: rgba8(245, 210, 214, 0.96),
                burn_bg: p::BURN_BADGE_BG,
                burn_ink: p::INK_WHITE,
                shimmer_base: rgba8(0, 0, 0, 0.07),
                shimmer_highlight: rgba8(0, 0, 0, 0.15),
                grip_dot: rgba8(17, 17, 17, 0.78),
                shadow: Shadow {
                    color: rgba8(0, 0, 0, 0.12),
                    offset: Vector::new(0.0, 2.0),
                    blur_radius: 10.0,
                },
            }
        }
    }
}

/// Layer 3: Component tokens and Iced widget style builders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DesignTokens {
    pub semantic: SemanticTokens,
}

impl DesignTokens {
    pub fn resolve(theme: Theme, glass_mode: GlassMode, system_dark: bool, os_glass: bool) -> Self {
        Self {
            semantic: SemanticTokens::resolve(theme, glass_mode, system_dark, os_glass),
        }
    }

    pub fn is_dark(&self) -> bool {
        self.semantic.is_dark
    }

    pub fn service_accent(&self, service: Service) -> Color {
        match service {
            Service::Claude => p::CLAUDE_ACCENT,
            Service::Codex => p::CODEX_ACCENT,
            Service::Antigravity => p::ANTIGRAVITY_ACCENT,
        }
    }

    pub fn pill_container_style(&self) -> container::Style {
        container::Style {
            text_color: Some(self.semantic.text_primary),
            background: Some(Background::Color(self.semantic.surface_base)),
            border: Border {
                color: self.semantic.rim_specular,
                width: 1.0,
                radius: border::radius(p::PILL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn popover_container_style(&self) -> container::Style {
        container::Style {
            text_color: Some(self.semantic.text_primary),
            background: Some(Background::Color(self.semantic.surface_base)),
            border: Border {
                color: self.semantic.rim_specular,
                width: 1.0,
                radius: border::radius(p::POPOVER_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn card_container_style(&self, expanded: bool, hovered: bool) -> container::Style {
        let bg = if expanded {
            self.semantic.surface_card_expanded
        } else if hovered {
            self.semantic.surface_card_hover
        } else {
            self.semantic.surface_card
        };
        let border_color = if expanded {
            self.semantic.rim_specular
        } else {
            self.semantic.border_subtle
        };
        container::Style {
            text_color: Some(self.semantic.text_primary),
            background: Some(Background::Color(bg)),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: border::radius(p::CARD_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn subtle_panel_style(&self) -> container::Style {
        container::Style {
            text_color: Some(self.semantic.text_primary),
            background: Some(Background::Color(self.semantic.surface_card)),
            border: Border {
                color: self.semantic.border_subtle,
                width: 1.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn danger_panel_style(&self) -> container::Style {
        container::Style {
            text_color: Some(self.semantic.text_primary),
            background: Some(Background::Color(self.semantic.danger_soft_bg)),
            border: Border {
                color: Color {
                    a: 0.42,
                    ..self.semantic.danger
                },
                width: 1.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn chip_style(&self, bg: Color, text: Color, border_color: Color) -> container::Style {
        container::Style {
            text_color: Some(text),
            background: Some(Background::Color(bg)),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: border::radius(p::CHIP_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn best_chip_style(&self) -> container::Style {
        self.chip_style(
            self.semantic.best_bg,
            self.semantic.best_ink,
            Color::TRANSPARENT,
        )
    }

    pub fn plan_chip_style(&self) -> container::Style {
        self.chip_style(
            self.semantic.surface_control,
            self.semantic.text_secondary,
            self.semantic.border_subtle,
        )
    }

    pub fn burn_chip_style(&self) -> container::Style {
        let border_color = if self.is_dark() {
            rgba8(255, 255, 255, 0.24)
        } else {
            rgba8(130, 16, 22, 0.50)
        };
        self.chip_style(self.semantic.burn_bg, self.semantic.burn_ink, border_color)
    }

    pub fn segmented_button_style(&self, active: bool, status: button::Status) -> button::Style {
        if active {
            return self.primary_button_style(status);
        }
        self.quiet_button_style(false, status)
    }

    pub fn tab_button_style(&self, active: bool, status: button::Status) -> button::Style {
        let bg = match (active, status) {
            (true, _) => self.semantic.surface_tab_active,
            (false, button::Status::Hovered) => self.semantic.surface_control_hover,
            (false, button::Status::Pressed) => self.semantic.surface_control_pressed,
            (false, _) => Color::TRANSPARENT,
        };
        let border_color = if active {
            if self.is_dark() {
                self.semantic.rim_specular
            } else {
                rgba8(0, 0, 0, 0.28)
            }
        } else {
            Color::TRANSPARENT
        };
        let text_color = if active {
            self.semantic.text_primary
        } else {
            self.semantic.text_secondary
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: border_color,
                width: if active { 1.0 } else { 0.0 },
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn primary_button_style(&self, status: button::Status) -> button::Style {
        let (bg, text_color) = match status {
            button::Status::Active => (self.semantic.primary_bg, self.semantic.primary_ink),
            button::Status::Hovered => (
                Color {
                    a: 0.90,
                    ..self.semantic.primary_bg
                },
                self.semantic.primary_ink,
            ),
            button::Status::Pressed => (
                Color {
                    a: 0.80,
                    ..self.semantic.primary_bg
                },
                self.semantic.primary_ink,
            ),
            button::Status::Disabled => (self.semantic.surface_control, self.semantic.text_disabled),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn quiet_button_style(&self, active: bool, status: button::Status) -> button::Style {
        let (bg, text_color) = match status {
            button::Status::Disabled => (
                Color {
                    a: 0.04,
                    ..self.semantic.surface_control
                },
                self.semantic.text_disabled,
            ),
            button::Status::Pressed => (self.semantic.surface_control_pressed, self.semantic.text_primary),
            button::Status::Hovered => (self.semantic.surface_control_hover, self.semantic.text_primary),
            button::Status::Active => (
                if active {
                    self.semantic.surface_tab_active
                } else {
                    self.semantic.surface_control
                },
                if active {
                    self.semantic.text_primary
                } else {
                    self.semantic.text_secondary
                },
            ),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: if active {
                    self.semantic.rim_specular
                } else {
                    self.semantic.border_subtle
                },
                width: 1.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn ghost_button_style(&self, status: button::Status) -> button::Style {
        let (bg, text_color) = match status {
            button::Status::Disabled => (Color::TRANSPARENT, self.semantic.text_disabled),
            button::Status::Pressed => (self.semantic.surface_control_pressed, self.semantic.text_primary),
            button::Status::Hovered => (self.semantic.surface_control_hover, self.semantic.text_primary),
            button::Status::Active => (Color::TRANSPARENT, self.semantic.text_secondary),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn danger_button_style(&self, status: button::Status) -> button::Style {
        let (bg, text_color) = match status {
            button::Status::Disabled => (self.semantic.surface_control, self.semantic.text_disabled),
            button::Status::Hovered | button::Status::Pressed => {
                (self.semantic.danger_hover_bg, self.semantic.danger)
            }
            button::Status::Active => (self.semantic.danger_soft_bg, self.semantic.danger),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color {
                    a: 0.45,
                    ..self.semantic.danger
                },
                width: 1.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }

    pub fn text_input_style(&self, status: text_input::Status) -> text_input::Style {
        let border_color = match status {
            text_input::Status::Focused { .. } => self.semantic.rim_specular,
            text_input::Status::Hovered => self.semantic.text_muted,
            text_input::Status::Active | text_input::Status::Disabled => self.semantic.border_subtle,
        };
        text_input::Style {
            background: Background::Color(self.semantic.surface_control),
            border: Border {
                color: border_color,
                width: 1.0,
                radius: border::radius(p::CONTROL_RADIUS),
            },
            icon: self.semantic.text_muted,
            placeholder: self.semantic.text_muted,
            value: if matches!(status, text_input::Status::Disabled) {
                self.semantic.text_disabled
            } else {
                self.semantic.text_primary
            },
            selection: self.semantic.surface_tab_active,
        }
    }

    pub fn toggler_style(&self, status: toggler::Status) -> toggler::Style {
        let (bg, fg, border_color) = match status {
            toggler::Status::Active { is_toggled } => {
                if is_toggled {
                    (
                        self.semantic.primary_bg,
                        self.semantic.primary_ink,
                        self.semantic.primary_bg,
                    )
                } else {
                    (
                        self.semantic.surface_control,
                        self.semantic.text_secondary,
                        self.semantic.border_subtle,
                    )
                }
            }
            toggler::Status::Hovered { is_toggled } => {
                if is_toggled {
                    (
                        Color {
                            a: 0.88,
                            ..self.semantic.primary_bg
                        },
                        self.semantic.primary_ink,
                        self.semantic.primary_bg,
                    )
                } else {
                    (
                        self.semantic.surface_control_hover,
                        self.semantic.text_primary,
                        self.semantic.rim_specular,
                    )
                }
            }
            toggler::Status::Disabled { is_toggled } => (
                Color {
                    a: if is_toggled { 0.35 } else { 0.15 },
                    ..self.semantic.surface_control
                },
                self.semantic.text_disabled,
                self.semantic.border_subtle,
            ),
        };
        toggler::Style {
            background: Background::Color(bg),
            background_border_width: 1.0,
            background_border_color: border_color,
            foreground: Background::Color(fg),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(self.semantic.text_primary),
            border_radius: None,
            padding_ratio: 0.14,
        }
    }

    pub fn scrollable_style(&self, status: scrollable::Status) -> scrollable::Style {
        let rail = scrollable::Rail {
            background: None,
            border: border::rounded(3),
            scroller: scrollable::Scroller {
                background: Background::Color(self.semantic.surface_control_hover),
                border: border::rounded(3),
            },
        };
        let hovered_rail = scrollable::Rail {
            scroller: scrollable::Scroller {
                background: Background::Color(self.semantic.surface_control_pressed),
                ..rail.scroller
            },
            ..rail
        };
        let vertical_rail = match status {
            scrollable::Status::Hovered {
                is_vertical_scrollbar_hovered: true,
                ..
            }
            | scrollable::Status::Dragged {
                is_vertical_scrollbar_dragged: true,
                ..
            } => hovered_rail,
            _ => rail,
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(self.semantic.surface_base),
                border: Border {
                    color: self.semantic.border_subtle,
                    width: 1.0,
                    radius: border::radius(999.0),
                },
                shadow: self.semantic.shadow,
                icon: self.semantic.text_primary,
            },
        }
    }

    pub fn rule_style(&self) -> rule::Style {
        rule::Style {
            color: self.semantic.border_subtle,
            radius: border::radius(0.0),
            fill_mode: rule::FillMode::Full,
            snap: true,
        }
    }
}

/// Alpha-composites `fg` over `bg` in sRGB space (for contrast checking).
pub fn composite_over(fg: Color, bg: Color) -> Color {
    let a = fg.a;
    Color::from_rgb(
        fg.r * a + bg.r * (1.0 - a),
        fg.g * a + bg.g * (1.0 - a),
        fg.b * a + bg.b * (1.0 - a),
    )
}

/// WCAG 2.1 relative luminance for an opaque or composited sRGB `Color`.
pub fn relative_luminance(c: Color) -> f32 {
    fn channel(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// WCAG 2.1 contrast ratio (`1.0..=21.0`) between `fg` and `bg`.
pub fn contrast_ratio(fg: Color, bg: Color) -> f32 {
    let fg_eff = if fg.a < 1.0 {
        composite_over(fg, bg)
    } else {
        fg
    };
    let l1 = relative_luminance(fg_eff);
    let l2 = relative_luminance(bg);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.005
    }

    #[test]
    fn brand_accents_match_quotaglass_identity() {
        let t = DesignTokens::resolve(Theme::Dark, GlassMode::LiquidGlass, true, true);
        assert_eq!(t.service_accent(Service::Claude).into_rgba8(), [0xd9, 0x77, 0x57, 0xff]);
        assert_eq!(t.service_accent(Service::Codex).into_rgba8(), [0x10, 0xa3, 0x7f, 0xff]);
        assert_eq!(
            t.service_accent(Service::Antigravity).into_rgba8(),
            [0x4c, 0x8d, 0xf6, 0xff]
        );
    }

    #[test]
    fn three_glass_modes_and_solid_fallback_have_distinct_alpha_and_rims() {
        for (theme, expected_dark) in [(Theme::Dark, true), (Theme::Light, false)] {
            let clear = DesignTokens::resolve(theme, GlassMode::Clear, expected_dark, true);
            let liquid = DesignTokens::resolve(theme, GlassMode::LiquidGlass, expected_dark, true);
            let frosted = DesignTokens::resolve(theme, GlassMode::Frosted, expected_dark, true);
            let solid = DesignTokens::resolve(theme, GlassMode::LiquidGlass, expected_dark, false);

            let a_clear = clear.semantic.surface_base.a;
            let a_liquid = liquid.semantic.surface_base.a;
            let a_frosted = frosted.semantic.surface_base.a;
            let a_solid = solid.semantic.surface_base.a;

            assert!(
                a_clear < a_liquid && a_liquid < a_frosted && a_frosted < a_solid,
                "expected Clear ({a_clear}) < LiquidGlass ({a_liquid}) < Frosted ({a_frosted}) < Solid ({a_solid})"
            );

            if expected_dark {
                assert!(approx(a_clear, 0.05));
                assert!(approx(a_liquid, 0.36));
                assert!(approx(a_frosted, 0.80));
                assert!(approx(a_solid, 0.96));
                assert!(approx(liquid.semantic.rim_specular.a, 0.28));
            } else {
                assert!(approx(a_clear, 0.05));
                assert!(approx(a_liquid, 0.40));
                assert!(approx(a_frosted, 0.78));
                assert!(approx(a_solid, 0.97));
                assert!(approx(liquid.semantic.rim_specular.a, 0.80));
            }

            // Solid fallback overrides all 3 glass modes equally.
            for mode in [GlassMode::Clear, GlassMode::LiquidGlass, GlassMode::Frosted] {
                let s = DesignTokens::resolve(theme, mode, expected_dark, false);
                assert!(s.semantic.surface_base.a >= 0.95);
            }
        }
    }

    #[test]
    fn system_theme_resolves_light_and_dark() {
        let dark = DesignTokens::resolve(Theme::System, GlassMode::LiquidGlass, true, true);
        let light = DesignTokens::resolve(Theme::System, GlassMode::LiquidGlass, false, true);
        assert!(dark.is_dark());
        assert!(!light.is_dark());
        assert_ne!(dark.semantic.text_primary, light.semantic.text_primary);
    }

    #[test]
    fn type_scale_enforces_12px_floor_and_matching_dwm_radii() {
        const {
            assert!(p::TEXT_XS >= 12.0);
            assert!(p::TEXT_SM > p::TEXT_XS);
            assert!(p::TEXT_BASE > p::TEXT_SM);
            assert!(p::TEXT_MD > p::TEXT_BASE);
            assert!(p::TEXT_LG > p::TEXT_MD);
        }
        assert_eq!(p::PILL_RADIUS, 8.0);
        assert_eq!(p::POPOVER_RADIUS, 8.0);
    }

    #[test]
    fn contrast_meets_readable_wcag_aa_floors() {
        let dark_desktop = Color::from_rgb8(0x12, 0x14, 0x1a);
        let light_desktop = Color::from_rgb8(0xf0, 0xec, 0xe2);

        for mode in [GlassMode::Clear, GlassMode::LiquidGlass, GlassMode::Frosted] {
            for os_glass in [true, false] {
                for wallpaper in [dark_desktop, light_desktop] {
                    let dark = DesignTokens::resolve(Theme::Dark, mode, true, os_glass);
                    let dark_bg = composite_over(
                        dark.semantic.surface_card,
                        composite_over(dark.semantic.surface_base, wallpaper),
                    );
                    assert!(
                        contrast_ratio(dark.semantic.text_primary, dark_bg) >= 7.0,
                        "dark primary text contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(dark.semantic.text_secondary, dark_bg) >= 4.5,
                        "dark secondary text contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(dark.semantic.text_muted, dark_bg) >= 4.5,
                        "dark muted text contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(dark.semantic.best_ink, dark.semantic.best_bg) >= 12.0
                    );
                    assert!(
                        contrast_ratio(dark.semantic.burn_ink, dark.semantic.burn_bg) >= 4.5
                    );
                    assert!(
                        contrast_ratio(dark.semantic.warn, dark_bg) >= 4.5,
                        "dark warn contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(dark.semantic.danger, dark_bg) >= 4.5,
                        "dark danger contrast too low for {mode:?} os_glass={os_glass}"
                    );

                    let light = DesignTokens::resolve(Theme::Light, mode, false, os_glass);
                    let light_bg = composite_over(
                        light.semantic.surface_card,
                        composite_over(light.semantic.surface_base, wallpaper),
                    );
                    assert!(
                        contrast_ratio(light.semantic.text_primary, light_bg) >= 7.0,
                        "light primary text contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(light.semantic.text_secondary, light_bg) >= 4.5,
                        "light secondary text contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(light.semantic.text_muted, light_bg) >= 4.5,
                        "light muted text contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(light.semantic.best_ink, light.semantic.best_bg) >= 12.0
                    );
                    assert!(
                        contrast_ratio(light.semantic.burn_ink, light.semantic.burn_bg) >= 4.5
                    );
                    assert!(
                        contrast_ratio(light.semantic.warn, light_bg) >= 4.5,
                        "light warn contrast too low for {mode:?} os_glass={os_glass}"
                    );
                    assert!(
                        contrast_ratio(light.semantic.danger, light_bg) >= 4.5,
                        "light danger contrast too low for {mode:?} os_glass={os_glass}"
                    );
                }
            }
        }
    }
}
