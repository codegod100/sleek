//! Vidya aesthetic showcase app (desktop + Android / Waydroid).
//!
//! Host: `cargo run -p vidya-demo`
//! Android APK: `cargo apk build -p vidya-demo --target x86_64-linux-android`

use std::path::PathBuf;

use eframe::egui::{
    self, Align, Color32, CursorIcon, Event, Layout, RichText, ScrollArea, Sense, Stroke, Vec2,
    ViewportCommand,
};
use vidya::{
    apply, body, button, card, checkbox, destructive_button, dim_label, emoji_icon, emoji_pack_len,
    grid_cols, hflow, icon, primary_button, reserve_system_chrome, text_field_multiline, title,
    title_2, two_col, ColSpec, Icon, Mode, Theme,
};

/// Desktop / host entry.
pub fn run_desktop() -> eframe::Result {
    let cli = Cli::from_env_and_args();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 680.0])
            .with_min_inner_size([360.0, 640.0])
            .with_title("Vidya — Aesthetic Showcase"),
        ..Default::default()
    };
    eframe::run_native(
        "Vidya Showcase",
        options,
        Box::new(move |cc| Ok(Box::new(DemoApp::new(cc, cli)))),
    )
}

/// Android NativeActivity entry (Waydroid / phone).
#[cfg(target_os = "android")]
pub fn run_android(android_app: winit::platform::android::activity::AndroidApp) -> eframe::Result {
    let cli = Cli::from_env_and_args();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Vidya"),
        ..Default::default()
    };
    options.android_app = Some(android_app);
    eframe::run_native(
        "Vidya Showcase",
        options,
        Box::new(move |cc| Ok(Box::new(DemoApp::new(cc, cli)))),
    )
}

#[derive(Clone)]
struct Cli {
    section: Nav,
    mode: Mode,
    /// When set, render a few frames, capture, write PNG, exit.
    screenshot: Option<PathBuf>,
}

impl Cli {
    fn from_env_and_args() -> Self {
        let mut section = std::env::var("VIDYA_DEMO_SECTION")
            .ok()
            .and_then(|s| Nav::parse(&s))
            .unwrap_or(Nav::Overview);
        let mut mode = std::env::var("VIDYA_DEMO_MODE")
            .ok()
            .and_then(|s| parse_mode(&s))
            .unwrap_or(Mode::Dark);
        let mut screenshot = std::env::var_os("VIDYA_SCREENSHOT_TO").map(PathBuf::from);

        // Waydroid / adb: write the section name before launch, e.g.
        //   adb shell 'echo forms >/data/local/tmp/vidya_section'
        for path in [
            "/data/local/tmp/vidya_section",
            "/sdcard/vidya_section",
            "/data/local/tmp/vidya_mode",
        ] {
            if path.ends_with("vidya_mode") {
                if let Ok(s) = std::fs::read_to_string(path) {
                    if let Some(m) = parse_mode(s.trim()) {
                        mode = m;
                    }
                }
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(path) {
                if let Some(n) = Nav::parse(s.trim()) {
                    section = n;
                }
            }
        }

        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            match a.as_str() {
                "--section" | "-s" => {
                    if let Some(v) = args.next() {
                        if let Some(n) = Nav::parse(&v) {
                            section = n;
                        }
                    }
                }
                "--mode" | "-m" => {
                    if let Some(v) = args.next() {
                        if let Some(m) = parse_mode(&v) {
                            mode = m;
                        }
                    }
                }
                "--screenshot" => {
                    if let Some(v) = args.next() {
                        screenshot = Some(PathBuf::from(v));
                    }
                }
                _ => {}
            }
        }
        Self {
            section,
            mode,
            screenshot,
        }
    }
}

fn parse_mode(s: &str) -> Option<Mode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "dark" | "d" => Some(Mode::Dark),
        "light" | "l" => Some(Mode::Light),
        _ => None,
    }
}

struct DemoApp {
    mode: Mode,
    name: String,
    notes: String,
    enable_sync: bool,
    volume: f32,
    selected_nav: Nav,
    toast: Option<String>,
    click_count: u32,
    screenshot: Option<PathBuf>,
    frame_count: u32,
    screenshot_requested: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    Overview,
    Typography,
    Actions,
    Surfaces,
    Palette,
    Forms,
}

