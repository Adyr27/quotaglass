//! Built-in plain-English FAQ view inside the compact popover.

use crate::ui::icons;
use crate::ui::tokens::{primitives as p, DesignTokens};
use crate::ui::UiMessage;
use iced::font;
use iced::widget::{button, column, container, row, rule, scrollable, space, text};
use iced::{Alignment, Element, Font, Length};

struct FaqEntry {
    question: &'static str,
    answer: &'static str,
}

const FAQ_ENTRIES: [FaqEntry; 4] = [
    FaqEntry {
        question: "How do Antigravity & T3 Code live readings work?",
        answer: "When Antigravity IDE or T3 Code (agy.exe) is open, QuotaGlass talks only to its local loopback status port on this PC. If neither app is running, QuotaGlass keeps your last reading and shows a friendly notice until you open it again.",
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
                    text("Back").size(p::TEXT_SM),
                ]
                .spacing(4.0)
                .align_y(Alignment::Center),
            )
            .center_y(Length::Fill),
        )
        .padding([0.0, 10.0])
        .height(Length::Fixed(p::TARGET_MIN_H))
        .style(move |_, status| t.quiet_button_style(false, status))
        .on_press(UiMessage::BackToList),
        text("FAQ")
            .size(p::TEXT_LG)
            .font(Font {
                weight: font::Weight::Semibold,
                ..Font::DEFAULT
            })
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
                        .font(Font {
                            weight: font::Weight::Semibold,
                            ..Font::DEFAULT
                        })
                        .color(t.semantic.text_primary),
                    text(item.answer)
                        .size(p::TEXT_SM)
                        .color(t.semantic.text_secondary),
                ]
                .spacing(4.0),
            )
            .padding(10.0)
            .width(Length::Fill)
            .style(move |_| t.subtle_panel_style()),
        );
    }

    column![
        header,
        rule::horizontal(1).style(move |_| t.rule_style()),
        scrollable(entries)
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(4.0)
            .style(move |_, status| t.scrollable_style(status)),
    ]
    .spacing(10.0)
    .height(Length::Fill)
    .into()
}
