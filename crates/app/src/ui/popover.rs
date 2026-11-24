//! Single compact click-popover window: top tabs + 1-click refresh, scrollable account list,
//! inline + Add account picker, Pin toggle, FAQ, Settings, and update badge.

use crate::connect::UrlTarget;
use crate::scheduler::Detected;
use crate::settings::{ProviderTab, Settings};
use crate::state::format_updated_ago;
use crate::ui::tokens::{primitives as p, DesignTokens};
use crate::ui::{
    account_card, faq, icons, onboarding, settings as settings_view, tab_label,
    visible_accounts_for_tab, Page, UiMessage, UiState,
};
use iced::font;
use iced::widget::{button, column, container, row, rule, scrollable, space, text};
use iced::{Alignment, Element, Font, Length};
use quotaglass_core::view::Snapshot;
use quotaglass_core::Service;

const TABS: [ProviderTab; 4] = [
    ProviderTab::All,
    ProviderTab::Claude,
    ProviderTab::Codex,
    ProviderTab::Antigravity,
];

fn top_bar<'a>(
    tokens: &DesignTokens,
    ui: &UiState,
    settings: &Settings,
) -> Element<'a, UiMessage> {
    let t = *tokens;
    let mut tab_row = row![].spacing(4.0).align_y(Alignment::Center);
    for tab in TABS {
        let active = settings.active_tab == tab;
        tab_row = tab_row.push(
            button(
                container(text(tab_label(tab)).size(p::TEXT_SM).font(Font {
                    weight: if active {
                        font::Weight::Semibold
                    } else {
                        font::Weight::Normal
                    },
                    ..Font::DEFAULT
                }))
                .center_y(Length::Fill),
            )
            .padding([0.0, 9.0])
            .height(Length::Fixed(p::TARGET_MIN_H))
            .style(move |_, status| t.tab_button_style(active, status))
            .on_press(UiMessage::SelectTab(tab)),
        );
    }

    let can_refresh = ui.pending.can_refresh();
    let refresh_icon_color = if can_refresh {
        t.semantic.text_primary
    } else {
        t.semantic.text_disabled
    };
    let refresh_btn = button(
        container(
            row![
                icons::tinted_svg(icons::REFRESH_SVG, 14.0, refresh_icon_color),
                text(if can_refresh { "Refresh" } else { "..." }).size(p::TEXT_SM),
            ]
            .spacing(5.0)
            .align_y(Alignment::Center),
        )
        .center_y(Length::Fill),
    )
    .padding([0.0, 10.0])
    .height(Length::Fixed(p::TARGET_MIN_H))
    .style(move |_, status| t.quiet_button_style(false, status))
    .on_press_maybe(can_refresh.then_some(UiMessage::RefreshNow));

    row![tab_row, space::horizontal(), refresh_btn]
        .spacing(6.0)
        .align_y(Alignment::Center)
        .into()
}