impl Nav {
    const ALL: [Nav; 6] = [
        Nav::Overview,
        Nav::Typography,
        Nav::Actions,
        Nav::Surfaces,
        Nav::Palette,
        Nav::Forms,
    ];

    fn label(self) -> &'static str {
        match self {
            Nav::Overview => "Overview",
            Nav::Typography => "Typography",
            Nav::Actions => "Actions",
            Nav::Surfaces => "Surfaces",
            Nav::Palette => "Palette",
            Nav::Forms => "Forms",
        }
    }

    /// Compact labels for phone bottom chips.
    /// Avoid short words that corrupt in the Android glow font atlas ("Type", "Surf"
    /// were painting as white triangle / circle glyphs).
    fn short_label(self) -> &'static str {
        match self {
            Nav::Overview => "Home",
            Nav::Typography => "Text",
            Nav::Actions => "Acts",
            Nav::Surfaces => "Card",
            Nav::Palette => "Pal",
            Nav::Forms => "Form",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "overview" | "home" => Some(Nav::Overview),
            "typography" | "type" | "text" => Some(Nav::Typography),
            "actions" | "buttons" => Some(Nav::Actions),
            "surfaces" | "cards" => Some(Nav::Surfaces),
            "palette" | "colors" => Some(Nav::Palette),
            "forms" | "form" => Some(Nav::Forms),
            _ => None,
        }
    }
}

impl DemoApp {
    fn new(cc: &eframe::CreationContext<'_>, cli: Cli) -> Self {
        let theme = match cli.mode {
            Mode::Dark => Theme::dark(),
            Mode::Light => Theme::light(),
        };
        apply(&cc.egui_ctx, &theme);
        Self {
            mode: cli.mode,
            name: String::from("Ada"),
            notes: String::from("A short note about the theme layer."),
            enable_sync: true,
            volume: 0.62,
            selected_nav: cli.section,
            toast: None,
            click_count: 0,
            screenshot: cli.screenshot,
            frame_count: 0,
            screenshot_requested: false,
        }
    }

    fn theme(&self) -> Theme {
        match self.mode {
            Mode::Dark => Theme::dark(),
            Mode::Light => Theme::light(),
        }
    }

    fn set_mode(&mut self, ctx: &egui::Context, mode: Mode) {
        self.mode = mode;
        apply(ctx, &self.theme());
    }
}

