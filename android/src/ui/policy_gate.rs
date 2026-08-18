//! Channel policy join-gate modal (freeq `JoinGateModal` parity).

use eframe::egui::{self, Align, Color32, CursorIcon, Layout, RichText, ScrollArea, Stroke};
use vidya::{button, dim_label, primary_button, Theme};

use crate::policy::{friendly_requirement, verification_url, PolicyCheck, RequirementStatus};
use crate::state::AppState;

pub enum PolicyGateAction {
    None,
    Close,
    Refresh,
    AcceptRules,
    VerifyExternal { url: String },
    Join,
}

/// Centered modal for accepting channel policy before JOIN.
pub fn policy_gate_overlay(
    ctx: &egui::Context,
    th: &Theme,
    state: &mut AppState,
) -> PolicyGateAction {
    let Some(channel) = state.policy_gate.channel.clone() else {
        return PolicyGateAction::None;
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

    let mut action = PolicyGateAction::None;
    let mut close = false;

    let modal = egui::Modal::new(egui::Id::new("policy_gate_modal"))
        .backdrop_color(Color32::from_black_alpha(140))
        .frame(frame)
        .show(ctx, |ui| {
            let panel_w = (ctx.screen_rect().width() - 32.0).clamp(280.0, 420.0);
            ui.set_width(panel_w);

            // Header
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "#{}",
                            channel.trim_start_matches('#')
                        ))
                        .size(th.type_scale.title)
                        .color(p.text)
                        .strong(),
                    );
                    dim_label(
                        ui,
                        th,
                        "This channel requires policy acceptance to join",
                    );
                });
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

            let body_h = (ctx.screen_rect().height() * 0.45).clamp(160.0, 360.0);
            ScrollArea::vertical()
                .id_salt("policy_gate_body")
                .max_height(body_h)
                .show(ui, |ui| {
                    if state.policy_gate.loading {
                        ui.vertical_centered(|ui| {
                            ui.add_space(sp.lg);
                            ui.spinner();
                            ui.add_space(sp.sm);
                            dim_label(ui, th, "Checking requirements…");
                        });
                        return;
                    }

                    if let Some(err) = state.policy_gate.error.as_ref() {
                        ui.label(
                            RichText::new(err)
                                .size(th.type_scale.body)
                                .color(p.destructive),
                        );
                        ui.add_space(sp.sm);
                    }

                    if let Some(rules) = state.policy_gate.rules.as_ref() {
                        if !rules.trim().is_empty() {
                            ui.label(
                                RichText::new("Channel rules")
                                    .size(th.type_scale.body)
                                    .color(p.text)
                                    .strong(),
                            );
                            ui.add_space(sp.xs);
                            ui.label(
                                RichText::new(rules.trim())
                                    .size(th.type_scale.caption)
                                    .color(p.text_secondary),
                            );
                            ui.add_space(sp.md);
                        }
                    }

                    if let Some(check) = state.policy_gate.check.as_ref() {
                        render_check(ui, th, check, state.policy_gate.accepting, &mut action);
                    }
                });

            ui.add_space(sp.sm);
            ui.separator();
            ui.add_space(sp.sm);

            // Footer
            ui.horizontal(|ui| {
                if button(ui, th, "Cancel").clicked() {
                    close = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can_join = state
                        .policy_gate
                        .check
                        .as_ref()
                        .is_some_and(|c| c.can_join);
                    let joining = state.policy_gate.joining;
                    let join_label = if joining {
                        "Joining…"
                    } else {
                        "Join channel"
                    };
                    if primary_button(ui, th, join_label).clicked() && can_join && !joining {
                        action = PolicyGateAction::Join;
                    }
                    ui.add_space(sp.xs);
                    if button(ui, th, "Refresh").clicked() {
                        action = PolicyGateAction::Refresh;
                    }
                });
            });
        });

    if close || modal.should_close() {
        action = PolicyGateAction::Close;
    }

    action
}

