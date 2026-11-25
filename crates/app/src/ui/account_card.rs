//! Collapsed/expanded account cards, animated Canvas pace strips, inline resets, and skeleton cards.

use crate::connect::UrlTarget;
use crate::state::format_reset_time;
use crate::ui::icons;
use crate::ui::tokens::{primitives as p, ui_font, DesignTokens};
use crate::ui::{
    forecast_line, friendly_state_notice, resets_chip_text, sentence_case, sign_in_button_label,
    status_dot, used_whole, UiMessage, UiState,
};
use iced::border;
use iced::font;
use iced::mouse;
use iced::widget::canvas::{self, Path};
use iced::widget::{
    button, column, container, row, rule, space, text, text_input, Canvas, MouseArea,
};
use iced::{Alignment, Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use quotaglass_core::view::{AccountView, BucketView, Freshness};
use quotaglass_core::Service;

struct PaceStripCanvas {
    fill_fraction: f32,
    passed_fraction: Option<f32>,
    fill_color: Color,
    track_color: Color,
    tick_color: Color,
    pace_color: Color,
}

impl<Message> canvas::Program<Message> for PaceStripCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width.max(1.0);
        let h = bounds.height.max(1.0);
        let radius = border::radius(h * 0.5);

        let track = Path::rounded_rectangle(Point::ORIGIN, Size::new(w, h), radius);
        frame.fill(&track, self.track_color);

        let fill_w = (w * self.fill_fraction.clamp(0.0, 1.0)).clamp(0.0, w);
        if fill_w > 1.0 {
            let bar_r = border::radius((h * 0.5).min(fill_w * 0.5));
            let bar = Path::rounded_rectangle(Point::ORIGIN, Size::new(fill_w, h), bar_r);
            frame.fill(&bar, self.fill_color);
        }

        // Nine 10% divider ticks inside the meter.
        for step in 1..10 {
            let tx = w * (step as f32 / 10.0);
            frame.fill_rectangle(
                Point::new(tx - 0.5, 0.0),
                Size::new(1.0, h),
                self.tick_color,
            );
        }

        // Elapsed-time vertical pace marker when window length and reset timestamp exist.
        if let Some(passed) = self.passed_fraction {
            let px = (w * passed.clamp(0.0, 1.0)).clamp(1.0, (w - 2.0).max(1.0));
            frame.fill_rectangle(
                Point::new(px - 1.0, -1.0),
                Size::new(2.0, h + 2.0),
                self.pace_color,
            );
        }

        vec![frame.into_geometry()]
    }
}

struct ShimmerCanvas {
    phase: f32,
    base_color: Color,
    highlight_color: Color,
}

impl<Message> canvas::Program<Message> for ShimmerCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width.max(1.0);
        let h = bounds.height.max(1.0);
        let r = border::radius(h * 0.5);
        frame.fill(
            &Path::rounded_rectangle(Point::ORIGIN, Size::new(w, h), r),
            self.base_color,
        );

        let band_w = (w * 0.36).max(28.0);
        let x = -band_w + (w + band_w * 2.0) * self.phase.clamp(0.0, 1.0);
        let visible_x = x.clamp(0.0, w);
        let visible_end = (x + band_w).clamp(0.0, w);
        if visible_end > visible_x + 1.0 {
            let band = Path::rounded_rectangle(
                Point::new(visible_x, 0.0),
                Size::new(visible_end - visible_x, h),
                r,
            );
            frame.fill(&band, self.highlight_color);
        }

        vec![frame.into_geometry()]
    }
}

fn bucket_fill_color(
    tokens: &DesignTokens,
    service: Service,
    b: &BucketView,
    stale: bool,
) -> Color {
    let base = if b.fast_burn || b.limit_reached {
        tokens.semantic.danger
    } else if b.used_percent >= 80.0 {
        tokens.semantic.warn
    } else {
        tokens.service_accent(service)
    };
    if stale {
        Color { a: 0.45, ..base }
    } else {
        base
    }
}