impl eframe::App for DemoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Docs capture: wait for fonts/layout to settle, then grab framebuffer.
        if self.screenshot.is_some() {
            self.frame_count = self.frame_count.saturating_add(1);
            if !self.screenshot_requested && self.frame_count >= 12 {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                self.screenshot_requested = true;
            }
            let events: Vec<Event> = ctx.input(|i| i.raw.events.clone());
            for ev in events {
                if let Event::Screenshot { image, .. } = ev {
                    if let Some(path) = self.screenshot.take() {
                        save_color_image_png(&image, &path);
                        eprintln!("wrote screenshot {}", path.display());
                        std::process::exit(0);
                    }
                }
            }
            ctx.request_repaint();
        }

        let th = self.theme();
        let p = &th.palette;
        let sp = &th.spacing;
        let screen = ctx.screen_rect();
        // Portrait phone / Waydroid: bottom chips. Landscape / desktop: side rail.
        let phone = screen.height() >= screen.width() || screen.width() < 640.0;

        // System status / nav bars (edge-to-edge Android) — library owns this so
        // header and chips cannot sit under the clock or gesture bar.
        reserve_system_chrome(ctx, &th);

        // ── Headerbar ──────────────────────────────────────────────
        let header_frame = th.header_frame().inner_margin(egui::Margin {
            left: sp.page as i8,
            right: sp.page as i8,
            top: (sp.md + 4.0) as i8,
            bottom: (sp.md + 2.0) as i8,
        });
        egui::TopBottomPanel::top("header")
            .frame(header_frame)
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        title(ui, &th, "Vidya");
                        ui.add_space(sp.xs + 2.0); // gap between title and subtitle
                        // Shorter blurb on narrow screens
                        let blurb = if screen.width() < 480.0 {
                            "HIG-inspired egui theme"
                        } else {
                            "GNOME/HIG-inspired theme layer for egui"
                        };
                        dim_label(ui, &th, blurb);
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let is_dark = self.mode == Mode::Dark;
                        let label = if is_dark { " Light " } else { " Dark " };
                        if button(ui, &th, label).clicked() {
                            let next = if is_dark { Mode::Light } else { Mode::Dark };
                            self.set_mode(ctx, next);
                        }

                        if screen.width() >= 480.0 {
                            ui.add_space(sp.sm);
                            dim_label(ui, &th, "shell");
                        }
                    });
                });
            });

        if phone {
            // ── App section chips (above system nav) ────────────────
            egui::TopBottomPanel::bottom("nav_bottom")
                .frame(
                    egui::Frame::new()
                        .fill(p.headerbar_bg)
                        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8))
                        .stroke(Stroke::new(1.0_f32, p.border_soft)),
                )
                .show_separator_line(false)
                .show(ctx, |ui| {
                    ScrollArea::horizontal()
                        .id_salt("nav_chips")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = sp.sm;
                            ui.horizontal(|ui| {
                                for nav in Nav::ALL {
                                    let selected = self.selected_nav == nav;
                                    let fill = if selected {
                                        p.accent
                                    } else {
                                        p.button_bg
                                    };
                                    let fg = if selected {
                                        p.accent_fg
                                    } else {
                                        p.button_fg
                                    };
                                    let label = RichText::new(nav.short_label())
                                        .size(th.type_scale.caption)
                                        .family(egui::FontFamily::Proportional)
                                        .color(fg);
                                    let btn = egui::Button::new(label)
                                        .fill(fill)
                                        .stroke(if selected {
                                            Stroke::NONE
                                        } else {
                                            Stroke::new(1.0_f32, p.border_soft)
                                        })
                                        .corner_radius(sp.radius_md)
                                        .min_size(Vec2::new(52.0, sp.control_height));
                                    if ui.add(btn).clicked() {
                                        self.selected_nav = nav;
                                    }
                                }
                            });
                        });
                });
        }

        // ── Toast snackbar (above chips on phone; bottom edge on desktop) ──
        if let Some(msg) = self.toast.clone() {
            egui::TopBottomPanel::bottom("toast")
                .frame(
                    egui::Frame::new()
                        .fill(p.accent.gamma_multiply(0.22))
                        .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8))
                        .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.55))),
                )
                .show_separator_line(false)
                .show(ctx, |ui| {
                    if phone {
                        // Stack message + dismiss so nothing fights for one narrow row.
                        ui.vertical(|ui| {
                            ui.set_width(ui.available_width());
                            body(ui, &th, &msg);
                            ui.add_space(sp.xs);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if button(ui, &th, "Dismiss").clicked() {
                                    self.toast = None;
                                }
                            });
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.set_max_width(ui.available_width() - 100.0);
                            body(ui, &th, &msg);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if button(ui, &th, "Dismiss").clicked() {
                                    self.toast = None;
                                }
                            });
                        });
                    }
                });
        }

        if !phone {
            // ── Side nav (desktop) ────────────────────────────────
            let nav_w = if screen.width() < 900.0 { 148.0 } else { 180.0 };
            egui::SidePanel::left("nav")
                .resizable(false)
                .exact_width(nav_w)
                .frame(
                    egui::Frame::new()
                        .fill(p.view_bg)
                        .inner_margin(egui::Margin::same(sp.md as i8))
                        .stroke(Stroke::new(1.0, p.border_soft)),
                )
                .show(ctx, |ui| {
                    dim_label(ui, &th, "SECTIONS");
                    ui.add_space(sp.sm);

                    for nav in Nav::ALL {
                        let selected = self.selected_nav == nav;
                        let fill = if selected {
                            p.accent.gamma_multiply(0.25)
                        } else {
                            Color32::TRANSPARENT
                        };
                        let text_color = if selected { p.accent } else { p.text };

                        let resp = egui::Frame::new()
                            .fill(fill)
                            .corner_radius(sp.radius_md)
                            .inner_margin(egui::Margin::symmetric(sp.md as i8, sp.sm as i8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(nav.label())
                                            .size(th.type_scale.body)
                                            .color(text_color)
                                            .strong_if(selected),
                                    )
                                    .wrap(),
                                );
                            })
                            .response
                            .interact(Sense::click())
                            .on_hover_cursor(CursorIcon::PointingHand);

                        if resp.hovered() && !selected {
                            ui.painter().rect_filled(
                                resp.rect,
                                sp.radius_md,
                                p.button_hover.gamma_multiply(0.5),
                            );
                        }
                        if resp.clicked() {
                            self.selected_nav = nav;
                        }
                        ui.add_space(sp.xs);
                    }

                    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                        dim_label(ui, &th, "v0.1 · MIT");
                        ui.add_space(sp.sm);
                        dim_label(ui, &th, "nandi.uk/vidya");
                    });
                });
        }

        // ── Main content ───────────────────────────────────────────
        // Tighter side inset on phone so cards + stroke stay on-screen.
        let page = if phone {
            egui::Frame::new()
                .fill(p.window_bg)
                .inner_margin(egui::Margin::symmetric(10_i8, sp.page as i8))
        } else {
            th.page_frame()
        };
        egui::CentralPanel::default()
            .frame(page)
            .show(ctx, |ui| {
                let panel_h = ui.available_height();
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(panel_h)
                    .id_salt("main_scroll")
                    .show(ui, |ui| {
                        // Column = viewport content width (page margin already applied
                        // by the panel frame). Cap so we never paint past the edge.
                        let col_w = if phone {
                            ui.available_width()
                        } else {
                            ui.available_width().min(760.0)
                        };
                        ui.set_max_width(col_w);
                        ui.set_min_width(col_w);
                        match self.selected_nav {
                            Nav::Overview => self.ui_overview(ui, &th),
                            Nav::Typography => self.ui_typography(ui, &th),
                            Nav::Actions => self.ui_actions(ui, &th),
                            Nav::Surfaces => self.ui_surfaces(ui, &th),
                            Nav::Palette => self.ui_palette(ui, &th),
                            Nav::Forms => self.ui_forms(ui, &th),
                        }
                        if phone {
                            ui.add_space(sp.xl + sp.lg);
                        }
                    });
            });
    }
}