fn render_check(
    ui: &mut egui::Ui,
    th: &Theme,
    check: &PolicyCheck,
    accepting: bool,
    action: &mut PolicyGateAction,
) {
    let p = &th.palette;
    let sp = &th.spacing;

    match check.status.as_str() {
        "no_policy" => {
            dim_label(ui, th, "This channel has no policy. You should be able to join directly.");
            return;
        }
        "satisfied" if check.requirements.is_empty() => {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("✅").size(28.0));
                ui.label(
                    RichText::new("All requirements satisfied")
                        .size(th.type_scale.body)
                        .color(p.text)
                        .strong(),
                );
                if let Some(role) = &check.role {
                    dim_label(ui, th, &format!("Role: {role}"));
                }
            });
            return;
        }
        _ => {}
    }

    if !check.requirements.is_empty() {
        for req in &check.requirements {
            requirement_row(ui, th, req, accepting, action);
            ui.add_space(sp.xs);
        }
    }

    if check.can_join {
        if let Some(role) = &check.role {
            ui.add_space(sp.sm);
            ui.label(
                RichText::new(format!("✓ All requirements met — role: {role}"))
                    .size(th.type_scale.caption)
                    .color(p.success),
            );
        }
    }
}

fn requirement_row(
    ui: &mut egui::Ui,
    th: &Theme,
    req: &RequirementStatus,
    accepting: bool,
    action: &mut PolicyGateAction,
) {
    let p = &th.palette;
    let sp = &th.spacing;
    let (title, subtitle) = friendly_requirement(req);
    let border = if req.satisfied {
        p.success
    } else {
        p.border_soft
    };
    let bg = if req.satisfied {
        Color32::from_rgba_unmultiplied(p.success.r(), p.success.g(), p.success.b(), 20)
    } else {
        p.view_bg
    };

    egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(sp.radius_sm)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let icon = if req.satisfied { "✓" } else { "○" };
                let icon_color = if req.satisfied {
                    p.success
                } else {
                    p.text_secondary
                };
                ui.label(
                    RichText::new(icon)
                        .size(th.type_scale.body)
                        .color(icon_color)
                        .strong(),
                );
                ui.add_space(sp.xs);
                ui.vertical(|ui| {
                    let title_color = if req.satisfied {
                        p.text_secondary
                    } else {
                        p.text
                    };
                    ui.label(
                        RichText::new(title)
                            .size(th.type_scale.body)
                            .color(title_color)
                            .strong(),
                    );
                    if !subtitle.is_empty() {
                        dim_label(ui, th, &subtitle);
                    }
                    if !req.satisfied {
                        if let Some(act) = &req.action {
                            ui.add_space(sp.xs);
                            match act.action_type.as_str() {
                                "accept_rules" if act.accept_hash.is_some() => {
                                    let label = if accepting {
                                        "Accepting…"
                                    } else {
                                        act.label.as_str()
                                    };
                                    if primary_button(ui, th, label).clicked() && !accepting {
                                        *action = PolicyGateAction::AcceptRules;
                                    }
                                }
                                "verify_external" if act.url.is_some() => {
                                    let label = act.label.clone();
                                    if button(ui, th, &label).clicked() {
                                        *action = PolicyGateAction::VerifyExternal {
                                            url: act.url.clone().unwrap_or_default(),
                                        };
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                });
            });
        });
}

/// Resolve a verification URL and open it in the system browser.
pub fn open_verification_url(ctx: &egui::Context, api_base: &str, raw: &str, subject_did: &str) {
    let url = verification_url(raw, api_base, subject_did);
    #[cfg(target_os = "android")]
    {
        if crate::android_media::open_url(&url).is_err() {
            ctx.open_url(egui::OpenUrl::new_tab(&url));
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        if open::that(&url).is_err() {
            ctx.open_url(egui::OpenUrl::new_tab(&url));
        }
    }
}
