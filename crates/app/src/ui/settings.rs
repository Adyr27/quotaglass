//! In-popover Settings view: Theme, Glass, Refresh interval, Behavior, Reset reminders,
//! Sources with detection notes, and Hidden accounts. Every control saves immediately.

use crate::scheduler::Detected;
use crate::settings::{Connection, GlassMode, Settings, Theme, ToastSound};
use crate::ui::tokens::{primitives as p, DesignTokens};
use crate::ui::{connection_name, icons, segmented_row, service_name, UiMessage};
use iced::font;
use iced::widget::{button, column, container, row, rule, scrollable, space, text, toggler};
use iced::{Alignment, Element, Font, Length};
use quotaglass_core::view::AccountView;
use quotaglass_core::Service;

fn section_heading<'a>(tokens: &DesignTokens, title: &'static str) -> Element<'a, UiMessage> {
    text(title)
        .size(p::TEXT_SM)
        .font(Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        })
        .color(tokens.semantic.text_secondary)
        .into()
}

fn setting_row<'a>(
    tokens: &DesignTokens,
    label: &'static str,
    note: Option<&'static str>,
    control: Element<'a, UiMessage>,
) -> Element<'a, UiMessage> {
    let mut left = column![text(label).size(p::TEXT_BASE).color(tokens.semantic.text_primary)].spacing(2.0);
    if let Some(note_text) = note {
        left = left.push(text(note_text).size(p::TEXT_XS).color(tokens.semantic.text_muted));
    }
    row![left.width(Length::Fill), control]
        .spacing(10.0)
        .align_y(Alignment::Center)
        .into()
}

fn source_detected(detected: Option<Detected>, conn: Connection) -> bool {
    let Some(d) = detected else {
        return true;
    };
    match conn {
        Connection::ClaudeCurrent => d.claude,
        Connection::CodexCurrent => d.codex,
        Connection::AntigravityApp => d.antigravity_app,
    }
}