impl DemoApp {
    fn section_header(&self, ui: &mut egui::Ui, th: &Theme, heading: &str, blurb: &str) {
        title(ui, th, heading);
        ui.add_space(th.spacing.xs);
        dim_label(ui, th, blurb);
        ui.add_space(th.spacing.lg);
    }

    fn ui_overview(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.section_header(
            ui,
            th,
            "Overview",
            "A calm, high-contrast shell with rounded chrome, soft borders, and a clear accent.",
        );

        card(ui, th, |ui| {
            title_2(ui, th, "What is Vidya?");
            ui.add_space(th.spacing.sm);
            body(
                ui,
                th,
                "Vidya is a theme layer for egui — palette, spacing, type scale, and a handful of \
                 styled widgets. It tracks the feel of modern desktop HIG without linking GTK.",
            );
            ui.add_space(th.spacing.md);
            hflow(ui, th, |ui| {
                if primary_button(ui, th, "Get started").clicked() {
                    self.selected_nav = Nav::Typography;
                    self.toast = Some("Jump to Typography →".into());
                }
                if button(ui, th, "View palette").clicked() {
                    self.selected_nav = Nav::Palette;
                }
            });
        });

        ui.add_space(th.spacing.md);

        // Side-by-side when wide enough; each column card still fills its column.
        two_col(
            ui,
            th,
            280.0,
            |ui| {
                card(ui, th, |ui| {
                    title_2(ui, th, "Dark by default");
                    ui.add_space(th.spacing.sm);
                    body(
                        ui,
                        th,
                        "Window, header, cards, and popovers use a layered charcoal stack with a blue accent.",
                    );
                });
            },
            |ui| {
                card(ui, th, |ui| {
                    title_2(ui, th, "Light when you want it");
                    ui.add_space(th.spacing.sm);
                    body(
                        ui,
                        th,
                        "Flip the shell with Theme::light() or apply_light — same metrics, inverted palette.",
                    );
                });
            },
        );

        ui.add_space(th.spacing.md);

        card(ui, th, |ui| {
            title_2(ui, th, "Design tokens");
            ui.add_space(th.spacing.sm);
            dim_label(ui, th, "grid_cols + row DSL — striped key / value table");
            ui.add_space(th.spacing.md);
            let tokens = [
                ("spacing", "4 · 6 · 12 · 18 · 24"),
                ("radius", "6 · 9 · 12"),
                ("control", "34px height"),
                ("type", "20 / 16 / 14 / 12"),
            ];
            grid_cols(
                ui,
                th,
                "overview_tokens",
                &[ColSpec::Flex, ColSpec::Flex],
                |g| {
                    g.row(|r| {
                        r.heading("Token");
                        r.heading("Value");
                    });
                    for (key, value) in tokens {
                        g.row(|r| {
                            r.dim(key);
                            r.text(value);
                        });
                    }
                },
            );
        });

        ui.add_space(th.spacing.md);

        // Showcase the metric column specs (avoids “waterfall” rates).
        card(ui, th, |ui| {
            title_2(ui, th, "Grid DSL — metrics");
            ui.add_space(th.spacing.sm);
            dim_label(
                ui,
                th,
                "ColSpec::MetricBps / MetricRate keep throughput columns right-edge aligned.",
            );
            ui.add_space(th.spacing.md);
            let rows = [
                ("chrome", "/usr/bin/google-chrome", 12_582_912.0_f64, 48.2_f64),
                ("rustc", "/home/nandi/.cargo/bin/rustc", 3_145_728.0, 12.0),
                ("waydroid", "/usr/bin/waydroid", 524_288.0, 2.4),
                ("idle", "—", 0.0, 0.0),
            ];
            grid_cols(
                ui,
                th,
                "overview_metrics",
                &[
                    ColSpec::Flex,
                    ColSpec::Flex,
                    ColSpec::MetricBps,
                    ColSpec::MetricRate,
                ],
                |g| {
                    g.row(|r| {
                        r.heading("Name");
                        r.heading("Path");
                        r.heading("Write");
                        r.heading("Freq");
                    });
                    for (name, path, bps, rate) in rows {
                        g.row(|r| {
                            if bps > 8_000_000.0 {
                                r.warn(name);
                            } else {
                                r.text(name);
                            }
                            r.dim(path);
                            r.metric_bps(bps);
                            r.metric_rate(rate);
                        });
                    }
                },
            );
        });
    }

