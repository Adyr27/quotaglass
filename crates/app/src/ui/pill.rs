//! Floating capsule pill window: Canvas progress ring + centered SVG provider mark per service,
//! drag-to-move threshold when unpinned, and right-edge resize grip.

use crate::settings::Settings;
use crate::ui::icons;
use crate::ui::tokens::DesignTokens;
use crate::ui::{UiMessage, UiState};
use crate::win32::{PILL_GAP, PILL_PAD, PILL_SLOT};
use iced::mouse;
use iced::widget::canvas::{self, LineCap, LineDash, Path, Stroke};
use iced::widget::{container, row, space, stack, Canvas, MouseArea};
use iced::{Alignment, Color, Element, Length, Point, Radians, Rectangle, Renderer, Theme};
use quotaglass_core::view::{ServiceView, Snapshot};
use quotaglass_core::Service;
use std::f32::consts::{FRAC_PI_2, TAU};

struct RingCanvas {
    progress: Option<f32>,
    stroke_color: Color,
    track_color: Color,
    pulse_alpha: f32,
    dashed: bool,
    stroke_width: f32,
}

impl<Message> canvas::Program<Message> for RingCanvas {
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
        let center = frame.center();
        let radius = (bounds.width.min(bounds.height) * 0.5 - self.stroke_width * 0.65).max(2.0);

        let track_circle = Path::circle(center, radius);
        let mut track_stroke = Stroke::default()
            .with_color(self.track_color)
            .with_width(self.stroke_width);
        if self.dashed {
            track_stroke.line_dash = LineDash {
                segments: &[3.0, 2.5],
                offset: 0,
            };
        }
        frame.stroke(&track_circle, track_stroke);

        if let Some(fill) = self.progress {
            let clamped = fill.clamp(0.0, 1.0);
            if clamped > 0.005 {
                let start = Radians(-FRAC_PI_2);
                let end = Radians(-FRAC_PI_2 + TAU * clamped);
                let arc = Path::new(|builder| {
                    builder.arc(canvas::path::Arc {
                        center,
                        radius,
                        start_angle: start,
                        end_angle: end,
                    });
                });
                let mut color = self.stroke_color;
                color.a *= self.pulse_alpha;
                frame.stroke(
                    &arc,
                    Stroke::default()
                        .with_color(color)
                        .with_width(self.stroke_width)
                        .with_line_cap(LineCap::Round),
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

struct GripCanvas {
    dot_color: Color,
    visible: bool,
    dot_radius: f32,
}

impl<Message> canvas::Program<Message> for GripCanvas {
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
        if !self.visible {
            return vec![frame.into_geometry()];
        }
        let cx = bounds.width * 0.5;
        let cy = bounds.height * 0.5;
        let step = (self.dot_radius * 3.2).max(3.5);
        for dy in [-step, 0.0, step] {
            let dot = Path::circle(Point::new(cx, cy + dy), self.dot_radius);
            frame.fill(&dot, self.dot_color);
        }
        vec![frame.into_geometry()]
    }
}

fn pulse_factor(phase: f32, active: bool) -> f32 {
    if !active {
        1.0
    } else {
        0.52 + 0.48 * (0.5 + 0.5 * (phase * TAU).sin())
    }
}

fn slot_ring_color(tokens: &DesignTokens, sv: &ServiceView) -> Color {
    if sv.all_exhausted {
        tokens.semantic.danger
    } else if sv.used_percent.is_some_and(|p| p >= 80.0) {
        tokens.semantic.warn
    } else {
        tokens.service_accent(sv.service)
    }
}

fn render_service_slot<'a>(
    tokens: &DesignTokens,
    ui: &UiState,
    sv: &ServiceView,
    scale: f32,
) -> Element<'a, UiMessage> {
    let slot_size = PILL_SLOT as f32 * scale;
    let icon_size = 11.5 * scale;
    let stroke_width = (2.3 * scale).max(1.5);
    let target = sv.used_percent.map(|p| (p as f32 / 100.0).clamp(0.0, 1.0));
    let animated = target.map(|t| ui.animations.ring_fill(sv.service, t));
    let pulsing = sv.rate_limited || ui.pending.refreshing;
    let pulse = pulse_factor(ui.animations.pulse_phase, pulsing);

    let ring = Canvas::new(RingCanvas {
        progress: animated,
        stroke_color: slot_ring_color(tokens, sv),
        track_color: tokens.semantic.track_bg,
        pulse_alpha: pulse,
        dashed: sv.used_percent.is_none(),
        stroke_width,
    })
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size));

    let mut icon_color = tokens.service_accent(sv.service);
    if sv.used_percent.is_none() {
        icon_color.a *= 0.65;
    } else if pulsing {
        icon_color.a *= pulse;
    }

    let badge = stack![
        ring,
        container(icons::provider_icon(sv.service, icon_size, icon_color))
            .center_x(Length::Fixed(slot_size))
            .center_y(Length::Fixed(slot_size))
    ]
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size));

    MouseArea::new(badge)
        .on_enter(UiMessage::PillSlotHovered(Some(sv.service)))
        .on_exit(UiMessage::PillSlotHovered(None))
        .on_press(UiMessage::PillSlotPressed(Some(sv.service)))
        .on_release(UiMessage::PillSlotReleased(Some(sv.service)))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn render_empty_slot<'a>(
    tokens: &DesignTokens,
    ui: &UiState,
    scale: f32,
) -> Element<'a, UiMessage> {
    let slot_size = PILL_SLOT as f32 * scale;
    let icon_size = 12.0 * scale;
    let stroke_width = (2.2 * scale).max(1.5);
    let pulse = pulse_factor(ui.animations.pulse_phase, ui.pending.refreshing);

    let ring = Canvas::new(RingCanvas {
        progress: None,
        stroke_color: tokens.semantic.text_primary,
        track_color: tokens.semantic.track_bg,
        pulse_alpha: pulse,
        dashed: true,
        stroke_width,
    })
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size));

    let badge = stack![
        ring,
        container(icons::tinted_svg(
            icons::QUOTAGLASS_SVG,
            icon_size,
            Color {
                a: 0.85 * pulse,
                ..tokens.semantic.text_primary
            },
        ))
        .center_x(Length::Fixed(slot_size))
        .center_y(Length::Fixed(slot_size))
    ]
    .width(Length::Fixed(slot_size))
    .height(Length::Fixed(slot_size));

    MouseArea::new(badge)
        .on_press(UiMessage::PillSlotPressed(None))
        .on_release(UiMessage::PillSlotReleased(None))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// Computes horizontal center anchor (in physical px relative to pill left) for the clicked slot.