fn pace_strip<'a>(
    tokens: &DesignTokens,
    account: &AccountView,
    b: &BucketView,
    height: f32,
    ui: &UiState,
) -> Element<'a, UiMessage> {
    let target = (b.used_percent as f32 / 100.0).clamp(0.0, 1.0);
    let fill_fraction = ui.animations.bar_fill(&account.id, &b.name, target);
    let passed_fraction = b.passed_percent.map(|p| (p as f32 / 100.0).clamp(0.0, 1.0));
    let stale = b.freshness == Freshness::Stale;

    Canvas::new(PaceStripCanvas {
        fill_fraction,
        passed_fraction,
        fill_color: bucket_fill_color(tokens, account.service, b, stale),
        track_color: tokens.semantic.track_bg,
        tick_color: tokens.semantic.tick_line,
        pace_color: tokens.semantic.pace_marker,
    })
    .width(Length::Fill)
    .height(Length::Fixed(height))
    .into()
}

fn reset_label(b: &BucketView, now_ms: i64) -> String {
    if b.limit_reached {
        "Limit reached".to_string()
    } else if let Some(at) = b.resets_at_ms {
        format_reset_time(now_ms, at)
            .map(|s| format!("resets {s}"))
            .unwrap_or_default()
    } else {
        String::new()
    }
}

fn chip<'a>(
    label: String,
    style_fn: impl Fn(&Theme) -> container::Style + 'a,
    size: f32,
    weight: font::Weight,
) -> Element<'a, UiMessage> {
    container(text(label).size(size).font(ui_font(weight)))
        .padding([2.0, 8.0])
        .style(style_fn)
        .into()
}

fn burn_chip<'a>(tokens: &DesignTokens) -> Element<'a, UiMessage> {
    let t = *tokens;
    container(
        row![
            status_dot(t.semantic.danger),
            text("fast burn")
                .size(p::TEXT_XS)
                .font(ui_font(font::Weight::Medium))
                .color(t.semantic.burn_ink),
        ]
        .spacing(5.0)
        .align_y(Alignment::Center),
    )
    .padding([2.0, 8.0])
    .style(move |_| t.burn_chip_style())
    .into()
}

pub fn skeleton_card<'a>(tokens: &DesignTokens, phase: f32) -> Element<'a, UiMessage> {
    let t = *tokens;
    let bar = |w: Length, h: f32| {
        Canvas::new(ShimmerCanvas {
            phase,
            base_color: t.semantic.shimmer_base,
            highlight_color: t.semantic.shimmer_highlight,
        })
        .width(w)
        .height(Length::Fixed(h))
    };

    container(
        column![
            row![
                bar(Length::Fixed(18.0), 18.0),
                bar(Length::Fixed(130.0), 14.0),
                space::horizontal(),
                bar(Length::Fixed(38.0), 14.0),
            ]
            .spacing(8.0)
            .align_y(Alignment::Center),
            bar(Length::Fill, 8.0),
            row![
                bar(Length::Fixed(92.0), 11.0),
                space::horizontal(),
                bar(Length::Fixed(68.0), 11.0),
            ],
        ]
        .spacing(8.0),
    )
    .padding(12.0)
    .width(Length::Fill)
    .style(move |_| t.card_container_style(false, false))
    .into()
}