    fn ui_typography(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.section_header(
            ui,
            th,
            "Typography",
            "A short scale: title, title_2, body, caption — no decorative display faces.",
        );

        card(ui, th, |ui| {
            title_2(ui, th, "Type scale");
            ui.add_space(th.spacing.md);
            let samples = [
                ("Title", th.type_scale.title, true),
                ("Title 2", th.type_scale.title_2, true),
                ("Body", th.type_scale.body, false),
                ("Caption / dim", th.type_scale.caption, false),
            ];
            grid_cols(
                ui,
                th,
                "type_scale",
                &[ColSpec::Fixed(110.0), ColSpec::Flex],
                |g| {
                    g.row(|r| {
                        r.heading("Role");
                        r.heading("Sample");
                    });
                    for (role, size, strong) in samples {
                        g.row(|r| {
                            r.dim(role);
                            r.cell(|ui| {
                                let mut rt = RichText::new(format!(
                                    "The quick brown fox — {size:.0}px"
                                ))
                                .size(size)
                                .color(th.palette.text);
                                if strong {
                                    rt = rt.strong();
                                }
                                ui.label(rt);
                            });
                        });
                    }
                },
            );
        });

        ui.add_space(th.spacing.md);

        card(ui, th, |ui| {
            title_2(ui, th, "In context");
            ui.add_space(th.spacing.sm);
            title(ui, th, "Preferences");
            dim_label(ui, th, "Appearance · Notifications · Privacy");
            ui.add_space(th.spacing.md);
            body(
                ui,
                th,
                "Secondary copy sits a step quieter so hierarchy stays readable without heavy weight.",
            );
            ui.add_space(th.spacing.sm);
            dim_label(
                ui,
                th,
                "Captions and helper text use text_secondary at caption size.",
            );
        });
    }

