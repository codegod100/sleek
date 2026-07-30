//! Shared Vidya-styled widgets.

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, RichText, Sense, Stroke, Vec2};
use vidya::{body, dim_label, title_2, Theme};

use crate::preview::{self, Embed};
use crate::state::{
    Buffer, ChatMessage, ImageState, LinkMeta, LinkState, MediaCache,
};

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
///
/// `display_name` is the human label (nick for DID-keyed DMs); `buf.name` stays
/// the stable buffer key.
pub fn conversation_row(
    ui: &mut egui::Ui,
    th: &Theme,
    buf: &Buffer,
    display_name: &str,
    selected: bool,
) -> egui::Response {
    let p = &th.palette;
    let sp = &th.spacing;

    // Stable id so we can read last-frame hover and paint fill *under* content.
    let row_id = ui.id().with("conv_row").with(&buf.name);
    let hovered = ui
        .ctx()
        .read_response(row_id)
        .is_some_and(|r| r.hovered());

    let fill = if selected {
        p.accent.gamma_multiply(0.22)
    } else if hovered {
        p.button_hover.gamma_multiply(0.45)
    } else {
        Color32::TRANSPARENT
    };

    // List rows share one horizontal band with dividers — full width, modest radius.
    let inner = egui::Frame::new()
        .fill(fill)
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8 + 2))
        .show(ui, |ui| {
            // Force full available width so hover/selected match divider ends.
            let w = ui.available_width();
            ui.set_min_width(w);
            ui.set_max_width(w);
            ui.horizontal(|ui| {
                avatar_circle(ui, th, display_name, 40.0);
                ui.add_space(sp.md);
                ui.vertical(|ui| {
                    ui.set_min_width((ui.available_width()).max(40.0));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(display_name)
                                .size(th.type_scale.body)
                                .color(p.text)
                                .strong(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if buf.unread > 0 {
                                badge(ui, th, &format!("{}", buf.unread.min(99)));
                            } else if buf.call.is_some() {
                                ui.label(
                                    RichText::new("📞")
                                        .size(th.type_scale.caption)
                                        .color(p.accent),
                                );
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
        });

    ui.interact(inner.response.rect, row_id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
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

/// Message bubble / row in chat detail (with optional image / OG link embed).
pub fn message_bubble(
    ui: &mut egui::Ui,
    th: &Theme,
    msg: &ChatMessage,
    own_nick: &str,
    media: &mut MediaCache,
) {
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

    let embed = msg.resolved_embed();

    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
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

            if let Some(embed) = embed {
                ui.add_space(sp.sm);
                match embed {
                    Embed::Image { url } => {
                        inline_image_preview(ui, th, media, &url);
                    }
                    Embed::Link { url } => {
                        let seed = msg.link_meta.clone();
                        og_link_preview(ui, th, media, &url, seed.as_ref());
                    }
                }
            }
        });
}

/// Max width for inline image / link cards inside a bubble.
const EMBED_MAX_W: f32 = 280.0;
const EMBED_MAX_H: f32 = 220.0;

fn inline_image_preview(ui: &mut egui::Ui, th: &Theme, media: &mut MediaCache, url: &str) {
    let p = &th.palette;
    let sp = &th.spacing;
    media.touch_image(url);

    // Snapshot state so we can mutably load textures without holding map borrows.
    let ready = match media.images.get_mut(url) {
        Some(ImageState::Ready(pixels)) => {
            let tex = pixels.texture(ui.ctx(), url).clone();
            Some((tex, pixels.width, pixels.height))
        }
        Some(ImageState::Loading) => None,
        Some(ImageState::Failed) | None => {
            // Failed: show the URL as a muted fallback (still tappable).
            link_fallback_row(ui, th, url, "Couldn't load image");
            return;
        }
    };

    match ready {
        None => {
            // Loading placeholder.
            egui::Frame::new()
                .fill(p.headerbar_bg)
                .corner_radius(sp.radius_sm)
                .inner_margin(egui::Margin::symmetric(12, 16))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(sp.sm);
                        dim_label(ui, th, "Loading image…");
                    });
                });
        }
        Some((tex, width, height)) => {
            let max_w = ui.available_width().min(EMBED_MAX_W).max(80.0);
            let scale = (max_w / width.max(1) as f32)
                .min(EMBED_MAX_H / height.max(1) as f32)
                .min(1.0);
            let size = Vec2::new(
                (width as f32 * scale).max(1.0),
                (height as f32 * scale).max(1.0),
            );
            let resp = ui
                .add(
                    egui::Image::new((tex.id(), size))
                        .corner_radius(sp.radius_sm)
                        .sense(Sense::click()),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(url);
            if resp.clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
    }
}

fn og_link_preview(
    ui: &mut egui::Ui,
    th: &Theme,
    media: &mut MediaCache,
    url: &str,
    seeded: Option<&LinkMeta>,
) {
    let p = &th.palette;
    let sp = &th.spacing;

    // Prefer seeded IRCv3 meta; otherwise fetch.
    if let Some(meta) = seeded {
        media.seed_link(url, meta.clone());
    } else {
        media.touch_link(url);
    }

    let (title, description, thumb_url, site_name, loading) = match media.links.get(url) {
        Some(LinkState::Ready(m)) => (
            m.title.clone(),
            m.description.clone(),
            m.thumb_url.clone(),
            m.site_name.clone(),
            false,
        ),
        Some(LinkState::Loading) => (None, None, None, None, true),
        Some(LinkState::Failed) | None => (None, None, None, None, false),
    };

    let host = preview::display_host(url);
    let path = preview::display_path(url);
    let card_w = ui.available_width().min(300.0).max(120.0);

    let frame_resp = egui::Frame::new()
        .fill(p.headerbar_bg)
        .stroke(Stroke::new(1.0_f32, p.border_soft))
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(sp.sm as i8 + 2, sp.sm as i8 + 2))
        .show(ui, |ui| {
            ui.set_width(card_w);
            ui.horizontal(|ui| {
                // Thumbnail (if we have one and it loaded).
                if let Some(ref thumb) = thumb_url {
                    media.touch_image(thumb);
                    if let Some(ImageState::Ready(pixels)) = media.images.get_mut(thumb.as_str()) {
                        let tex = pixels.texture(ui.ctx(), thumb).clone();
                        const TH: f32 = 56.0;
                        let scale = (TH / pixels.width.max(1) as f32)
                            .min(TH / pixels.height.max(1) as f32)
                            .min(1.0);
                        let size = Vec2::new(
                            (pixels.width as f32 * scale).max(1.0),
                            (pixels.height as f32 * scale).max(1.0),
                        );
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(TH), Sense::hover());
                        ui.painter()
                            .rect_filled(rect, sp.radius_sm, p.card_bg);
                        let img_rect = egui::Rect::from_center_size(rect.center(), size);
                        ui.put(
                            img_rect,
                            egui::Image::new((tex.id(), size)).corner_radius(sp.radius_sm),
                        );
                        ui.add_space(sp.sm);
                    } else if matches!(
                        media.images.get(thumb.as_str()),
                        Some(ImageState::Loading)
                    ) {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                        ui.painter()
                            .rect_filled(rect, sp.radius_sm, p.card_bg);
                        ui.put(rect, egui::Spinner::new());
                        ui.add_space(sp.sm);
                    }
                } else {
                    // Link glyph stand-in.
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        sp.radius_sm,
                        p.accent.gamma_multiply(0.18),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "🔗",
                        egui::FontId::proportional(14.0),
                        p.accent,
                    );
                    ui.add_space(sp.sm);
                }

                ui.vertical(|ui| {
                    ui.set_max_width((card_w - 72.0).max(80.0));
                    if loading {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(4.0);
                            dim_label(ui, th, "Fetching preview…");
                        });
                    }
                    // Site name above title (freeq-app parity).
                    if let Some(site) = site_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        ui.label(
                            RichText::new(site.to_uppercase())
                                .size((th.type_scale.caption - 1.0).max(9.0))
                                .color(p.text_secondary),
                        );
                    }
                    let headline = title
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or(host.as_str());
                    ui.label(
                        RichText::new(headline)
                            .size(th.type_scale.body)
                            .color(p.text)
                            .strong(),
                    );
                    if let Some(desc) = description
                        .as_deref()
                        .filter(|d| !d.trim().is_empty())
                    {
                        let short = if desc.chars().count() > 160 {
                            format!("{}…", desc.chars().take(160).collect::<String>())
                        } else {
                            desc.to_string()
                        };
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(short)
                                .size(th.type_scale.caption)
                                .color(p.text_secondary),
                        );
                    }
                    ui.add_space(2.0);
                    let foot = if path.is_empty() {
                        host.clone()
                    } else if title.is_some() {
                        format!("{host}{path}")
                    } else {
                        path
                    };
                    ui.label(
                        RichText::new(foot)
                            .size(th.type_scale.caption)
                            .color(p.accent),
                    );
                });
            });
        });

    let resp = ui.interact(
        frame_resp.response.rect,
        ui.id().with("og").with(url),
        Sense::click(),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

fn link_fallback_row(ui: &mut egui::Ui, th: &Theme, url: &str, note: &str) {
    let p = &th.palette;
    ui.horizontal(|ui| {
        dim_label(ui, th, note);
        ui.add_space(th.spacing.xs);
        let resp = ui
            .add(
                egui::Label::new(
                    RichText::new(preview::display_host(url))
                        .size(th.type_scale.caption)
                        .color(p.accent),
                )
                .sense(Sense::click()),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text(url);
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    });
}

pub fn empty_state(ui: &mut egui::Ui, th: &Theme, title: &str, blurb: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        title_2(ui, th, title);
        ui.add_space(th.spacing.sm);
        // Allow multi-line server reasons (wrap within the column).
        ui.set_max_width((ui.available_width() - 24.0).max(120.0));
        dim_label(ui, th, blurb);
    });
}
