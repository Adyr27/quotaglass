//! Built-in plain-English FAQ view inside the compact popover.

use crate::ui::icons;
use crate::ui::tokens::{primitives as p, ui_font, DesignTokens};
use crate::ui::UiMessage;
use iced::font;
use iced::widget::{button, column, container, row, rule, scrollable, space, text};
use iced::{Alignment, Element, Length, Padding};

struct FaqEntry {
    question: &'static str,
    answer: &'static str,
}

const FAQ_ENTRIES: [FaqEntry; 4] = [
    FaqEntry {
        question: "How do Antigravity live readings work?",
        answer: "When Antigravity is open on this PC, QuotaGlass talks only to its local loopback status port. If Antigravity is closed, QuotaGlass keeps your last reading and shows a friendly notice until you open it again.",
    },
    FaqEntry {
        question: "Why does Claude poll less often and stagger accounts?",
        answer: "Claude's usage endpoint applies strict rate limits. QuotaGlass spaces out multiple Claude accounts across the refresh window so they never burst together, and keeps your last good reading visible if the service pauses checks.",
    },
    FaqEntry {
        question: "How does multi-monitor taskbar pinning work?",
        answer: "Pinning locks the pill in place and saves a taskbar-relative anchor per display setup. When you switch between an external monitor and a laptop screen, QuotaGlass snaps the pill right back into the taskbar band instead of floating above it.",
    },
    FaqEntry {
        question: "What privacy and security guarantees does QuotaGlass keep?",
        answer: "Your credentials stay isolated in the core crate, are never saved to settings, and are never logged. QuotaGlass reads local CLI files read-only, never refreshes OAuth tokens, contacts only Anthropic, OpenAI, and localhost, and checks GitHub releases only if enabled.",
    },
];

pub fn view<'a>(tokens: &DesignTokens) -> Element<'a, UiMessage> {
    let t = *tokens;

    let header = row![
        button(
            container(
                row![
                    icons::tinted_svg(icons::BACK_SVG, 14.0, t.semantic.text_primary),
                    text("Back")
                        .size(p::TEXT_SM)
                        .font(ui_font(font::Weight::Medium)),
                ]
                .spacing(5.0)
                .align_y(Alignment::Center),
            )
            .center_y(Length::Fill),
        )
        .padding([0.0, 12.0])
        .height(Length::Fixed(p::TARGET_MIN_H))
        .style(move |_, status| t.quiet_button_style(false, status))
        .on_press(UiMessage::BackToList),
        text("FAQ")
            .size(p::TEXT_LG)
            .font(ui_font(font::Weight::Semibold))
            .color(t.semantic.text_primary),
        space::horizontal(),
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    let mut entries = column![].spacing(10.0);
    for item in FAQ_ENTRIES {
        entries = entries.push(
            container(
                column![
                    text(item.question)
                        .size(p::TEXT_BASE)
                        .line_height(1.3)
                        .font(ui_font(font::Weight::Semibold))
                        .color(t.semantic.text_primary),
                    text(item.answer)
                        .size(p::TEXT_SM)
                        .line_height(1.38)
                        .color(t.semantic.text_secondary),
                ]
                .spacing(5.0),
            )
            .padding(12.0)
            .width(Length::Fill)
            .style(move |_| t.subtle_panel_style()),
        );
    }

    let scroll_content = container(entries).padding(Padding::ZERO.right(10.0).bottom(10.0));

    column![
        header,
        rule::horizontal(1).style(move |_| t.rule_style()),
        scrollable(scroll_content)
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(4.0)
            .style(move |_, status| t.scrollable_style(status)),
    ]
    .spacing(10.0)
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faq_covers_antigravity_without_t3_code_or_exe_names() {
        let joined: String = FAQ_ENTRIES
            .iter()
            .flat_map(|e| [e.question, " ", e.answer, "\n"])
            .collect();
        assert!(joined.contains("Antigravity"));
        assert!(!joined.to_ascii_lowercase().contains("t3 code"));
        assert!(!joined.to_ascii_lowercase().contains("agy.exe"));
    }
}