    fn ui_actions(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.section_header(
            ui,
            th,
            "Actions",
            "Primary (accent fill), default (flat), and destructive — all share control height.",
        );

        card(ui, th, |ui| {
            title_2(ui, th, "Button row");
            ui.add_space(th.spacing.md);
            ui.horizontal(|ui| {
                if primary_button(ui, th, "Save").clicked() {
                    self.click_count += 1;
                    self.toast = Some(format!("Saved · clicks: {}", self.click_count));
                }
                if button(ui, th, "Cancel").clicked() {
                    self.toast = Some("Cancelled".into());
                }
                if destructive_button(ui, th, "Delete").clicked() {
                    self.toast = Some("Destructive action pressed".into());
                }
            });
            ui.add_space(th.spacing.md);
            dim_label(
                ui,
                th,
                "Suggested actions use accent blue; destructive uses the red token.",
            );
        });

        ui.add_space(th.spacing.md);

        card(ui, th, |ui| {
            title_2(ui, th, "Dialog footer");
            ui.add_space(th.spacing.sm);
            body(
                ui,
                th,
                "Typical confirm pattern: dismiss on the left, primary on the right.",
            );
            ui.add_space(th.spacing.lg);
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                let _ = button(ui, th, "Back");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let _ = primary_button(ui, th, "Continue");
                    ui.add_space(th.spacing.sm);
                    let _ = button(ui, th, "Not now");
                });
            });
        });

        ui.add_space(th.spacing.md);

        card(ui, th, |ui| {
            title_2(ui, th, "Status colors");
            ui.add_space(th.spacing.md);
            hflow(ui, th, |ui| {
                status_pill(ui, th, "Success", th.palette.success);
                status_pill(ui, th, "Warning", th.palette.warning);
                status_pill(ui, th, "Error", th.palette.destructive);
                status_pill(ui, th, "Accent", th.palette.accent);
            });
        });

        ui.add_space(th.spacing.md);

        card(ui, th, |ui| {
            title_2(ui, th, "Emoji icons (full Twemoji)");
            ui.add_space(th.spacing.sm);
            dim_label(
                ui,
                th,
                &format!(
                    "{} color glyphs embedded — any emoji maps by codepoint (flags, ZWJ, skin tones).",
                    emoji_pack_len()
                ),
            );
            ui.add_space(th.spacing.md);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for e in [
                    "👍", "❤️", "😂", "😮", "😢", "👎", "🚀", "🎉", "🔥", "✨", "👀", "💯", "🙏",
                    "😎", "🤯", "👨‍💻", "🏳️‍🌈", "🐱",
                ] {
                    emoji_icon(ui, th, e, 26.0);
                }
            });
            ui.add_space(th.spacing.sm);
            ui.horizontal(|ui| {
                dim_label(ui, th, "named shortcuts:");
                ui.add_space(th.spacing.sm);
                for ic in Icon::EMOJI {
                    icon(ui, th, *ic, 22.0);
                }
            });
        });
    }

    fn ui_surfaces(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.section_header(
            ui,
            th,
            "Surfaces",
            "Window → view → card → popover layers, with soft 1px borders and gentle radius.",
        );

        // Layer stack as a grid table (name · swatch · hex)
        card(ui, th, |ui| {
            title_2(ui, th, "Layer stack");
            ui.add_space(th.spacing.sm);
            dim_label(ui, th, "Surface tokens as a grid_cols table");
            ui.add_space(th.spacing.md);

            let layers = [
                ("window_bg", th.palette.window_bg),
                ("view_bg", th.palette.view_bg),
                ("headerbar / card", th.palette.card_bg),
                ("popover_bg", th.palette.popover_bg),
            ];

            grid_cols(
                ui,
                th,
                "surface_layers",
                &[ColSpec::Flex, ColSpec::Fixed(36.0), ColSpec::Fixed(88.0)],
                |g| {
                    g.row(|r| {
                        r.heading("Layer");
                        r.heading("");
                        r.heading("Hex");
                    });
                    for (name, color) in layers {
                        g.row(|r| {
                            r.text(name);
                            r.cell(|ui| {
                                let (rect, _) = ui.allocate_exact_size(
                                    Vec2::new(28.0, 18.0),
                                    Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, 4.0, color);
                                ui.painter().rect_stroke(
                                    rect,
                                    4.0,
                                    Stroke::new(1.0, th.palette.border_soft),
                                    egui::StrokeKind::Outside,
                                );
                            });
                            r.dim(&format_hex(color));
                        });
                    }
                },
            );
        });

        ui.add_space(th.spacing.md);

        two_col(
            ui,
            th,
            260.0,
            |ui| {
                card(ui, th, |ui| {
                    title_2(ui, th, "Card");
                    ui.add_space(th.spacing.sm);
                    body(ui, th, "card_frame() — raised content blocks.");
                    ui.add_space(th.spacing.md);
                    dim_label(ui, th, "radius_lg · soft border");
                });
            },
            |ui| {
                // Header chrome: fill column, stay inside bounds (same rule as card).
                let outer = ui.available_width();
                ui.set_max_width(outer);
                th.header_frame().show(ui, |ui| {
                    let w = ui.available_width().max(1.0);
                    ui.set_min_width(w);
                    ui.set_max_width(w);
                    title_2(ui, th, "Header");
                    ui.add_space(th.spacing.sm);
                    body(ui, th, "header_frame() — top chrome strip.");
                    ui.add_space(th.spacing.md);
                    dim_label(ui, th, "bottom border · horizontal padding");
                });
            },
        );
    }

    fn ui_palette(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.section_header(
            ui,
            th,
            "Palette",
            "Semantic tokens — not a flat material list. Swatches update with the shell toggle.",
        );

        let groups: &[(&str, &[(&str, Color32)])] = &[
            (
                "Surfaces",
                &[
                    ("window", th.palette.window_bg),
                    ("view", th.palette.view_bg),
                    ("header", th.palette.headerbar_bg),
                    ("card", th.palette.card_bg),
                    ("popover", th.palette.popover_bg),
                ],
            ),
            (
                "Accent",
                &[
                    ("accent", th.palette.accent),
                    ("hover", th.palette.accent_hover),
                    ("active", th.palette.accent_active),
                    ("on accent", th.palette.accent_fg),
                ],
            ),
            (
                "Feedback",
                &[
                    ("destructive", th.palette.destructive),
                    ("success", th.palette.success),
                    ("warning", th.palette.warning),
                ],
            ),
            (
                "Text",
                &[
                    ("primary", th.palette.text),
                    ("secondary", th.palette.text_secondary),
                    ("disabled", th.palette.text_disabled),
                ],
            ),
            (
                "Controls",
                &[
                    ("button", th.palette.button_bg),
                    ("hover", th.palette.button_hover),
                    ("active", th.palette.button_active),
                    ("border", th.palette.border),
                    ("border soft", th.palette.border_soft),
                ],
            ),
        ];

        for (group, swatches) in groups {
            card(ui, th, |ui| {
                title_2(ui, th, group);
                ui.add_space(th.spacing.md);
                hflow(ui, th, |ui| {
                    for (name, color) in *swatches {
                        swatch(ui, th, name, *color);
                    }
                });
            });
            ui.add_space(th.spacing.md);
        }
    }

    fn ui_forms(&mut self, ui: &mut egui::Ui, th: &Theme) {
        self.section_header(
            ui,
            th,
            "Forms",
            "Fields and sliders inherit Visuals from apply(); checkboxes use a themed accent control.",
        );

        card(ui, th, |ui| {
            title_2(ui, th, "Profile");
            ui.add_space(th.spacing.md);

            // Label + field: field eats remaining row width.
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                let label_w = 56.0_f32.min(ui.available_width() * 0.25);
                ui.allocate_ui_with_layout(
                    Vec2::new(label_w, th.spacing.control_height),
                    Layout::left_to_right(Align::Center),
                    |ui| body(ui, th, "Name"),
                );
                ui.add_space(th.spacing.field_gap);
                let field_w = (ui.available_width() - 4.0).max(80.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .margin(th.text_edit_margin())
                        .desired_width(field_w)
                        .min_size(egui::vec2(0.0, th.spacing.control_height))
                        .hint_text("Your name"),
                );
            });
            ui.add_space(th.spacing.md);

            body(ui, th, "Notes");
            ui.add_space(th.spacing.field_gap * 0.5);
            let _ = text_field_multiline(ui, th, &mut self.notes, 3);
            ui.add_space(th.spacing.md);

            checkbox(ui, th, &mut self.enable_sync, "Sync preferences across devices");
            ui.add_space(th.spacing.md);

            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                body(ui, th, "Volume");
                ui.add_space(th.spacing.field_gap);
                // Slider grows into remaining horizontal space (no desired_width on Slider).
                ui.add(egui::Slider::new(&mut self.volume, 0.0..=1.0).show_value(true));
            });

            ui.add_space(th.spacing.lg);
            ui.horizontal(|ui| {
                if primary_button(ui, th, "Apply").clicked() {
                    self.toast = Some(format!(
                        "Applied for {} · sync={} · vol={:.0}%",
                        self.name,
                        self.enable_sync,
                        self.volume * 100.0
                    ));
                }
                ui.add_space(th.spacing.sm);
                if button(ui, th, "Reset").clicked() {
                    self.name = "Ada".into();
                    self.notes = "A short note about the theme layer.".into();
                    self.enable_sync = true;
                    self.volume = 0.62;
                    self.toast = Some("Form reset".into());
                }
            });
        });

        ui.add_space(th.spacing.md);

        card(ui, th, |ui| {
            title_2(ui, th, "Combo & progress");
            ui.add_space(th.spacing.md);
            egui::ComboBox::from_id_salt("demo_combo")
                .selected_text(self.selected_nav.label())
                .width(ui.available_width().min(280.0))
                .show_ui(ui, |ui| {
                    for nav in Nav::ALL {
                        ui.selectable_value(&mut self.selected_nav, nav, nav.label());
                    }
                });
            ui.add_space(th.spacing.md);
            ui.add(
                egui::ProgressBar::new(self.volume)
                    .desired_width(ui.available_width())
                    .text(format!("{:.0}%", self.volume * 100.0)),
            );
        });
    }
}

