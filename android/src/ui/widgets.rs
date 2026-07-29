//! Shared Vidya-styled widgets.

use eframe::egui::{self, Align, Color32, Layout, RichText, Sense, Stroke, Vec2};
use vidya::{body, dim_label, title_2, Theme};

use crate::state::{Buffer, ChatMessage};

/// Card frame filling parent width.
pub fn card(ui: &mut egui::Ui, th: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    let outer = ui.available_width();
    ui.set_max_width(outer);
    th.card_frame().show(ui, |ui| {
        let inner = ui.available_width().max(1.0);
        ui.set_min_width(inner);
        ui.set_max_width(inner);
        add(ui);
    });
}

pub fn section_label(ui: &mut egui::Ui, th: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .size(th.type_scale.caption)
            .color(th.palette.text_secondary)
            .strong(),
    );
}

/// Colored initial circle (freeq-style avatar stand-in).
pub fn avatar_circle(ui: &mut egui::Ui, th: &Theme, name: &str, size: f32) {
    let letter = name
        .trim_start_matches(['#', '&', '@', '+', '%'])
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());

    let hue = hash_hue(name);
    let fill = Color32::from_rgb(
        ((hue * 0.6 + 40.0) as u8).saturating_add(30),
        ((hue * 0.4 + 80.0) as u8).min(200),
        ((200.0 - hue * 0.3) as u8).max(80),
    );

    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size * 0.5, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional((size * 0.42).max(10.0)),
        th.palette.accent_fg,
    );
}

fn hash_hue(s: &str) -> f32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    (h % 160) as f32
}

/// One row in the chats list.
pub fn conversation_row(
    ui: &mut egui::Ui,
    th: &Theme,
    buf: &Buffer,
    selected: bool,
) -> egui::Response {
    let p = &th.palette;
    let sp = &th.spacing;
    let fill = if selected {
        p.accent.gamma_multiply(0.22)
    } else {
        Color32::TRANSPARENT
    };

    let resp = egui::Frame::new()
        .fill(fill)
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                avatar_circle(ui, th, &buf.name, 40.0);
                ui.add_space(sp.md);
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 36.0).max(40.0));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(buf.display_name())
                                .size(th.type_scale.body)
                                .color(p.text)
                                .strong(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if buf.unread > 0 {
                                badge(ui, th, &format!("{}", buf.unread.min(99)));
                            }
                        });
                    });
                    ui.add_space(2.0);
                    let preview = buf.last_preview();
                    let truncated = if preview.chars().count() > 48 {
                        let t: String = preview.chars().take(48).collect();
                        format!("{t}…")
                    } else {
                        preview
                    };
                    ui.label(
                        RichText::new(truncated)
                            .size(th.type_scale.caption)
                            .color(p.text_secondary),
                    );
                });
            });
        })
        .response
        .interact(Sense::click());

    if resp.hovered() && !selected {
        ui.painter().rect_filled(
            resp.rect,
            sp.radius_md,
            p.button_hover.gamma_multiply(0.45),
        );
    }
    resp
}

fn badge(ui: &mut egui::Ui, th: &Theme, text: &str) {
    let p = &th.palette;
    egui::Frame::new()
        .fill(p.accent)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .size(th.type_scale.caption)
                    .color(p.accent_fg)
                    .strong(),
            );
        });
}

/// Message bubble / row in chat detail.
pub fn message_bubble(ui: &mut egui::Ui, th: &Theme, msg: &ChatMessage, own_nick: &str) {
    let p = &th.palette;
    let sp = &th.spacing;
    let is_own = !msg.is_system && msg.from.eq_ignore_ascii_case(own_nick);

    if msg.is_system {
        ui.horizontal(|ui| {
            ui.add_space(sp.sm);
            dim_label(ui, th, &format!("· {}", msg.text));
        });
        return;
    }

    if msg.is_deleted {
        dim_label(ui, th, "Message deleted");
        return;
    }

    let body_text = if msg.is_action {
        format!("* {} {}", msg.from, msg.text)
    } else {
        msg.text.clone()
    };

    let bg = if is_own {
        p.accent.gamma_multiply(0.28)
    } else {
        p.card_bg
    };

    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, p.border_soft))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                if !is_own {
                    ui.label(
                        RichText::new(&msg.from)
                            .size(th.type_scale.caption)
                            .color(p.accent)
                            .strong(),
                    );
                } else {
                    ui.label(
                        RichText::new("You")
                            .size(th.type_scale.caption)
                            .color(p.accent)
                            .strong(),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut meta = msg.time_label();
                    if msg.is_edited {
                        meta = format!("edited · {meta}");
                    }
                    if msg.is_signed {
                        meta = format!("✓ {meta}");
                    }
                    dim_label(ui, th, &meta);
                });
            });
            ui.add_space(sp.xs);
            body(ui, th, &body_text);
        });
}

#[allow(dead_code)]
pub fn empty_state(ui: &mut egui::Ui, th: &Theme, title: &str, blurb: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        title_2(ui, th, title);
        ui.add_space(th.spacing.sm);
        dim_label(ui, th, blurb);
    });
}