pub fn view<'a>(
    tokens: &DesignTokens,
    settings: &Settings,
    detected: Option<Detected>,
    accounts: &[AccountView],
) -> Element<'a, UiMessage> {
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
        text("Settings")
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

    let appearance_group = container(
        column![
            setting_row(
                &t,
                "Theme",
                None,
                segmented_row(
                    t,
                    &[
                        (Theme::System, "System"),
                        (Theme::Light, "Light"),
                        (Theme::Dark, "Dark"),
                    ],
                    settings.theme,
                    UiMessage::SetTheme,
                ),
            ),
            setting_row(
                &t,
                "Glass",
                Some("Solid when Windows transparency is off."),
                segmented_row(
                    t,
                    &[
                        (GlassMode::LiquidGlass, "Liquid"),
                        (GlassMode::Clear, "Clear"),
                        (GlassMode::Frosted, "Frosted"),
                    ],
                    settings.effective_glass_mode(),
                    UiMessage::SetGlassMode,
                ),
            ),
            setting_row(
                &t,
                "Refresh every",
                None,
                segmented_row(
                    t,
                    &[(1u8, "1 min"), (2u8, "2 min"), (5u8, "5 min")],
                    settings.refresh_minutes,
                    UiMessage::SetRefreshMinutes,
                ),
            ),
            setting_row(
                &t,
                "Always on top",
                None,
                toggler(settings.always_on_top)
                    .on_toggle(UiMessage::SetAlwaysOnTop)
                    .style(move |_, status| t.toggler_style(status))
                    .into(),
            ),
            setting_row(
                &t,
                "Start with Windows",
                None,
                toggler(settings.autostart)
                    .on_toggle(UiMessage::SetAutostart)
                    .style(move |_, status| t.toggler_style(status))
                    .into(),
            ),
            setting_row(
                &t,
                "Check for updates",
                Some("Once a day, from GitHub. Nothing else is sent."),
                toggler(settings.check_updates)
                    .on_toggle(UiMessage::SetCheckUpdates)
                    .style(move |_, status| t.toggler_style(status))
                    .into(),
            ),
        ]
        .spacing(10.0),
    )
    .padding(10.0)
    .width(Length::Fill)
    .style(move |_| t.subtle_panel_style());

    let toasts = &settings.reset_toasts;
    let service_toggle = |svc: Service, enabled: bool| {
        let master = toasts.enabled;
        let mut tog = toggler(enabled).style(move |_, status| t.toggler_style(status));
        if master {
            tog = tog.on_toggle(move |v| UiMessage::SetResetToastService(svc, v));
        }
        setting_row(&t, service_name(svc), None, tog.into())
    };

    let reminders_group = container(
        column![
            setting_row(
                &t,
                "Reset reminders",
                Some("Notify when a limit window resets."),
                toggler(toasts.enabled)
                    .on_toggle(UiMessage::SetResetToastsEnabled)
                    .style(move |_, status| t.toggler_style(status))
                    .into(),
            ),
            service_toggle(Service::Claude, toasts.claude),
            service_toggle(Service::Codex, toasts.codex),
            service_toggle(Service::Antigravity, toasts.antigravity),
            setting_row(
                &t,
                "Sound",
                None,
                segmented_row(
                    t,
                    &[
                        (ToastSound::Default, "Default"),
                        (ToastSound::Silent, "Silent"),
                    ],
                    toasts.sound,
                    UiMessage::SetResetToastSound,
                ),
            ),
        ]
        .spacing(9.0),
    )
    .padding(10.0)
    .width(Length::Fill)
    .style(move |_| t.subtle_panel_style());

    let mut sources_col = column![].spacing(9.0);
    for conn in [
        Connection::ClaudeCurrent,
        Connection::CodexCurrent,
        Connection::AntigravityApp,
    ] {
        let on = settings.connected.contains(&conn);
        let found = source_detected(detected, conn);
        let note = match conn {
            Connection::AntigravityApp => {
                Some("Reads live limits when Antigravity IDE or T3 Code is running.")
            }
            _ if !found && !on => Some("Not signed in on this PC."),
            _ => None,
        };
        let mut tog = toggler(on).style(move |_, status| t.toggler_style(status));
        if on || found {
            tog = tog.on_toggle(move |v| UiMessage::ToggleSource(conn, v));
        }
        sources_col = sources_col.push(setting_row(&t, connection_name(conn), note, tog.into()));
    }
    let sources_group = container(sources_col)
        .padding(10.0)
        .width(Length::Fill)
        .style(move |_| t.subtle_panel_style());

    let mut hidden_col = column![].spacing(8.0);
    let hidden_accounts: Vec<&AccountView> = accounts.iter().filter(|a| a.hidden).collect();
    if hidden_accounts.is_empty() && settings.hidden_accounts.is_empty() {
        hidden_col = hidden_col.push(
            text("No hidden accounts.")
                .size(p::TEXT_SM)
                .color(t.semantic.text_muted),
        );
    } else {
        for acct in &hidden_accounts {
            let id = acct.id.clone();
            hidden_col = hidden_col.push(
                row![
                    text(format!("{} · {}", service_name(acct.service), acct.label))
                        .size(p::TEXT_SM)
                        .color(t.semantic.text_primary)
                        .width(Length::Fill),
                    button(text("Unhide").size(p::TEXT_SM))
                        .padding([5.0, 10.0])
                        .style(move |_, status| t.quiet_button_style(false, status))
                        .on_press(UiMessage::UnhideAccount(id)),
                ]
                .spacing(8.0)
                .align_y(Alignment::Center),
            );
        }
        for id in &settings.hidden_accounts {
            if hidden_accounts.iter().any(|a| &a.id == id) {
                continue;
            }
            let id_owned = id.clone();
            hidden_col = hidden_col.push(
                row![
                    text("Not connected")
                        .size(p::TEXT_SM)
                        .color(t.semantic.text_muted)
                        .width(Length::Fill),
                    button(text("Unhide").size(p::TEXT_SM))
                        .padding([5.0, 10.0])
                        .style(move |_, status| t.quiet_button_style(false, status))
                        .on_press(UiMessage::UnhideAccount(id_owned)),
                ]
                .spacing(8.0)
                .align_y(Alignment::Center),
            );
        }
    }
    let hidden_group = container(hidden_col)
        .padding(10.0)
        .width(Length::Fill)
        .style(move |_| t.subtle_panel_style());

    let scroll_body = scrollable(
        column![
            appearance_group,
            section_heading(&t, "Reset reminders"),
            reminders_group,
            section_heading(&t, "Sources"),
            sources_group,
            section_heading(&t, "Hidden accounts"),
            hidden_group,
        ]
        .spacing(10.0),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .spacing(4.0)
    .style(move |_, status| t.scrollable_style(status));

    column![
        header,
        rule::horizontal(1).style(move |_| t.rule_style()),
        scroll_body,
    ]
    .spacing(10.0)
    .height(Length::Fill)
    .into()
}