// ── helpers ────────────────────────────────────────────────────────

fn status_pill(ui: &mut egui::Ui, th: &Theme, label: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.2))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(th.spacing.radius_sm)
        .inner_margin(egui::Margin::symmetric(th.spacing.md as i8, th.spacing.sm as i8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(label)
                    .size(th.type_scale.caption)
                    .color(color)
                    .strong(),
            );
        });
}

fn swatch(ui: &mut egui::Ui, th: &Theme, name: &str, color: Color32) {
    // Width from row so wrap triggers; height grows with labels (no fixed clip).
    let row_w = ui.max_rect().width().max(1.0);
    let gap = th.spacing.sm;
    let per_row = 4.0_f32;
    let cell_w = ((row_w - gap * (per_row - 1.0)) / per_row).clamp(52.0, 88.0);
    let swatch_h = cell_w * 0.78;
    // caption line-height is a bit taller than point size on Android.
    let line = th.type_scale.caption + 6.0;
    let cell_h = swatch_h + 6.0 + line * 2.0 + 8.0;

    ui.allocate_ui_with_layout(
        Vec2::new(cell_w, cell_h),
        Layout::top_down(Align::LEFT),
        |ui| {
            ui.set_width(cell_w);
            ui.set_max_width(cell_w);
            let (rect, _resp) =
                ui.allocate_exact_size(Vec2::new(cell_w, swatch_h), Sense::hover());
            ui.painter()
                .rect_filled(rect, th.spacing.radius_md, color);
            ui.painter().rect_stroke(
                rect,
                th.spacing.radius_md,
                Stroke::new(1.0_f32, th.palette.border_soft),
                egui::StrokeKind::Inside,
            );
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(
                    RichText::new(name)
                        .size(th.type_scale.caption)
                        .color(th.palette.text),
                )
                .wrap()
                .truncate(),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(format_hex(color))
                        .size(th.type_scale.caption)
                        .color(th.palette.text_secondary),
                )
                .wrap()
                .truncate(),
            );
        },
    );
}

fn format_hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

fn save_color_image_png(image: &egui::ColorImage, path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for px in &image.pixels {
        rgba.push(px.r());
        rgba.push(px.g());
        rgba.push(px.b());
        rgba.push(px.a());
    }
    image::save_buffer(
        path,
        &rgba,
        image.size[0] as u32,
        image.size[1] as u32,
        image::ColorType::Rgba8,
    )
    .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
}

trait StrongIf {
    fn strong_if(self, on: bool) -> Self;
}

impl StrongIf for RichText {
    fn strong_if(self, on: bool) -> Self {
        if on {
            self.strong()
        } else {
            self
        }
    }
}
