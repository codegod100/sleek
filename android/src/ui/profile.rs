//! Peer profile modal (freeq `UserPopover` parity — nick tap → Bluesky).

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, RichText, ScrollArea, Sense, Stroke};
use vidya::{button, dim_label, primary_button, Theme};

use crate::bsky::{self, ActorProfile};
use crate::state::AppState;
use crate::ui::widgets::user_avatar;

pub enum ProfileGateAction {
    None,
    Close,
    Message { nick: String },
    OpenBluesky { url: String },
}

/// Centered modal with peer nick, DID, Bluesky profile, and actions.
pub fn profile_gate_overlay(
    ctx: &egui::Context,
    th: &Theme,
    state: &mut AppState,
) -> ProfileGateAction {
    let Some(nick) = state.profile_gate.nick.clone() else {
        return ProfileGateAction::None;
    };

    let p = &th.palette;
    let sp = &th.spacing;
    let frame = egui::Frame::new()
        .fill(p.card_bg)
        .stroke(Stroke::new(1.0, p.border_soft))
        .corner_radius(sp.radius_md)
        .inner_margin(egui::Margin::symmetric(16, 14))
        .shadow(egui::epaint::Shadow {
            offset: [0, 4],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(80),
        });

    let mut action = ProfileGateAction::None;
    let mut close = false;
    let is_self = nick.eq_ignore_ascii_case(&state.nick);
    let did = state.profile_gate.did.clone();
    let loading = state.profile_gate.loading;
    let error = state.profile_gate.error.clone();
    let profile = state.profile_gate.profile.clone();
    let whois = state.profile_gate.whois_lines.clone();
    let whois_sent = state.profile_gate.whois_sent;
    let whois_exhausted = state.profile_gate.whois_exhausted();
    let avatar_url = profile.as_ref().and_then(|pr| pr.avatar.clone());

    let modal = egui::Modal::new(egui::Id::new("profile_gate_modal"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(frame)
        .show(ctx, |ui| {
            let panel_w = (ctx.screen_rect().width() - 32.0).clamp(280.0, 380.0);
            ui.set_width(panel_w);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Profile")
                        .size(th.type_scale.title)
                        .color(p.text)
                        .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close_resp = ui
                        .add(
                            egui::Button::new(
                                RichText::new("✕")
                                    .size(th.type_scale.body)
                                    .color(p.text_secondary),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .frame(false),
                        )
                        .on_hover_cursor(CursorIcon::PointingHand);
                    if close_resp.clicked() {
                        close = true;
                    }
                });
            });

            ui.add_space(sp.md);
            ui.separator();
            ui.add_space(sp.sm);

            // Sticky identity header (avatar + name/handle); scroll the rest.
            ui.horizontal(|ui| {
                user_avatar(
                    ui,
                    th,
                    &mut state.media,
                    &nick,
                    avatar_url.as_deref(),
                    56.0,
                );
                ui.add_space(sp.md);
                ui.vertical(|ui| {
                    let display = profile
                        .as_ref()
                        .and_then(|pr| pr.display_name.as_deref())
                        .unwrap_or(nick.as_str());
                    ui.label(
                        RichText::new(display)
                            .size(th.type_scale.body)
                            .color(p.text)
                            .strong(),
                    );
                    if display != nick {
                        dim_label(ui, th, &nick);
                    }
                    if let Some(pr) = profile.as_ref() {
                        if !pr.handle.is_empty() {
                            ui.label(
                                RichText::new(format!("@{}", pr.handle))
                                    .size(th.type_scale.caption)
                                    .color(p.accent),
                            );
                        }
                    }
                });
            });

            let body_h = (ctx.screen_rect().height() * 0.5).clamp(180.0, 420.0);
            ScrollArea::vertical()
                .id_salt("profile_gate_body")
                .max_height(body_h)
                .show(ui, |ui| {
                    if let Some(did) = did.as_ref() {
                        ui.add_space(sp.sm);
                        let did_resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(did)
                                        .size(th.type_scale.caption)
                                        .color(p.text_secondary)
                                        .monospace(),
                                )
                                .sense(Sense::click()),
                            )
                            .on_hover_text("Copy DID")
                            .on_hover_cursor(CursorIcon::PointingHand);
                        if did_resp.clicked() {
                            ui.ctx().copy_text(did.clone());
                            state.show_toast("DID copied");
                        }
                    }

                    if let Some(pr) = profile.as_ref() {
                        if let Some(desc) = pr.description.as_ref() {
                            ui.add_space(sp.sm);
                            ui.label(
                                RichText::new(truncate_bio(desc, 280))
                                    .size(th.type_scale.caption)
                                    .color(p.text_secondary),
                            );
                        }
                        if pr.followers_count.is_some()
                            || pr.follows_count.is_some()
                            || pr.posts_count.is_some()
                        {
                            ui.add_space(sp.sm);
                            dim_label(ui, th, &format_stats(pr));
                        }
                    }

                    if let Some(err) = error.as_ref() {
                        ui.add_space(sp.sm);
                        ui.label(
                            RichText::new(err)
                                .size(th.type_scale.caption)
                                .color(p.destructive),
                        );
                    }

                    let whois_done = whois_exhausted || !whois.is_empty();
                    let looking_up = !loading
                        && profile.is_none()
                        && did.is_none()
                        && !whois_done
                        && whois_sent;
                    let is_guest = !loading
                        && profile.is_none()
                        && (did.as_ref().is_some_and(|d| d.starts_with("did:key:"))
                            || (did.is_none() && whois_done));

                    if loading {
                        ui.add_space(sp.md);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(sp.sm);
                            dim_label(ui, th, "Loading Bluesky profile…");
                        });
                    } else if looking_up {
                        ui.add_space(sp.md);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(sp.sm);
                            dim_label(ui, th, "Looking up identity…");
                        });
                    } else if is_guest {
                        ui.add_space(sp.sm);
                        if whois_exhausted {
                            dim_label(
                                ui,
                                th,
                                "Not on the server right now — no Bluesky profile found",
                            );
                        } else {
                            dim_label(ui, th, "Guest — no Bluesky / AT Protocol identity");
                        }
                    }

                    // Skip raw "No such nick" noise when we already have a profile
                    // (common for domain-handle nicks who have quit).
                    let whois_show: Vec<&String> = whois
                        .iter()
                        .filter(|l| {
                            if profile.is_some() {
                                !l.to_ascii_lowercase().contains("no such nick")
                            } else {
                                true
                            }
                        })
                        .collect();
                    if !whois_show.is_empty() {
                        ui.add_space(sp.md);
                        ui.label(
                            RichText::new("WHOIS")
                                .size(th.type_scale.caption)
                                .color(p.text)
                                .strong(),
                        );
                        ui.add_space(sp.xs);
                        for line in whois_show {
                            dim_label(ui, th, line);
                        }
                    }
                });

            ui.add_space(sp.sm);
            ui.separator();
            ui.add_space(sp.sm);

            ui.horizontal(|ui| {
                if !is_self && button(ui, th, "Message").clicked() {
                    action = ProfileGateAction::Message { nick: nick.clone() };
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(pr) = profile.as_ref() {
                        if !pr.handle.is_empty()
                            && primary_button(ui, th, "Bluesky ↗").clicked()
                        {
                            action = ProfileGateAction::OpenBluesky {
                                url: bsky::bluesky_profile_url(&pr.handle),
                            };
                        }
                    }
                    if button(ui, th, "Close").clicked() {
                        close = true;
                    }
                });
            });
        });

    if close || modal.should_close() {
        action = ProfileGateAction::Close;
    }

    action
}

fn truncate_bio(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    let first = trimmed.lines().next().unwrap_or(trimmed);
    if first.chars().count() <= max {
        first.to_string()
    } else {
        let cut: String = first.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn format_stats(pr: &ActorProfile) -> String {
    let mut parts = Vec::new();
    if let Some(n) = pr.followers_count {
        parts.push(format!("{n} followers"));
    }
    if let Some(n) = pr.follows_count {
        parts.push(format!("{n} following"));
    }
    if let Some(n) = pr.posts_count {
        parts.push(format!("{n} posts"));
    }
    parts.join(" · ")
}

/// Open a Bluesky (or other) URL in the system browser.
pub fn open_profile_url(ctx: &egui::Context, url: &str) {
    #[cfg(target_os = "android")]
    {
        if crate::android_media::open_url(url).is_err() {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        if open::that(url).is_err() {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_bio_keeps_short() {
        assert_eq!(truncate_bio("hello", 10), "hello");
    }

    #[test]
    fn truncate_bio_cuts_long() {
        let s = "a".repeat(20);
        let out = truncate_bio(&s, 10);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 10);
    }
}