pub fn slot_anchor_x_px(
    services: &[ServiceView],
    target: Option<Service>,
    pill_width_px: u32,
) -> i32 {
    let count = services.len().max(1);
    let idx = target
        .and_then(|svc| services.iter().position(|s| s.service == svc))
        .unwrap_or(0);
    if count <= 1 {
        return (pill_width_px / 2) as i32;
    }
    let total_units =
        (PILL_PAD * 2 + count as u32 * PILL_SLOT + (count as u32 - 1) * PILL_GAP) as f64;
    let center_unit =
        PILL_PAD as f64 + idx as f64 * (PILL_SLOT + PILL_GAP) as f64 + PILL_SLOT as f64 * 0.5;
    ((center_unit / total_units) * f64::from(pill_width_px)).round() as i32
}

pub fn view<'a>(
    tokens: &DesignTokens,
    ui: &UiState,
    settings: &Settings,
    snapshot: &Snapshot,
) -> Element<'a, UiMessage> {
    let scale = settings.pill_scale as f32;
    let pad_h = PILL_PAD as f32 * scale;
    let gap = PILL_GAP as f32 * scale;

    let mut slots_row = row![].spacing(gap).align_y(Alignment::Center);
    if snapshot.services.is_empty() {
        slots_row = slots_row.push(render_empty_slot(tokens, ui, scale));
    } else {
        for sv in &snapshot.services {
            slots_row = slots_row.push(render_service_slot(tokens, ui, sv, scale));
        }
    }

    let left_pad = space().width(Length::Fixed(pad_h));
    let body: Element<'a, UiMessage> = if settings.pill_locked {
        row![left_pad, slots_row, space().width(Length::Fixed(pad_h))]
            .align_y(Alignment::Center)
            .into()
    } else {
        // Reserve clean breathing space between the rightmost ring and the 3-dot grip,
        // keeping exact horizontal sum == 2 * PILL_PAD * scale at every scale (0.85..=2.0).
        let right_left_pad = space().width(Length::Fixed((PILL_PAD as f32 - 1.0) * scale));
        let grip_gap = space().width(Length::Fixed(4.0 * scale));
        let grip_w = 7.0 * scale;
        let grip_h = 22.0 * scale;
        let dot_alpha = if ui.grip_hovered {
            0.94
        } else if ui.pill_hovered {
            0.62
        } else {
            0.0
        };
        let grip = MouseArea::new(
            Canvas::new(GripCanvas {
                dot_color: Color {
                    a: dot_alpha,
                    ..tokens.semantic.grip_dot
                },
                visible: ui.pill_hovered || ui.grip_hovered,
                dot_radius: (1.25 * scale).clamp(1.0, 2.3),
            })
            .width(Length::Fixed(grip_w))
            .height(Length::Fixed(grip_h)),
        )
        .on_enter(UiMessage::PillGripHovered(true))
        .on_exit(UiMessage::PillGripHovered(false))
        .on_press(UiMessage::PillGripPressed)
        .interaction(mouse::Interaction::ResizingHorizontally);

        row![right_left_pad, slots_row, grip_gap, grip]
            .align_y(Alignment::Center)
            .into()
    };

    let capsule = container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center)
        .style({
            let tokens = *tokens;
            move |_| tokens.pill_container_style()
        });

    MouseArea::new(capsule)
        .on_enter(UiMessage::PillHovered(true))
        .on_exit(UiMessage::PillHovered(false))
        .into()
}

#[cfg(test)]
mod tests {
    use crate::settings::PILL_SCALE;
    use crate::win32::{pill_px_size, PILL_H};

    #[test]
    fn pill_scales_and_slots_stay_proportional_from_085_to_200() {
        for scale in [PILL_SCALE.0, 1.0, 1.5, PILL_SCALE.1] {
            for slots in 1u32..=3 {
                let (w, h) = pill_px_size(slots, scale, 1.0);
                let expected_h = (f64::from(PILL_H) * scale).round() as u32;
                assert_eq!(h, expected_h);
                assert!(w > h, "pill must remain horizontal capsule/rounded bar at scale={scale}, slots={slots}");
            }
        }
    }
}