fn list_page<'a>(
    tokens: &DesignTokens,
    ui: &'a UiState,
    settings: &'a Settings,
    snapshot: &Snapshot,
    update_version: Option<&str>,
    now_ms: i64,
) -> Element<'a, UiMessage> {
    let t = *tokens;
    let mut root = column![top_bar(&t, ui, settings)].spacing(8.0);

    if let Some(problem) = &ui.problem_banner {
        root = root.push(
            container(
                row![
                    text(problem.clone())
                        .size(p::TEXT_SM)
                        .color(t.semantic.danger)
                        .width(Length::Fill),
                    button(text("Dismiss").size(p::TEXT_XS))
                        .padding([3.0, 8.0])
                        .style(move |_, status| t.ghost_button_style(status))
                        .on_press(UiMessage::DismissProblem),
                ]
                .spacing(8.0)
                .align_y(Alignment::Center),
            )
            .padding([6.0, 10.0])
            .width(Length::Fill)
            .style(move |_| t.danger_panel_style()),
        );
    }

    let visible = visible_accounts_for_tab(&snapshot.accounts, settings.active_tab);
    let status_text = if !ui.pending.can_refresh() {
        "Refreshing...".to_string()
    } else if ui.has_snapshot && snapshot.generated_at_ms > 0 {
        format!("Updated {}", format_updated_ago(now_ms, snapshot.generated_at_ms))
    } else {
        "Ready".to_string()
    };

    let count_text = if visible.len() == 1 {
        "1 account".to_string()
    } else {
        format!("{} accounts", visible.len())
    };

    root = root.push(
        row![
            text(status_text).size(p::TEXT_XS).color(t.semantic.text_muted),
            space::horizontal(),
            text(count_text).size(p::TEXT_XS).color(t.semantic.text_muted),
        ]
        .align_y(Alignment::Center),
    );

    // Scrollable account list capped cleanly at 420.0 logical px.
    let mut list_col = column![].spacing(8.0);
    if ui.is_skeleton_loading(snapshot) {
        list_col = list_col
            .push(account_card::skeleton_card(&t, ui.animations.shimmer_phase))
            .push(account_card::skeleton_card(&t, ui.animations.shimmer_phase));
    } else if visible.is_empty() {
        let empty_msg = match settings.active_tab {
            ProviderTab::All => {
                "No accounts yet. Connect a source in Settings or use + Add account below."
            }
            ProviderTab::Claude => "No Claude accounts connected yet.",
            ProviderTab::Codex => "No Codex accounts connected yet.",
            ProviderTab::Antigravity => {
                "No Antigravity limits yet. Open Antigravity IDE or T3 Code to read live limits."
            }
        };
        list_col = list_col.push(
            container(
                text(empty_msg)
                    .size(p::TEXT_SM)
                    .color(t.semantic.text_secondary),
            )
            .padding(14.0)
            .width(Length::Fill)
            .style(move |_| t.subtle_panel_style()),
        );
    } else {
        for acct in visible {
            list_col = list_col.push(account_card::view(&t, ui, acct, now_ms));
        }
    }

    let list_scroll = container(
        scrollable(list_col)
            .width(Length::Fill)
            .height(Length::Shrink)
            .spacing(4.0)
            .style(move |_, status| t.scrollable_style(status)),
    )
    .width(Length::Fill)
    .max_height(p::ACCOUNT_LIST_MAX_H);
    root = root.push(list_scroll);

    // Compact inline + Add account picker.
    if ui.add_picker_open {
        let adding = ui.pending.adding_service;
        let can_add = ui.pending.can_add();
        let picker = container(
            column![
                text(match adding {
                    Some(Service::Claude) => "Opening Claude sign-in window...",
                    Some(Service::Codex) => "Opening Codex sign-in window...",
                    _ => "Sign in to a new account",
                })
                .size(p::TEXT_SM)
                .color(t.semantic.text_secondary),
                row![
                    button(text("Claude").size(p::TEXT_SM))
                        .padding([6.0, 12.0])
                        .style(move |_, status| t.quiet_button_style(false, status))
                        .on_press_maybe(can_add.then_some(UiMessage::AddAccount(Service::Claude))),
                    button(text("Codex").size(p::TEXT_SM))
                        .padding([6.0, 12.0])
                        .style(move |_, status| t.quiet_button_style(false, status))
                        .on_press_maybe(can_add.then_some(UiMessage::AddAccount(Service::Codex))),
                    space::horizontal(),
                    button(text("Cancel").size(p::TEXT_SM))
                        .padding([6.0, 10.0])
                        .style(move |_, status| t.ghost_button_style(status))
                        .on_press_maybe(can_add.then_some(UiMessage::ToggleAddPicker)),
                ]
                .spacing(6.0)
                .align_y(Alignment::Center),
            ]
            .spacing(6.0),
        )
        .padding([8.0, 10.0])
        .width(Length::Fill)
        .style(move |_| t.subtle_panel_style());
        root = root.push(picker);
    }

    // Update badge shown ONLY when check_updates is enabled and a newer release exists.
    if settings.check_updates {
        if let Some(ver) = update_version {
            root = root.push(
                button(
                    container(
                        text(format!("Update to v{ver}"))
                            .size(p::TEXT_SM)
                            .font(Font {
                                weight: font::Weight::Semibold,
                                ..Font::DEFAULT
                            }),
                    )
                    .center_x(Length::Fill),
                )
                .padding([7.0, 12.0])
                .width(Length::Fill)
                .style(move |_, status| t.primary_button_style(status))
                .on_press(UiMessage::OpenUrl(UrlTarget::GitHubReleases)),
            );
        }
    }

    root = root.push(rule::horizontal(1).style(move |_| t.rule_style()));

    // Footer actions.
    let can_add = ui.pending.can_add();
    let pin_label = if settings.pill_locked {
        "Unpin"
    } else {
        "Pin"
    };
    let footer = row![
        button(text("+ Add account").size(p::TEXT_SM))
            .padding([6.0, 9.0])
            .style(move |_, status| t.quiet_button_style(ui.add_picker_open, status))
            .on_press_maybe(can_add.then_some(UiMessage::ToggleAddPicker)),
        space::horizontal(),
        button(
            row![
                icons::tinted_svg(icons::PIN_SVG, 13.0, t.semantic.text_secondary),
                text(pin_label).size(p::TEXT_SM),
            ]
            .spacing(4.0)
            .align_y(Alignment::Center),
        )
        .padding([6.0, 9.0])
        .style(move |_, status| t.quiet_button_style(settings.pill_locked, status))
        .on_press(UiMessage::TogglePin),
        button(
            row![
                icons::tinted_svg(icons::HELP_SVG, 13.0, t.semantic.text_secondary),
                text("FAQ").size(p::TEXT_SM),
            ]
            .spacing(4.0)
            .align_y(Alignment::Center),
        )
        .padding([6.0, 9.0])
        .style(move |_, status| t.ghost_button_style(status))
        .on_press(UiMessage::OpenPage(Page::Faq)),
        button(
            row![
                icons::tinted_svg(icons::SLIDERS_SVG, 13.0, t.semantic.text_secondary),
                text("Settings").size(p::TEXT_SM),
            ]
            .spacing(4.0)
            .align_y(Alignment::Center),
        )
        .padding([6.0, 9.0])
        .style(move |_, status| t.ghost_button_style(status))
        .on_press(UiMessage::OpenPage(Page::Settings)),
    ]
    .spacing(4.0)
    .align_y(Alignment::Center);

    root.push(footer).into()
}

pub fn view<'a>(
    tokens: &DesignTokens,
    ui: &'a UiState,
    settings: &'a Settings,
    snapshot: &Snapshot,
    detected: Option<Detected>,
    update_version: Option<&str>,
    now_ms: i64,
) -> Element<'a, UiMessage> {
    let t = *tokens;
    let content = match ui.page {
        Page::List => list_page(&t, ui, settings, snapshot, update_version, now_ms),
        Page::Settings => settings_view::view(&t, settings, detected, &snapshot.accounts),
        Page::Faq => faq::view(&t),
        Page::Onboarding => onboarding::view(&t, settings, detected),
    };

    container(content)
        .padding(12.0)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| t.popover_container_style())
        .into()
}
