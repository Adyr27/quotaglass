//! Compact first-run onboarding view when no sources or added accounts are connected yet.

use crate::scheduler::Detected;
use crate::settings::{Connection, Settings};
use crate::ui::tokens::{primitives as p, DesignTokens};
use crate::ui::{connection_name, icons, UiMessage};
use iced::font;
use iced::widget::{button, column, container, row, rule, space, text, toggler};
use iced::{Alignment, Element, Font, Length};
use quotaglass_core::Service;

fn source_found(detected: Option<Detected>, conn: Connection) -> bool {
    let Some(d) = detected else {
        return true;
    };
    match conn {
        Connection::ClaudeCurrent => d.claude,
        Connection::CodexCurrent => d.codex,
        Connection::AntigravityApp => d.antigravity_app,
    }
}

fn source_service(conn: Connection) -> Service {
    match conn {
        Connection::ClaudeCurrent => Service::Claude,
        Connection::CodexCurrent => Service::Codex,
        Connection::AntigravityApp => Service::Antigravity,
    }
}

pub fn view<'a>(
    tokens: &DesignTokens,
    settings: &Settings,
    detected: Option<Detected>,
) -> Element<'a, UiMessage> {
    let t = *tokens;

    let header = column![
        text("Connect your accounts")
            .size(p::TEXT_LG)
            .font(Font {
                weight: font::Weight::Semibold,
                ..Font::DEFAULT
            })
            .color(t.semantic.text_primary),
        text("Nothing is read until you turn a source on.")
            .size(p::TEXT_SM)
            .color(t.semantic.text_secondary),
    ]
    .spacing(3.0);

    let mut sources_col = column![].spacing(8.0);
    for conn in [
        Connection::ClaudeCurrent,
        Connection::CodexCurrent,
        Connection::AntigravityApp,
    ] {
        let on = settings.connected.contains(&conn);
        let found = source_found(detected, conn);
        let svc = source_service(conn);
        let accent = t.service_accent(svc);

        let note = match conn {
            Connection::AntigravityApp => "Works while Antigravity IDE or T3 Code is running.",
            _ if !found && !on => "Not signed in on this PC.",
            _ => "Detected on this PC.",
        };

        let mut tog = toggler(on).style(move |_, status| t.toggler_style(status));
        if on || found {
            tog = tog.on_toggle(move |v| UiMessage::ToggleSource(conn, v));
        }

        sources_col = sources_col.push(
            container(
                row![
                    icons::provider_icon(svc, 16.0, accent),
                    column![
                        text(connection_name(conn))
                            .size(p::TEXT_BASE)
                            .font(Font {
                                weight: font::Weight::Medium,
                                ..Font::DEFAULT
                            })
                            .color(t.semantic.text_primary),
                        text(note).size(p::TEXT_XS).color(t.semantic.text_muted),
                    ]
                    .spacing(2.0)
                    .width(Length::Fill),
                    tog,
                ]
                .spacing(10.0)
                .align_y(Alignment::Center),
            )
            .padding([9.0, 10.0])
            .width(Length::Fill)
            .style(move |_| t.subtle_panel_style()),
        );
    }

    let updates_row = container(
        row![
            column![
                text("Check for updates")
                    .size(p::TEXT_BASE)
                    .color(t.semantic.text_primary),
                text("Once a day, from GitHub. Nothing else is sent.")
                    .size(p::TEXT_XS)
                    .color(t.semantic.text_muted),
            ]
            .spacing(2.0)
            .width(Length::Fill),
            toggler(settings.check_updates)
                .on_toggle(UiMessage::SetCheckUpdates)
                .style(move |_, status| t.toggler_style(status)),
        ]
        .spacing(10.0)
        .align_y(Alignment::Center),
    )
    .padding([8.0, 10.0])
    .width(Length::Fill)
    .style(move |_| t.subtle_panel_style());

    let any_connected = !settings.connected.is_empty() || !settings.added_accounts.is_empty();
    let footer = row![
        space::horizontal(),
        button(
            container(
                text(if any_connected { "Continue" } else { "Skip for now" })
                    .size(p::TEXT_SM)
                    .font(Font {
                        weight: font::Weight::Semibold,
                        ..Font::DEFAULT
                    }),
            )
            .center_y(Length::Fill),
        )
        .padding([0.0, 14.0])
        .height(Length::Fixed(p::TARGET_MIN_H))
        .style(move |_, status| if any_connected {
            t.primary_button_style(status)
        } else {
            t.quiet_button_style(false, status)
        })
        .on_press(UiMessage::FinishOnboarding),
    ];

    column![
        header,
        sources_col,
        updates_row,
        rule::horizontal(1).style(move |_| t.rule_style()),
        footer,
    ]
    .spacing(10.0)
    .into()
}