pub fn view<'a>(
    tokens: &DesignTokens,
    ui: &UiState,
    account: &AccountView,
    now_ms: i64,
) -> Element<'a, UiMessage> {
    let t = *tokens;
    let accent = t.service_accent(account.service);
    let expanded = ui.expanded_id.as_deref() == Some(account.id.as_str());
    let expand_t = ui.animations.expand_progress(&account.id, expanded);

    let headline = account
        .headline
        .as_ref()
        .or_else(|| account.buckets.first());
    let pct_text = headline
        .map(|h| format!("{}%", used_whole(h.used_percent)))
        .unwrap_or_else(|| "—".to_string());

    let mut title_row = row![
        icons::provider_icon(account.service, 16.0, accent),
        text(account.label.clone())
            .size(p::TEXT_MD)
            .font(ui_font(font::Weight::Semibold))
            .color(t.semantic.text_primary),
    ]
    .spacing(7.0)
    .align_y(Alignment::Center);

    if let Some(plan) = &account.plan {
        let label = sentence_case(plan);
        if !label.is_empty() {
            title_row = title_row.push(chip(
                label,
                move |_| t.plan_chip_style(),
                p::TEXT_XS,
                font::Weight::Medium,
            ));
        }
    }

    if account.best_now {
        title_row = title_row.push(chip(
            "Best now".to_string(),
            move |_| t.best_chip_style(),
            p::TEXT_XS,
            font::Weight::Semibold,
        ));
    }

    if account.service == Service::Codex && account.banked_resets.is_some_and(|n| n > 0) {
        let count = account.banked_resets.unwrap_or(0);
        title_row = title_row.push(chip(
            resets_chip_text(count),
            move |_| {
                t.chip_style(
                    t.semantic.surface_control,
                    t.semantic.text_primary,
                    t.semantic.border_subtle,
                )
            },
            p::TEXT_XS,
            font::Weight::Medium,
        ));
    }

    let chevron = icons::tinted_svg(
        if expanded {
            icons::CHEVRON_UP_SVG
        } else {
            icons::CHEVRON_DOWN_SVG
        },
        14.0,
        t.semantic.text_muted,
    );

    let header_row = row![
        title_row,
        space::horizontal(),
        text(pct_text)
            .size(p::TEXT_MD)
            .font(ui_font(font::Weight::Semibold))
            .color(t.semantic.text_primary),
        chevron,
    ]
    .spacing(6.0)
    .align_y(Alignment::Center);

    let mut header_col = column![header_row].spacing(7.0);

    if !expanded {
        if let Some(b) = headline {
            header_col = header_col.push(pace_strip(&t, account, b, 8.0, ui));
            let mut meta = row![text(b.name.clone())
                .size(p::TEXT_XS)
                .color(t.semantic.text_muted)]
            .spacing(6.0)
            .align_y(Alignment::Center);
            let reset = reset_label(b, now_ms);
            if !reset.is_empty() {
                meta = meta.push(text("·").size(p::TEXT_XS).color(t.semantic.text_muted));
                meta = meta.push(text(reset).size(p::TEXT_XS).color(t.semantic.text_muted));
            }
            meta = meta.push(space::horizontal());
            if b.fast_burn {
                meta = meta.push(burn_chip(&t));
            }
            header_col = header_col.push(meta);
        }
    }

    if let Some(notice) = friendly_state_notice(account) {
        let notice_color = match account.state {
            quotaglass_core::view::CardState::LoginExpired
            | quotaglass_core::view::CardState::ProfileMissing
            | quotaglass_core::view::CardState::Error => t.semantic.warn,
            _ => t.semantic.text_secondary,
        };
        header_col = header_col.push(
            text(notice)
                .size(p::TEXT_SM)
                .line_height(1.3)
                .color(notice_color),
        );
    }

    let clickable_header: Element<'a, UiMessage> = MouseArea::new(header_col)
        .on_press(UiMessage::ToggleExpand(account.id.clone()))
        .interaction(mouse::Interaction::Pointer)
        .into();

    let mut card_body = column![clickable_header].spacing(10.0);

    if expanded || expand_t > 0.15 {
        card_body = card_body.push(rule::horizontal(1).style(move |_| t.rule_style()));

        // All buckets with full pace strips.
        if !account.buckets.is_empty() {
            let mut buckets_col = column![].spacing(10.0);
            for b in &account.buckets {
                let mut foot =
                    row![text(reset_label(b, now_ms))
                        .size(p::TEXT_XS)
                        .color(if b.limit_reached {
                            t.semantic.danger
                        } else {
                            t.semantic.text_muted
                        })]
                    .spacing(6.0)
                    .align_y(Alignment::Center);
                foot = foot.push(space::horizontal());
                if b.fast_burn {
                    foot = foot.push(burn_chip(&t));
                }

                buckets_col = buckets_col.push(
                    column![
                        row![
                            text(b.name.clone())
                                .size(p::TEXT_SM)
                                .color(t.semantic.text_secondary),
                            space::horizontal(),
                            text(format!("{}%", used_whole(b.used_percent)))
                                .size(p::TEXT_SM)
                                .font(ui_font(font::Weight::Semibold))
                                .color(t.semantic.text_primary),
                        ]
                        .align_y(Alignment::Center),
                        pace_strip(&t, account, b, 9.0, ui),
                        foot,
                    ]
                    .spacing(5.0),
                );
            }
            card_body = card_body.push(buckets_col);
        }

        if let Some(forecast) = forecast_line(account.forecast_mins) {
            let warn = account.forecast_mins.is_some_and(|m| m < 30);
            card_body = card_body.push(text(forecast).size(p::TEXT_SM).color(if warn {
                t.semantic.warn
            } else {
                t.semantic.text_secondary
            }));
        }

        // Inline reset/check/redeem controls right inside this account card.
        match account.service {
            Service::Codex => {
                if let Some(count) = account.banked_resets {
                    let mut reset_row = row![
                        text(resets_chip_text(count))
                            .size(p::TEXT_SM)
                            .font(ui_font(font::Weight::Medium))
                            .color(t.semantic.text_primary),
                        space::horizontal(),
                    ]
                    .align_y(Alignment::Center);
                    if count > 0 {
                        reset_row = reset_row.push(
                            button(
                                container(
                                    row![
                                        text("Redeem")
                                            .size(p::TEXT_SM)
                                            .font(ui_font(font::Weight::Medium)),
                                        icons::tinted_svg(
                                            icons::EXTERNAL_SVG,
                                            12.0,
                                            t.semantic.primary_ink,
                                        ),
                                    ]
                                    .spacing(5.0)
                                    .align_y(Alignment::Center),
                                )
                                .center_y(Length::Fill),
                            )
                            .padding([0.0, 12.0])
                            .height(Length::Fixed(36.0))
                            .style(move |_, status| t.primary_button_style(status))
                            .on_press(UiMessage::OpenUrl(UrlTarget::OpenAiUsage)),
                        );
                    }
                    let mut reset_col = column![reset_row].spacing(5.0);
                    if count > 0 {
                        reset_col = reset_col.push(
                            text("Check what this reset restores before you use it.")
                                .size(p::TEXT_XS)
                                .color(t.semantic.text_muted),
                        );
                    }
                    card_body = card_body.push(
                        container(reset_col)
                            .padding([9.0, 10.0])
                            .width(Length::Fill)
                            .style(move |_| t.subtle_panel_style()),
                    );
                }
            }
            Service::Claude => {
                card_body = card_body.push(row![
                    button(
                        container(
                            row![
                                text("Check limit resets").size(p::TEXT_SM),
                                icons::tinted_svg(
                                    icons::EXTERNAL_SVG,
                                    12.0,
                                    t.semantic.text_secondary,
                                ),
                            ]
                            .spacing(5.0)
                            .align_y(Alignment::Center),
                        )
                        .center_y(Length::Fill),
                    )
                    .padding([0.0, 12.0])
                    .height(Length::Fixed(36.0))
                    .style(move |_, status| t.quiet_button_style(false, status))
                    .on_press(UiMessage::OpenUrl(UrlTarget::ClaudeUsage)),
                    space::horizontal(),
                ]);
            }
            Service::Antigravity => {}
        }

        // Inline rename form when active for this card.
        if let Some(draft) = ui.rename.as_ref().filter(|r| r.account_id == account.id) {
            let mut rename_col = column![row![
                text_input("Account name (max 40 chars)", &draft.text)
                    .on_input(UiMessage::RenameDraftChanged)
                    .on_submit(UiMessage::SubmitRename)
                    .size(p::TEXT_SM)
                    .padding([7.0, 10.0])
                    .width(Length::Fill)
                    .style(move |_, status| t.text_input_style(status)),
                button(container(text("Save").size(p::TEXT_SM)).center_y(Length::Fill))
                    .padding([0.0, 12.0])
                    .height(Length::Fixed(36.0))
                    .style(move |_, status| t.primary_button_style(status))
                    .on_press(UiMessage::SubmitRename),
                button(container(text("Cancel").size(p::TEXT_SM)).center_y(Length::Fill))
                    .padding([0.0, 10.0])
                    .height(Length::Fixed(36.0))
                    .style(move |_, status| t.quiet_button_style(false, status))
                    .on_press(UiMessage::CancelRename),
            ]
            .spacing(6.0)
            .align_y(Alignment::Center),]
            .spacing(5.0);
            if let Some(err) = draft.error {
                rename_col = rename_col.push(text(err).size(p::TEXT_XS).color(t.semantic.danger));
            }
            card_body = card_body.push(
                container(rename_col)
                    .padding([8.0, 10.0])
                    .width(Length::Fill)
                    .style(move |_| t.subtle_panel_style()),
            );
        }

        // 2-step inline Remove confirmation for added profile accounts.
        let removing = ui.pending.is_removing(&account.id);
        if ui.confirm_remove_id.as_deref() == Some(account.id.as_str()) {
            let confirm_box = container(
                column![
                    text("Signs out and deletes this profile.")
                        .size(p::TEXT_SM)
                        .color(t.semantic.text_primary),
                    row![
                        button(
                            container(
                                text(if removing {
                                    "Removing..."
                                } else {
                                    "Confirm remove"
                                })
                                .size(p::TEXT_SM),
                            )
                            .center_y(Length::Fill),
                        )
                        .padding([0.0, 12.0])
                        .height(Length::Fixed(36.0))
                        .style(move |_, status| t.danger_button_style(status))
                        .on_press_maybe(
                            (!removing).then(|| UiMessage::ConfirmRemove(account.id.clone()))
                        ),
                        button(container(text("Cancel").size(p::TEXT_SM)).center_y(Length::Fill))
                            .padding([0.0, 12.0])
                            .height(Length::Fixed(36.0))
                            .style(move |_, status| t.quiet_button_style(false, status))
                            .on_press_maybe((!removing).then_some(UiMessage::CancelRemove)),
                    ]
                    .spacing(8.0),
                ]
                .spacing(8.0),
            )
            .padding([10.0, 12.0])
            .width(Length::Fill)
            .style(move |_| t.danger_panel_style());
            card_body = card_body.push(confirm_box);
        } else {
            // Card Action Bar.
            let refresh_busy = !ui.pending.can_refresh();
            let signing_in = ui.pending.is_signing_in(&account.id);
            let mut action_row = row![
                button(
                    container(
                        text(if refresh_busy {
                            "Refreshing..."
                        } else {
                            "Refresh"
                        })
                        .size(p::TEXT_SM),
                    )
                    .center_y(Length::Fill),
                )
                .padding([0.0, 11.0])
                .height(Length::Fixed(36.0))
                .style(move |_, status| t.quiet_button_style(false, status))
                .on_press_maybe((!refresh_busy).then_some(UiMessage::RefreshNow)),
                button(container(text("Rename").size(p::TEXT_SM)).center_y(Length::Fill))
                    .padding([0.0, 11.0])
                    .height(Length::Fixed(36.0))
                    .style(move |_, status| t.quiet_button_style(false, status))
                    .on_press(UiMessage::StartRename(account.id.clone())),
            ]
            .spacing(6.0)
            .align_y(Alignment::Center);

            if let Some(sign_label) = sign_in_button_label(account) {
                let btn_text = if signing_in {
                    "Opening...".to_string()
                } else {
                    sign_label
                };
                action_row = action_row.push(
                    button(container(text(btn_text).size(p::TEXT_SM)).center_y(Length::Fill))
                        .padding([0.0, 11.0])
                        .height(Length::Fixed(36.0))
                        .style(move |_, status| t.primary_button_style(status))
                        .on_press_maybe(
                            (!signing_in).then(|| UiMessage::SignInAccount(account.id.clone())),
                        ),
                );
            }

            action_row = action_row.push(space::horizontal());

            if account.profile_id.is_some() {
                action_row = action_row.push(
                    button(
                        container(
                            text(if removing { "Removing..." } else { "Remove" }).size(p::TEXT_SM),
                        )
                        .center_y(Length::Fill),
                    )
                    .padding([0.0, 11.0])
                    .height(Length::Fixed(36.0))
                    .style(move |_, status| t.danger_button_style(status))
                    .on_press_maybe(
                        (!removing).then(|| UiMessage::RequestRemove(account.id.clone())),
                    ),
                );
            } else {
                action_row = action_row.push(
                    button(container(text("Hide").size(p::TEXT_SM)).center_y(Length::Fill))
                        .padding([0.0, 11.0])
                        .height(Length::Fixed(36.0))
                        .style(move |_, status| t.quiet_button_style(false, status))
                        .on_press(UiMessage::HideAccount(account.id.clone())),
                );
            }

            card_body = card_body.push(action_row);
        }
    }

    container(card_body)
        .padding([10.0, 12.0])
        .width(Length::Fill)
        .style(move |_| t.card_container_style(expanded, false))
        .into()
}
