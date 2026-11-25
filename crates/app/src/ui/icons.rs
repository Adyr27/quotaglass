//! Embedded provider and UI vector icons (`svg::Handle::from_memory`), tinted via `svg::Style`.

use iced::widget::{svg, Svg};
use iced::{Color, ContentFit, Element, Length};
use quotaglass_core::Service;

/// Provider mark matching `src/assets/providers/claude.svg`.
pub const CLAUDE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.6" stroke-linecap="round"><path d="M12 2.5v19M2.5 12h19M5.3 5.3l13.4 13.4M18.7 5.3 5.3 18.7"/></svg>"##;

/// Provider mark matching `src/assets/providers/codex.svg`.
pub const CODEX_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="2.5" y="3.5" width="19" height="17" rx="4"/><path d="M7 9.5l3 2.5-3 2.5M12.5 15h4.5"/></svg>"##;

/// Provider mark matching `src/assets/providers/antigravity.svg`.
pub const ANTIGRAVITY_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 18 12 8.5l7.5 9.5M3.5 21.5h17"/><circle cx="12" cy="3.6" r="2" fill="#000" stroke="none"/></svg>"##;

/// QuotaGlass capsule ring mark for empty/first-run pill slot.
pub const QUOTAGLASS_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.5" stroke-linecap="round"><path d="M12 4.5a7.5 7.5 0 1 1-7.5 7.5"/></svg>"##;

pub const REFRESH_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 12a8 8 0 1 1-2.3-5.6"/><path d="M20 4v5h-5"/></svg>"##;

pub const PIN_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><path d="M15 4.5 19.5 9l-2.3 2.3-1.2-.3-3.7 3.7.7 3.3-1.4 1.4L8 15.8l-4 4-.5-.5 4-4-3.6-3.6 1.4-1.4 3.3.7 3.7-3.7-.3-1.2z"/></svg>"##;

pub const SLIDERS_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.1" stroke-linecap="round"><path d="M4 7h8M16 7h4M12 4.5v5M4 17h4M12 17h8M8 14.5v5"/></svg>"##;

pub const HELP_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M9.6 9.4a2.6 2.6 0 0 1 4.9.9c0 1.6-2.5 2.1-2.5 3.6"/><circle cx="12" cy="17.2" r="0.9" fill="#000" stroke="none"/></svg>"##;

pub const BACK_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5.5 8.5 12 15 18.5"/></svg>"##;

pub const CHEVRON_DOWN_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m6.5 9.5 5.5 5.5 5.5-5.5"/></svg>"##;

pub const CHEVRON_UP_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m6.5 14.5 5.5-5.5 5.5 5.5"/></svg>"##;

pub const EXTERNAL_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4.5h5.5V10M10 14 19.5 4.5M18 14v4.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H10"/></svg>"##;

pub fn provider_svg_bytes(service: Service) -> &'static [u8] {
    match service {
        Service::Claude => CLAUDE_SVG,
        Service::Codex => CODEX_SVG,
        Service::Antigravity => ANTIGRAVITY_SVG,
    }
}

pub fn provider_handle(service: Service) -> svg::Handle {
    svg::Handle::from_memory(provider_svg_bytes(service))
}

pub fn tinted_svg<'a, Message: 'a>(
    bytes: &'static [u8],
    size: f32,
    color: Color,
) -> Element<'a, Message> {
    Svg::new(svg::Handle::from_memory(bytes))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .content_fit(ContentFit::Contain)
        .style(move |_theme, _status| svg::Style { color: Some(color) })
        .into()
}

pub fn provider_icon<'a, Message: 'a>(
    service: Service,
    size: f32,
    color: Color,
) -> Element<'a, Message> {
    tinted_svg(provider_svg_bytes(service), size, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_svgs_match_asset_geometry() {
        let c = std::str::from_utf8(CLAUDE_SVG).unwrap();
        let x = std::str::from_utf8(CODEX_SVG).unwrap();
        let a = std::str::from_utf8(ANTIGRAVITY_SVG).unwrap();
        assert!(c.contains("M12 2.5v19"));
        assert!(x.contains("M7 9.5l3 2.5-3 2.5"));
        assert!(a.contains("M4.5 18 12 8.5l7.5 9.5"));

        for bytes in [
            CLAUDE_SVG,
            CODEX_SVG,
            ANTIGRAVITY_SVG,
            QUOTAGLASS_SVG,
            REFRESH_SVG,
            PIN_SVG,
            SLIDERS_SVG,
            HELP_SVG,
            BACK_SVG,
            CHEVRON_DOWN_SVG,
            CHEVRON_UP_SVG,
            EXTERNAL_SVG,
        ] {
            let s = std::str::from_utf8(bytes).unwrap();
            assert!(s.starts_with("<svg ") && s.ends_with("</svg>"));
            assert!(s.contains("viewBox=\"0 0 24 24\""));
        }
    }
}
