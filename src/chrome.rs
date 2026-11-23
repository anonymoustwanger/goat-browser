//! The browser chrome, drawn natively with egui: tab strip (also the title bar, with
//! minimize / maximize / close), navigation bar with the address bar, bookmarks bar,
//! loading bar and toasts. Layout and look follow Goat Browser's toolbar.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat, TextWrapping};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Id, Key,
    Modifiers, Painter, Pos2, Rect, ResizeDirection, Response, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2,
};

use crate::icons::{self, Icon};
use crate::paint::{self, alpha, mix};
use crate::themes::Palette;
use crate::anim;
use crate::{now, App, Cmd, DlState, Popup, TabKind, INTERNAL_BASE};

pub const DL_H: f32 = 48.0;
pub const STRIP_H: f32 = 38.0;
pub const NAV_H: f32 = 44.0;
pub const BM_H: f32 = 32.0;
pub const CONTROLS_W: f32 = 138.0;
/// Frame left around web pages so the (borderless) window can be resized from its edges.
pub const MARGIN: f32 = 4.0;
pub const PURPLE: Color32 = Color32::from_rgb(0xa4, 0x63, 0xf2);
/// Goat's private-tab violet (--pv).
pub const PV: Color32 = Color32::from_rgb(0xa7, 0x8b, 0xfa);

/// Tint for a tab kind (None for normal tabs).
pub fn kind_color(k: TabKind) -> Option<Color32> {
    match k {
        TabKind::Normal => None,
        TabKind::Private => Some(PV),
        TabKind::Tor => Some(PURPLE),
    }
}

pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, semibold())
}

/// Use Segoe UI (Windows' UI font) when it's there, falling back to egui's built-in fonts.
pub fn install_fonts(ctx: &egui::Context) -> bool {
    let mut fonts = FontDefinitions::default();
    let mut ok = false;
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let mut prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut semi = prop.clone();
    if let Ok(b) = std::fs::read(dir.join("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(b)));
        prop.insert(0, "segoe".into());
        semi.insert(0, "segoe".into());
        ok = true;
    }
    if let Ok(b) = std::fs::read(dir.join("seguisb.ttf")) {
        fonts.font_data.insert("segoe-sb".into(), Arc::new(FontData::from_owned(b)));
        semi.insert(0, "segoe-sb".into());
    }
    if let Ok(b) = std::fs::read(dir.join("seguisym.ttf")) {
        fonts.font_data.insert("segoe-sym".into(), Arc::new(FontData::from_owned(b)));
        prop.push("segoe-sym".into());
        semi.push("segoe-sym".into());
    }
    // Every emoji (profile logos); egui's own emoji font only has some of them.
    if let Ok(b) = std::fs::read(dir.join("seguiemj.ttf")) {
        fonts.font_data.insert("segoe-emoji".into(), Arc::new(FontData::from_owned(b)));
        prop.push("segoe-emoji".into());
        semi.push("segoe-emoji".into());
    }
    fonts.families.insert(FontFamily::Proportional, prop);
    fonts.families.insert(semibold(), semi);
    ctx.set_fonts(fonts);
    ok
}

/// Window-relative position → screen position (for pop-up windows).
pub fn to_screen(ctx: &egui::Context, p: Pos2) -> Pos2 {
    ctx.input(|i| i.viewport().inner_rect).map(|r| r.min + p.to_vec2()).unwrap_or(p)
}

/// Single-line text that ends in "…" when it doesn't fit.
pub fn text_elided(p: &Painter, pos: Pos2, text: &str, f: FontId, color: Color32, max_w: f32, anchor: Align2) -> Rect {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(f, color));
    job.wrap = TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = p.layout_job(job);
    let rect = anchor.anchor_size(pos, g.size());
    p.galley(rect.min, g, color);
    rect
}

pub fn text_width(ui: &Ui, text: &str, f: FontId) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), f, Color32::WHITE).size().x
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// First letter of a bookmark's name (falls back to its site).
pub fn name_letter(title: &str, url: &str) -> String {
    match title.chars().find(|c| c.is_alphanumeric()) {
        Some(c) => c.to_uppercase().to_string(),
        None => site_letter("", url),
    }
}

/// First letter of a site for the tab/bookmark "dot".
pub fn site_letter(title: &str, url: &str) -> String {
    let host = crate::adblock::host_of(url);
    let h = host.trim_start_matches("www.");
    h.chars().next().or_else(|| title.trim().chars().next()).map(|c| c.to_uppercase().to_string()).unwrap_or_default()
}

pub fn nav_bg(pal: &Palette) -> Color32 {
    if pal.gradients {
        alpha(pal.bg2, 0.86)
    } else {
        pal.bg2
    }
}

/// Square icon button with Goat's hover/press look.
pub fn icon_button(ui: &mut Ui, rect: Rect, id: Id, icon: Icon, pal: &Palette, enabled: bool, width: f32) -> Response {
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let hover = crate::anim::hover(ui.ctx(), id.with("h"), enabled && resp.hovered(), 0.12);
    if hover > 0.0 {
        p.rect_filled(rect, 8, alpha(pal.bg3, hover));
    }
    let shrink = crate::anim::tween(ui.ctx(), id.with("press"), if resp.is_pointer_button_down_on() { 0.82 } else { 1.0 }, 0.3, crate::anim::spring);
    // Hover: the icon grows a little with a bounce.
    let grow = crate::anim::tween(ui.ctx(), id.with("grow"), if enabled && resp.hovered() { 1.22 } else { 1.0 }, 0.32, crate::anim::ease_bounce);
    let ir = Rect::from_center_size(rect.center(), Vec2::splat(16.0 * shrink * grow));
    let color = if enabled { pal.text } else { alpha(pal.muted, 0.45) };
    // Goat's extensions button tilts when hovered.
    let tilt = if id == Id::new("extbtn") { crate::anim::tween(ui.ctx(), id.with("tilt"), if resp.hovered() { -0.14 } else { 0.0 }, 0.32, crate::anim::ease_bounce) } else { 0.0 };
    if tilt.abs() > 0.001 {
        icons::draw_rot(p, ir, icon, color, width, tilt);
    } else {
        icons::draw(p, ir, icon, color, width);
    }
    resp
}

impl App {
    /// Start-up "Who's browsing?" screen. Returns the chosen profile ("" = Default).
    pub fn draw_profile_picker(&mut self, ui: &mut Ui) -> Option<String> {
        let ctx = ui.ctx().clone();
        let pal = self.palette();
        self.clear = [0.0, 0.0, 0.0, 0.0];
        let full = ui.max_rect();
        let p = ui.painter().clone();
        p.rect_filled(full, 0, pal.bg);
        // Drag the window by its top; close button top-right.
        let bar = Rect::from_min_size(full.min, vec2(full.width() - 46.0, 40.0));
        if ui.interact(bar, Id::new("picker-drag"), Sense::click_and_drag()).drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let xr = Rect::from_min_size(pos2(full.right() - 46.0, full.top()), vec2(46.0, 36.0));
        let xresp = ui.interact(xr, Id::new("picker-close"), Sense::click());
        if xresp.hovered() {
            p.rect_filled(xr, 0, Color32::from_rgb(0xc4, 0x2b, 0x1c));
        }
        let c = xr.center();
        p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], Stroke::new(1.2, pal.text));
        p.line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], Stroke::new(1.2, pal.text));
        if xresp.clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let profiles = crate::store::profiles();
        let tile = vec2(132.0, 150.0);
        let gap = 18.0;
        let per_row = (((full.width() - 80.0) / (tile.x + gap)).floor() as usize).clamp(1, 6);
        let rows = profiles.len().div_ceil(per_row);
        let block_h = 70.0 + rows as f32 * (tile.y + gap) + 50.0;
        let top = (full.center().y - block_h / 2.0).max(full.top() + 50.0);
        p.text(pos2(full.center().x, top), Align2::CENTER_TOP, "Who's browsing?", bold(26.0), pal.text);
        p.text(pos2(full.center().x, top + 38.0), Align2::CENTER_TOP, "Pick a profile. Each has its own cookies, history and extensions.", font(13.5), pal.muted);
        let mut chosen = None;
        for (i, prof) in profiles.iter().enumerate() {
            let row = i / per_row;
            let in_row = if row + 1 == rows { profiles.len() - row * per_row } else { per_row };
            let row_w = in_row as f32 * tile.x + (in_row as f32 - 1.0) * gap;
            let x = full.center().x - row_w / 2.0 + (i % per_row) as f32 * (tile.x + gap);
            let y = top + 80.0 + row as f32 * (tile.y + gap);
            let r = Rect::from_min_size(pos2(x, y), tile);
            let resp = ui.interact(r, Id::new(("picker-tile", i)), Sense::click());
            // Bouncy grow on hover.
            let grow = anim::tween(&ctx, Id::new(("picker-grow", i)), if resp.hovered() { 1.08 } else { 1.0 }, 0.32, anim::ease_bounce);
            let h = anim::hover(&ctx, Id::new(("picker-h", i)), resp.hovered(), 0.15);
            let rr = Rect::from_center_size(r.center(), r.size() * grow);
            p.rect_filled(rr, 18, mix(pal.bg2, pal.bg3, h));
            p.rect_stroke(rr, 18, Stroke::new(1.0, mix(pal.line, pal.accent, h * 0.6)), egui::StrokeKind::Inside);
            let (glyph, color) = crate::store::profile_logo(prof);
            let color = Color32::from_hex(&color).unwrap_or(pal.accent);
            let lc = pos2(rr.center().x, rr.top() + 58.0 * grow);
            p.circle_filled(lc, 34.0 * grow, color);
            p.text(lc + vec2(0.0, 1.0), Align2::CENTER_CENTER, &glyph, bold((28.0 * grow).round()), Color32::from_rgb(0x10, 0x12, 0x14));
            let name = if prof.is_empty() { "Default" } else { prof.as_str() };
            text_elided(&p, pos2(rr.center().x, rr.bottom() - 26.0 * grow), name, bold(((14.0 * grow) * 2.0).round() / 2.0), pal.text, rr.width() - 16.0, Align2::CENTER_CENTER);
            if resp.hovered() {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                chosen = Some(prof.clone());
            }
        }
        // "Always open this profile".
        let cy = top + 80.0 + rows as f32 * (tile.y + gap) + 14.0;
        let label = "Always open the profile I pick when Goat starts";
        let w = 18.0 + 10.0 + text_width(ui, label, font(13.0));
        let row = Rect::from_min_size(pos2(full.center().x - w / 2.0, cy), vec2(w, 22.0));
        let cr = ui.interact(row, Id::new("picker-always"), Sense::click());
        if cr.clicked() {
            self.profile_always = !self.profile_always;
        }
        let on = self.profile_always;
        let b = Rect::from_min_size(pos2(row.left(), row.center().y - 9.0), Vec2::splat(18.0));
        p.rect_filled(b, 5, if on { pal.accent } else { pal.bg2 });
        p.rect_stroke(b, 5, Stroke::new(1.0, if on { pal.accent } else { pal.line }), egui::StrokeKind::Inside);
        if on {
            icons::draw(&p, b.shrink(3.0), icons::CHECK, pal.ink, 2.0);
        }
        p.text(pos2(b.right() + 10.0, row.center().y), Align2::LEFT_CENTER, label, font(13.0), if cr.hovered() { pal.text } else { pal.muted });
        p.text(pos2(full.center().x, cy + 30.0), Align2::CENTER_TOP, "You can change this later in Settings → Profiles.", font(11.5), alpha(pal.muted, 0.8));
        chosen
    }

    pub fn draw_chrome(&mut self, ui: &mut Ui, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let pal = self.palette();
        // Pop-up windows are see-through, so the clear colour is transparent; Goat's own
        // window paints its background here instead.
        self.clear = [0.0, 0.0, 0.0, 0.0];
        let anim = self.animations();
        anim::set_enabled(&ctx, anim);
        let settings = self.settings();
        self.apply_visuals(&ctx, &pal);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0, pal.bg);
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let fs = self.fullscreen;
        let chrome_h = if fs { 0.0 } else { STRIP_H + NAV_H + if settings.show_bookmarks_bar { BM_H } else { 0.0 } };
        let margin = if maximized || fs { 0.0 } else { MARGIN };
        let dl_h = if !fs && self.dl_bar && settings.download_bar && !self.dl_items.is_empty() { DL_H } else { 0.0 };
        self.content_rect = Rect::from_min_max(
            pos2(full.left() + margin, full.top() + chrome_h),
            pos2(full.right() - margin, full.bottom() - margin - dl_h),
        );

        self.shortcuts(&ctx, cmds);
        if fs {
            return;
        }

        // Clicking the toolbar takes keyboard focus away from the web page.
        if ctx.input(|i| i.pointer.any_pressed()) {
            if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                if p.y < full.top() + chrome_h {
                    if let Some(t) = self.active_tab() {
                        let _ = t.webview.focus_parent();
                    }
                }
            }
        }

        // Background: Goat's chrome gradient (accent tint at the left, accent-2 at the right).
        let p = ui.painter().clone();
        if pal.gradients {
            paint::hgrad(
                &p,
                full,
                &[(0.0, mix(pal.accent, pal.bg, 0.16)), (0.32, pal.bg), (0.66, pal.bg), (1.0, mix(pal.accent2, pal.bg, 0.14))],
            );
        } else {
            p.rect_filled(full, 0, pal.bg);
        }
        if margin > 0.0 {
            // Thin window frame around the page area.
            let below = Rect::from_min_max(pos2(full.left(), full.top() + chrome_h), full.max);
            p.rect_filled(below, 0, nav_bg(&pal));
        }

        let strip = Rect::from_min_size(full.min, vec2(full.width(), STRIP_H));
        let nav = Rect::from_min_size(pos2(full.left(), strip.bottom()), vec2(full.width(), NAV_H));

        // Window edges for resizing (top edge lives in the tab strip).
        let resizing = !maximized && self.resize_edges(ui, full, cmds);

        self.draw_tabstrip(ui, strip, &pal, anim, cmds, resizing);
        self.draw_navbar(ui, nav, &pal, anim, cmds, settings.show_bookmarks_bar);
        if settings.show_bookmarks_bar {
            let bm = Rect::from_min_size(pos2(full.left(), nav.bottom()), vec2(full.width(), BM_H));
            self.draw_bookmarks(ui, bm, &pal, cmds);
        }

        // Loading bar along the bottom of the chrome (fades in and out).
        let load_t = anim::hover(&ctx, Id::new("progress"), self.active_tab().is_some_and(|t| t.loading), 0.25);
        if load_t > 0.01 {
            let mut p = p.clone();
            p.set_opacity(load_t);
            let bar = Rect::from_min_size(pos2(full.left(), full.top() + chrome_h - 2.0), vec2(full.width(), 2.0));
            if anim {
                let t = (now(&ctx) % 1.1 / 1.1) as f32;
                let w = bar.width() * 0.35;
                let x = bar.left() - w + (bar.width() + w) * ease_out(t);
                let seg = Rect::from_min_size(pos2(x, bar.top()), vec2(w, 2.0)).intersect(bar);
                if seg.width() > 0.0 {
                    let tr = alpha(pal.accent, 0.0);
                    paint::hgrad(&p, seg, &[(0.0, tr), (0.4, pal.accent), (0.7, pal.accent2), (1.0, alpha(pal.accent2, 0.0))]);
                }
                ctx.request_repaint();
            } else {
                p.rect_filled(bar, 0, alpha(pal.accent, 0.5));
            }
        }

        if dl_h > 0.0 {
            let bar = Rect::from_min_max(pos2(self.content_rect.left(), self.content_rect.bottom()), pos2(self.content_rect.right(), self.content_rect.bottom() + dl_h));
            self.draw_download_bar(ui, bar, &pal, cmds);
        }
        self.draw_toast(ui, full, &pal, anim, cmds);
    }

    fn apply_visuals(&self, ctx: &egui::Context, pal: &Palette) {
        let mut v = if pal.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.selection.bg_fill = alpha(pal.accent, 0.35);
        v.selection.stroke = Stroke::new(1.0, pal.text);
        v.text_cursor.stroke = Stroke::new(1.5, pal.accent);
        v.extreme_bg_color = pal.bg;
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = pal.bg2;
        v.hyperlink_color = pal.accent;
        let cur = ctx.global_style();
        let changed = cur.visuals.selection.bg_fill != v.selection.bg_fill || cur.visuals.dark_mode != v.dark_mode || cur.visuals.window_fill != v.window_fill;
        if changed {
            ctx.all_styles_mut(|s| s.visuals = v.clone());
        }
    }

    fn resize_edges(&self, ui: &mut Ui, full: Rect, cmds: &mut Vec<Cmd>) -> bool {
        let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) else { return false };
        let e = 5.0;
        let (l, r, t, b) = (pos.x < full.left() + e, pos.x > full.right() - e, pos.y < full.top() + 3.0, pos.y > full.bottom() - e);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest)),
            (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast)),
            (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest)),
            (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast)),
            (true, ..) => Some((ResizeDirection::West, CursorIcon::ResizeWest)),
            (_, true, ..) => Some((ResizeDirection::East, CursorIcon::ResizeEast)),
            (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeNorth)),
            (.., true) => Some((ResizeDirection::South, CursorIcon::ResizeSouth)),
            _ => None,
        };
        if let Some((d, c)) = dir {
            // Window controls at the top-right keep their own cursor.
            if !(t && pos.x > full.right() - CONTROLS_W && !r) {
                ui.ctx().set_cursor_icon(c);
                if ui.ctx().input(|i| i.pointer.primary_pressed()) {
                    cmds.push(Cmd::WinResize(d));
                }
                return true;
            }
        }
        false
    }

    fn shortcuts(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        let c = Modifiers::COMMAND;
        let mut k = |m: Modifiers, key: Key, cmd: Cmd| {
            if ctx.input_mut(|i| i.consume_key(m, key)) {
                cmds.push(cmd);
            }
        };
        k(cs, Key::T, Cmd::ReopenTab);
        k(cs, Key::N, Cmd::NewPrivateTab);
        k(Modifiers::ALT | Modifiers::SHIFT, Key::N, Cmd::NewTorTab);
        k(cs, Key::Tab, Cmd::PrevTab);
        k(cs, Key::B, Cmd::ToggleBar);
        k(cs, Key::O, Cmd::OpenPage("bookmarks"));
        k(c, Key::T, Cmd::NewTab);
        k(c, Key::Tab, Cmd::NextTab);
        k(c, Key::L, Cmd::FocusAddress);
        k(Modifiers::ALT, Key::D, Cmd::FocusAddress);
        k(Modifiers::NONE, Key::F6, Cmd::FocusAddress);
        k(c, Key::D, Cmd::ToggleBookmark);
        k(c, Key::F, Cmd::FindOpen(true));
        k(c, Key::H, Cmd::OpenPage("history"));
        k(c, Key::J, Cmd::OpenPage("downloads"));
        k(c, Key::Comma, Cmd::OpenPage("settings"));
        k(c, Key::R, Cmd::Reload);
        k(Modifiers::NONE, Key::F5, Cmd::Reload);
        k(Modifiers::NONE, Key::F11, Cmd::Fullscreen);
        k(Modifiers::NONE, Key::F12, Cmd::Devtools);
        k(c, Key::P, Cmd::Print);
        k(c, Key::Equals, Cmd::ZoomIn);
        k(c, Key::Plus, Cmd::ZoomIn);
        k(c, Key::Minus, Cmd::ZoomOut);
        k(c, Key::Num0, Cmd::ZoomReset);
        k(Modifiers::ALT, Key::ArrowLeft, Cmd::Back);
        k(Modifiers::ALT, Key::ArrowRight, Cmd::Forward);
        k(Modifiers::ALT, Key::Home, Cmd::Home);
        for (i, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
            k(c, key, Cmd::SelectIndex(i + 1));
        }
        if let Some(id) = self.active_tab().map(|t| t.id) {
            k(c, Key::W, Cmd::CloseTab(id));
        }
        if self.fullscreen && ctx.input(|i| i.key_pressed(Key::Escape)) {
            cmds.push(Cmd::Fullscreen);
        }
    }

    // ------------------------------------------------------------------ tab strip

    fn draw_tabstrip(&mut self, ui: &mut Ui, strip: Rect, pal: &Palette, anim: bool, cmds: &mut Vec<Cmd>, resizing: bool) {
        let ctx = ui.ctx().clone();
        let t_now = now(&ctx);

        // The empty part of the strip drags the window; double-click maximizes.
        let drag = ui.interact(strip, Id::new("strip-drag"), Sense::click_and_drag());
        if !resizing {
            if drag.double_clicked() {
                cmds.push(Cmd::WinToggleMax);
            } else if drag.drag_started() {
                cmds.push(Cmd::WinDrag);
            }
        }

        let controls = Rect::from_min_max(pos2(strip.right() - CONTROLS_W, strip.top()), strip.right_bottom());
        let n = self.tabs.len().max(1) as f32;
        let avail = (strip.width() - 8.0 - CONTROLS_W - 40.0 - 30.0).max(44.0);
        let tab_w = (avail / n - 2.0).clamp(44.0, 220.0);
        let mut x = strip.left() + 8.0;
        let p = ui.painter().with_clip_rect(Rect::from_min_max(strip.min, pos2(controls.left(), strip.bottom())));

        // The active-tab highlight slides from tab to tab.
        if let Some(t) = self.tabs.get(self.active) {
            let target_x = strip.left() + 8.0 + self.active as f32 * (tab_w + 2.0);
            let mut hx = anim::tween(&ctx, Id::new("active-x"), target_x, 0.34, anim::spring);
            // Dragging the active tab: the highlight goes with it.
            if let (Some((did, grab, _)), Some(p)) = (self.tab_drag, ctx.input(|i| i.pointer.interact_pos())) {
                if did == t.id {
                    let min_x = strip.left() + 8.0;
                    let max_x = (controls.left() - 40.0 - tab_w).max(min_x);
                    hx = (p.x - grab).clamp(min_x, max_x);
                    anim::set(&ctx, Id::new("active-x"), hx);
                }
            }
            let hw = anim::tween(&ctx, Id::new("active-w"), tab_w, 0.34, anim::spring);
            let tint = anim::tween(&ctx, Id::new("active-tint"), if t.kind == TabKind::Normal { 0.0 } else { 1.0 }, 0.26, anim::out_cubic);
            let kc = kind_color(t.kind).unwrap_or(PV);
            let nbg = nav_bg(pal);
            let fill = mix(mix(kc, nbg, 0.14), nbg, tint);
            let r = Rect::from_min_size(pos2(hx, strip.top() + 6.0), vec2(hw, 32.0));
            p.rect_filled(r, CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }, fill);
            p.line_segment([r.left_top() + vec2(9.0, 0.5), r.right_top() + vec2(-9.0, 0.5)], Stroke::new(1.0, alpha(pal.text, 0.06)));
        }
        let mut prev_lit = true;
        let icons_on = self.settings().site_icons;
        let tabs: Vec<(u32, String, String, bool, TabKind, f64, Option<egui::TextureId>)> = self
            .tabs
            .iter()
            .map(|t| (t.id, t.title.clone(), t.url.clone(), t.loading, t.kind, t.born, t.favicon.as_ref().filter(|_| icons_on).map(|f| f.1.id()).or_else(|| if t.pending.is_some() { self.site_icon(&t.url) } else { None })))
            .collect();
        let mut rects = std::collections::HashMap::new();
        for (i, (id, title, url, loading, kind, born, icon)) in tabs.into_iter().enumerate() {
            let tid = Id::new(("tab", id));
            // Tabs glide to their slot when others open or close.
            let mut ax = anim::tween(&ctx, tid.with("x"), x, 0.32, anim::spring);
            let aw = anim::tween(&ctx, tid.with("w"), tab_w, 0.24, anim::out_expo);
            let rect = Rect::from_min_size(pos2(ax, strip.top() + 6.0), vec2(aw, 32.0));
            x += tab_w + 2.0;
            let dragging_me = self.tab_drag.is_some_and(|d| d.0 == id);
            rects.insert(id, rect);
            let active = i == self.active;
            // Clickable right up to the top edge of the window (like the window buttons), so a
            // maximised window's tabs work with the mouse slammed against the top of the screen.
            let hit = Rect::from_min_max(pos2(rect.left(), strip.top()), rect.max);
            let resp = ui.interact(hit, tid, Sense::click_and_drag());
            // Drag to reorder; drag out of the strip to move it to another window.
            if resp.drag_started() {
                if let Some(p) = ctx.input(|i| i.pointer.press_origin()) {
                    self.tab_drag = Some((id, p.x - rect.left(), false));
                }
            }
            let mut lift = 0.0;
            if let Some((did, grab, _)) = self.tab_drag {
                if did == id {
                    if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                        let min_x = strip.left() + 8.0;
                        let max_x = (controls.left() - 40.0 - aw).max(min_x);
                        ax = (p.x - grab).clamp(min_x, max_x);
                        anim::set(&ctx, tid.with("x"), ax);
                        let slot = ((ax - min_x + (tab_w + 2.0) / 2.0) / (tab_w + 2.0)).floor().max(0.0) as usize;
                        if slot != i {
                            cmds.push(Cmd::MoveTab(id, slot));
                        }
                        // Pulled down out of the strip: about to become its own window.
                        lift = (p.y - strip.bottom()).max(0.0).min(24.0);
                        let screen = ctx.input(|i| i.viewport_rect());
                        let outside = p.y > strip.bottom() + 40.0 || p.y < strip.top() - 10.0 || !screen.contains(p);
                        if outside && self.tabs.len() > 1 {
                            if self.tear_preview.as_ref().is_none_or(|t| t.drop_at.is_some()) {
                                let icon = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.favicon.as_ref()).filter(|_| self.settings().site_icons).map(|f| f.1.id());
                                self.tear_preview = Some(crate::TearPreview { title: title.clone(), icon, born: t_now, drop_at: None, fade_from: None });
                            }
                        } else if self.tear_preview.as_ref().is_some_and(|t| t.drop_at.is_none()) {
                            self.tear_preview = None;
                        }
                        if let Some(d) = self.tab_drag.as_mut() {
                            d.2 = true;
                        }
                    }
                }
            }
            let close_rect = Rect::from_min_size(pos2(rect.right() - 6.0 - 22.0, rect.center().y - 11.0), Vec2::splat(22.0));
            let narrow = aw < 66.0;
            // Geometric hover (egui stops reporting hover on other widgets while a button is held).
            let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| hit.contains(p));
            let show_close = active || (over && aw > 96.0) || (narrow && over);
            let close = ui.interact(close_rect, tid.with("x"), Sense::click());
            let hovered = over;

            if (show_close && close.clicked()) || resp.middle_clicked() {
                cmds.push(Cmd::CloseTab(id));
            } else if over && !(show_close && close.hovered()) && ctx.input(|i| i.pointer.primary_pressed()) && !active {
                cmds.push(Cmd::SelectTab(id));
            }
            if resp.secondary_clicked() {
                let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(rect.left_bottom());
                cmds.push(Cmd::Popup(Popup::TabMenu(id), to_screen(&ctx, at)));
            }

            // Tab-in: fade in place (nothing drawn outside the strip).
            let k = if anim { ((t_now - born) / 0.36) as f32 } else { 1.0 };
            if k < 1.0 {
                ctx.request_repaint();
            }
            let e = anim::spring(k);
            let mut tp = p.clone();
            tp.set_opacity(anim::out_cubic(k * 1.6));
            // Goat's tab-in: rise from 8px below and grow from 94%, with a little spring.
            let r = Rect::from_center_size(rect.center() + vec2(0.0, 8.0 * (1.0 - e) + lift), rect.size() * (0.94 + 0.06 * e));
            if dragging_me {
                tp.set_opacity(if lift > 12.0 { 0.7 } else { 1.0 });
                ctx.request_repaint();
            }
            self.paint_tab(&tp, r, pal, &title, &url, loading, kind, active, hovered, narrow, show_close, close.hovered(), i > 0 && !prev_lit, tid, t_now, anim, icon);
            prev_lit = active || hovered;
            resp.on_hover_text_at_pointer(&title);
        }
        self.tab_rects = rects;
        // Let go of a dragged tab: well below the strip, or outside the window, it moves out.
        if let Some((did, _, moved)) = self.tab_drag {
            if !ctx.input(|i| i.pointer.primary_down()) {
                self.tab_drag = None;
                let p = ctx.input(|i| i.pointer.latest_pos());
                let screen = ctx.input(|i| i.viewport_rect());
                let out = match p {
                    Some(p) => p.y > strip.bottom() + 40.0 || p.y < strip.top() - 10.0 || !screen.contains(p),
                    None => true,
                };
                if moved && out {
                    cmds.push(Cmd::TearOff(did));
                    // The preview stays where you let go until the new window is showing.
                    let ppp = ctx.pixels_per_point();
                    #[cfg(windows)]
                    let at = crate::winhooks::cursor_pos().map(|(x, y)| pos2(x / ppp, y / ppp));
                    #[cfg(not(windows))]
                    let at = p;
                    if let (Some(tp), Some(at)) = (self.tear_preview.as_mut(), at) {
                        tp.drop_at = Some((t_now, at));
                    }
                } else {
                    self.tear_preview = None;
                }
            }
        }
        // Recently closed tabs fade out where they were.
        self.ghosts.retain(|g| t_now - g.born < 0.2);
        for g in &self.ghosts {
            let k = ((t_now - g.born) / 0.2) as f32;
            let mut gp = p.clone();
            gp.set_opacity((1.0 - anim::in_cubic(k)) * 0.9);
            let r = Rect::from_center_size(g.rect.center(), g.rect.size() * (1.0 - 0.1 * anim::out_cubic(k)));
            self.paint_tab(&gp, r, pal, &g.title, &g.url, false, g.kind, false, false, false, false, false, false, Id::new(("ghost", g.born.to_bits())), t_now, anim, None);
            ctx.request_repaint();
        }

        // New-tab button (full strip height for easy clicking).
        let nt = Rect::from_min_size(pos2(x, strip.top()), vec2(40.0, STRIP_H));
        let r = ui.interact(nt, Id::new("newtab"), Sense::click());
        let h = crate::anim::hover(&ctx, Id::new("newtab-h"), r.hovered(), 0.2);
        let circle = Rect::from_min_size(pos2(nt.center().x - 15.0, nt.bottom() - 33.0), Vec2::splat(30.0));
        if h > 0.0 {
            p.rect_filled(circle, 9, alpha(pal.bg3, h));
        }
        let angle = if anim { h * std::f32::consts::FRAC_PI_2 } else { 0.0 };
        icons::draw_rot(&p, Rect::from_center_size(circle.center(), Vec2::splat(18.0)), icons::PLUS, if r.hovered() { pal.text } else { pal.muted }, 1.6, angle);
        if r.clicked() {
            cmds.push(Cmd::NewTab);
        }
        if r.secondary_clicked() {
            cmds.push(Cmd::NewPrivateTab);
        }
        r.on_hover_text("New tab (Ctrl+T) · right-click for a private tab");

        // Minimize / maximize / close.
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let wp = ui.painter();
        for (k, (icon, cmd)) in [
            (icons::WIN_MIN, Cmd::WinMinimize),
            (if maximized { icons::WIN_RESTORE } else { icons::WIN_MAX }, Cmd::WinToggleMax),
            (icons::WIN_CLOSE, Cmd::WinClose),
        ]
        .into_iter()
        .enumerate()
        {
            let b = Rect::from_min_size(pos2(controls.left() + 46.0 * k as f32, strip.top()), vec2(46.0, STRIP_H));
            let resp = ui.interact(b, Id::new(("wc", k)), Sense::click());
            let is_close = k == 2;
            let h = crate::anim::hover(&ctx, Id::new(("wch", k)), resp.hovered(), 0.1);
            if h > 0.0 {
                let bg = if is_close { alpha(Color32::from_rgb(0xe8, 0x11, 0x23), h) } else { alpha(pal.text, 0.08 * h) };
                wp.rect_filled(b, 0, bg);
            }
            let col = if is_close && resp.hovered() { Color32::WHITE } else { pal.text };
            icons::draw(wp, Rect::from_center_size(b.center(), Vec2::splat(16.0)), icon, col, 1.0);
            if resp.clicked() {
                cmds.push(cmd);
            }
        }
    }

    /// One tab, Goat style (also used for the fading "ghost" of a closed tab).
    #[allow(clippy::too_many_arguments)]
    fn paint_tab(&self, tp: &Painter, r: Rect, pal: &Palette, title: &str, url: &str, loading: bool, kind: TabKind, active: bool,
                 hovered: bool, narrow: bool, show_close: bool, close_hovered: bool, sep: bool, tid: Id, t_now: f64, anim: bool, icon: Option<egui::TextureId>) {
        let ctx = &self.ctx;
        let nbg = nav_bg(pal);
        let tint = kind_color(kind);
        let top = CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 };
        let hover_t = anim::hover(ctx, tid.with("hv"), hovered && !active, 0.2);
        let _ = nbg;
        if active {
            // Background is the sliding highlight drawn by draw_tabstrip.
        } else if hover_t > 0.0 {
            tp.rect_filled(r, top, alpha(pal.bg2, 0.7 * hover_t));
        }
        if let Some(c) = tint {
            tp.rect_filled(Rect::from_min_max(pos2(r.left() + 6.0, r.bottom() - 2.0), pos2(r.right() - 6.0, r.bottom())), 2, c);
        }
        if sep && !active && !hovered {
            tp.line_segment([pos2(r.left() - 1.0, r.top() + 9.0), pos2(r.left() - 1.0, r.top() + 23.0)], Stroke::new(1.0, alpha(pal.line, 1.0 - hover_t)));
        }
        // Site dot: letter, spinner, or the private / Tor mark.
        let dot = if narrow { Rect::from_center_size(r.center(), Vec2::splat(16.0)) } else { Rect::from_min_size(pos2(r.left() + 12.0, r.center().y - 8.0), Vec2::splat(16.0)) };
        let hide_dot = narrow && (active || hovered);
        if !hide_dot {
            if loading {
                tp.circle_stroke(dot.center(), 6.0, Stroke::new(2.0, pal.line));
                let a0 = if anim { (t_now * std::f64::consts::TAU / 0.8) as f32 } else { 0.0 };
                let pts: Vec<Pos2> = (0..=14)
                    .map(|k| dot.center() + Vec2::angled(a0 + k as f32 / 14.0 * std::f32::consts::FRAC_PI_2) * 6.0)
                    .collect();
                tp.add(egui::Shape::line(pts, Stroke::new(2.0, if anim { pal.accent } else { pal.accent })));
                if anim {
                    ctx.request_repaint();
                }
            } else if let Some(c) = tint {
                tp.rect_filled(dot, 5, alpha(c, 0.28));
                let icon = if kind == TabKind::Tor { icons::ONION } else { icons::MASK };
                icons::draw_filled(tp, dot.shrink(1.5), icon, c, 1.4, Some(alpha(c, 0.25)));
            } else if let Some(tex) = icon {
                tp.image(tex, dot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                tp.rect_filled(dot, 5, pal.bg3);
                let letter = if url.starts_with(INTERNAL_BASE) { title.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() } else { site_letter(title, url) };
                tp.text(dot.center(), Align2::CENTER_CENTER, letter, bold(10.0), pal.muted);
            }
        }
        if !narrow {
            let tx = dot.right() + 8.0;
            let close_left = r.right() - 6.0 - 22.0;
            let right = if show_close { close_left - 4.0 } else { r.right() - 8.0 };
            let color = mix(pal.text, pal.muted, if active { 1.0 } else { hover_t });
            let label = if title.is_empty() { "New Tab" } else { title };
            text_elided(tp, pos2(tx, r.center().y), label, font(12.5), color, right - tx, Align2::LEFT_CENTER);
        }
        let close_rect = Rect::from_min_size(pos2(r.right() - 6.0 - 22.0, r.center().y - 11.0), Vec2::splat(22.0));
        let vis = anim::hover(ctx, tid.with("xv"), show_close, 0.12);
        if vis > 0.0 {
            let ch = anim::hover(ctx, tid.with("xh"), close_hovered, 0.12);
            if ch > 0.0 {
                tp.rect_filled(close_rect, 6, alpha(pal.bg3, ch * vis));
            }
            let col = alpha(mix(pal.text, pal.muted, ch), vis);
            icons::draw(tp, Rect::from_center_size(close_rect.center(), Vec2::splat(12.0)), icons::CLOSE, col, 1.8);
        }
    }

    // ------------------------------------------------------------------ navigation bar

    fn draw_navbar(&mut self, ui: &mut Ui, nav: Rect, pal: &Palette, anim: bool, cmds: &mut Vec<Cmd>, bar_shown: bool) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        p.rect_filled(nav, 0, nav_bg(pal));
        if !bar_shown {
            p.line_segment([nav.left_bottom() - vec2(0.0, 0.5), nav.right_bottom() - vec2(0.0, 0.5)], Stroke::new(1.0, pal.line));
        }
        let cy = nav.center().y;
        let btn = |x: f32| Rect::from_min_size(pos2(x, cy - 16.0), Vec2::splat(32.0));

        let (loading, kind, url, title) = match self.active_tab() {
            Some(t) => (t.loading, t.kind, t.url.clone(), t.title.clone()),
            None => (false, TabKind::Normal, String::new(), String::new()),
        };
        let tor = kind == TabKind::Tor;
        let private_like = kind.is_private_like();
        let internal = url.starts_with(INTERNAL_BASE);
        let web = url.starts_with("http") && !internal;

        let mut x = nav.left() + 8.0;
        if icon_button(ui, btn(x), Id::new("back"), icons::BACK, pal, true, 1.6).on_hover_text("Back (Alt+Left)").clicked() {
            cmds.push(Cmd::Back);
        }
        x += 36.0;
        if icon_button(ui, btn(x), Id::new("fwd"), icons::FORWARD, pal, true, 1.6).on_hover_text("Forward (Alt+Right)").clicked() {
            cmds.push(Cmd::Forward);
        }
        x += 36.0;
        let (ic, tip, cmd) = if loading { (icons::STOP, "Stop", Cmd::Stop) } else { (icons::RELOAD, "Reload (Ctrl+R)", Cmd::Reload) };
        if icon_button(ui, btn(x), Id::new("reload"), ic, pal, true, 1.6).on_hover_text(tip).clicked() {
            cmds.push(cmd);
        }
        x += 36.0 + 6.0;

        // Right-hand side, laid out from the right edge.
        let mut rx = nav.right() - 8.0;
        let cog = btn(rx - 32.0);
        rx -= 36.0;
        // "Update" pill when a new version is ready.
        if self.update.is_some() || self.update_manual.is_some() {
            let label = "Update";
            let w = text_width(ui, label, bold(12.5)) + 26.0;
            let r = Rect::from_min_size(pos2(rx - w, cy - 14.0), vec2(w, 28.0));
            rx -= w + 6.0;
            let resp = ui.interact(r, Id::new("update-pill"), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new("update-pill-h"), resp.hovered(), 0.25);
            let r = r.expand(1.5 * h);
            crate::paint::accent_fill(&p, r, 14, pal);
            p.text(r.center(), Align2::CENTER_CENTER, label, bold(12.5), pal.ink);
            let tip = match (&self.update, &self.update_manual) {
                (Some(u), _) => format!("Install Goat {} now (Goat restarts)", u.version),
                (_, Some(v)) => format!("Download Goat {v}"),
                _ => String::new(),
            };
            if resp.on_hover_text(tip).clicked() {
                cmds.push(Cmd::InstallUpdate);
            }
        }
        // Profile logo (only when there's more than one profile): click for Profiles.
        if let Some((glyph, color)) = self.profile_badge.clone() {
            let r = btn(rx - 32.0);
            rx -= 36.0;
            let resp = ui.interact(r, Id::new("profile-badge"), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new("profile-badge-h"), resp.hovered(), 0.15);
            let c = r.center();
            p.circle_filled(c, 12.0 + 1.0 * h, color);
            if h > 0.0 {
                p.circle_stroke(c, 14.5, Stroke::new(1.5, alpha(color, 0.5 * h)));
            }
            p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, &glyph, bold(13.0), Color32::from_rgb(0x10, 0x12, 0x14));
            let name = if crate::store::launch().profile.is_empty() { "Default".to_string() } else { crate::store::launch().profile.clone() };
            if resp.on_hover_text(format!("Profile: {name}")).clicked() {
                cmds.push(Cmd::OpenUrl(format!("{}/settings#profiles", crate::INTERNAL_BASE), true));
            }
        }
        // Shield: icon + count.
        let ab = self.sh.adblock.clone();
        let host = self.active_tab().map(|t| crate::adblock::host_of(&t.page_url.lock().unwrap())).unwrap_or_default();
        let count = self.active_tab().map(|t| t.blocked.load(Ordering::Relaxed)).unwrap_or(0);
        let (shield_label, shield_on) = if !ab.enabled() {
            ("off".to_string(), false)
        } else if web && ab.is_allowed(&host) {
            ("off".to_string(), false)
        } else if !ab.ready() {
            ("…".to_string(), true)
        } else {
            (count.to_string(), true)
        };
        let sw = 16.0 + 5.0 + text_width(ui, &shield_label, bold(13.0)) + 20.0;
        let shield = Rect::from_min_size(pos2(rx - sw, cy - 16.0), vec2(sw, 32.0));
        rx -= sw + 4.0;
        // Tor pill.
        let tor_state = self.sh.tor.state();
        let show_tor = tor || tor_state.phase != "off";
        let tor_label = match tor_state.phase.as_str() {
            "ready" => "Tor".to_string(),
            "error" => "Tor !".to_string(),
            "off" => "Tor".to_string(),
            _ => format!("Tor {}%", tor_state.progress),
        };
        let torw = if show_tor { 9.0 + 14.0 + 5.0 + text_width(ui, &tor_label, bold(12.0)) + 9.0 } else { 0.0 };
        let torr = Rect::from_min_size(pos2(rx - torw, cy - 14.0), vec2(torw, 28.0));
        if show_tor {
            rx -= torw + 4.0;
        }
        // Extensions button (extensions don't run in private or Tor tabs).
        let ext = btn(rx - 32.0);
        if !private_like {
            rx -= 36.0;
        }
        // Pinned extension buttons, left of the extensions button.
        let mut pinned_btns: Vec<(Rect, crate::extinstall::ExtInfo)> = Vec::new();
        if !private_like {
            let pins = self.settings().pinned_extensions;
            if !pins.is_empty() {
                let off = self.settings().disabled_extensions;
                for e in crate::extinstall::list().into_iter().filter(|e| e.valid && pins.contains(&e.folder) && !off.contains(&e.folder)) {
                    pinned_btns.push((btn(rx - 32.0), e));
                    rx -= 34.0;
                }
            }
        }
        // Find bar.
        let find_w = 10.0 + 160.0 + 64.0 + 26.0 * 3.0 + 4.0;
        // The find bar glides in, and the address bar narrows smoothly to make room.
        let find_t = anim::tween(&ctx, Id::new("find-open"), if self.find_open { 1.0 } else { 0.0 }, 0.34, anim::spring);
        let findr = Rect::from_min_size(pos2(rx - find_w, cy - 16.0), vec2(find_w, 32.0));
        // The spring overshoots below 0 when closing; never let the address bar grow past its
        // own space (it would slide over pinned extension buttons).
        rx -= (find_w + 6.0) * find_t.max(0.0);

        // Address bar fills the middle.
        let addr = Rect::from_min_max(pos2(x + 6.0, cy - 16.0), pos2(rx - 6.0, cy + 16.0));
        self.addr_rect = addr;
        self.draw_address(ui, addr, pal, anim, cmds, kind, web, internal, &url, &title);

        if find_t > 0.01 {
            let mut sub = ui.new_child(UiBuilder::new().max_rect(findr));
            sub.set_opacity(find_t);
            if !self.find_open {
                sub.disable();
            }
            self.draw_findbar(&mut sub, findr, pal, cmds);
        }
        for (b, e) in pinned_btns {
            let r = ui.interact(b, Id::new(("pinext", &e.folder)), Sense::click());
            let open = self.ext_popup.as_ref().is_some_and(|p| p.1 == e.folder);
            let h = anim::hover(&ctx, Id::new(("pinext-h", &e.folder)), r.hovered() || open, 0.12);
            if h > 0.0 {
                ui.painter().rect_filled(b, 8, alpha(pal.bg3, h));
            }
            let ic = Rect::from_center_size(b.center(), Vec2::splat(18.0));
            match self.ext_icon(&ctx, &e) {
                Some(tex) => {
                    ui.painter().image(tex.id(), ic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    ui.painter().rect_filled(ic, 5, alpha(pal.accent, 0.18));
                    let letter = e.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                    ui.painter().text(ic.center(), Align2::CENTER_CENTER, letter, bold(11.0), pal.accent);
                }
            }
            if r.on_hover_text(&e.name).clicked() {
                cmds.push(Cmd::ExtOpen(e.folder.clone(), b));
            }
        }
        if !private_like {
            let r = icon_button(ui, ext, Id::new("extbtn"), icons::PUZZLE, pal, true, 1.4).on_hover_text("Extensions");
            if r.clicked() {
                cmds.push(Cmd::Popup(Popup::Extensions, to_screen(&ctx, pos2(ext.right(), ext.bottom() + 4.0))));
            }
        }
        if show_tor {
            let r = ui.interact(torr, Id::new("torbtn"), Sense::click());
            let (bg, fg) = match tor_state.phase.as_str() {
                "ready" => (alpha(Color32::from_rgb(0x9d, 0x4e, 0xdd), if r.hovered() { 0.32 } else { 0.20 }), mix(PURPLE, pal.text, 0.75)),
                "error" => (alpha(pal.danger, 0.16), pal.danger),
                "off" => (if r.hovered() { pal.bg3 } else { Color32::TRANSPARENT }, pal.muted),
                _ => (pal.bg3, pal.muted),
            };
            let tor_in = anim::hover(&ctx, Id::new("tor-in"), true, 0.3);
            let p = { let mut q = p.clone(); q.set_opacity(tor_in); q };
            p.rect_filled(torr, 8, bg);
            let pulse = if matches!(tor_state.phase.as_str(), "starting" | "downloading") && anim {
                ctx.request_repaint();
                0.35 + 0.65 * ((now(&ctx) * 5.2).sin() * 0.5 + 0.5) as f32
            } else {
                1.0
            };
            icons::draw(&p, Rect::from_min_size(pos2(torr.left() + 9.0, cy - 7.0), Vec2::splat(14.0)), icons::ONION, alpha(fg, pulse), 1.3);
            p.text(pos2(torr.left() + 28.0, cy), Align2::LEFT_CENTER, &tor_label, bold(12.0), fg);
            // In a Tor window it shows the site's circuit (like Tor Browser); elsewhere it opens a Tor window.
            let in_tor = self.window_kind == TabKind::Tor;
            let tip = if in_tor { format!("Tor: {}. Click to see this site's circuit.", tor_state.message) } else { format!("Tor: {}. Click for a new Tor tab.", tor_state.message) };
            if r.on_hover_text(tip).clicked() {
                cmds.push(if in_tor { Cmd::ShowTorCircuit(to_screen(&ctx, pos2(torr.right(), torr.bottom() + 4.0))) } else { Cmd::NewTorTab });
            }
        }
        // Shield.
        {
            let r = ui.interact(shield, Id::new("shield"), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new("shield-h"), r.hovered(), 0.12);
            if h > 0.0 {
                p.rect_filled(shield, 8, alpha(pal.bg3, h));
            }
            let on_t = anim::hover(&ctx, Id::new("shield-on"), shield_on, 0.25);
            let col = mix(pal.accent, pal.muted, on_t);
            // Count highlight when it goes up.
            let bump_id = Id::new("shield-bump");
            let last: u32 = ctx.data(|d| d.get_temp(bump_id).unwrap_or(0));
            if count > last && anim {
                let t = now(&ctx); // never call into ctx from inside a ctx closure: egui's lock isn't re-entrant
                ctx.data_mut(|d| d.insert_temp(Id::new("shield-bump-t"), t));
            }
            ctx.data_mut(|d| d.insert_temp(bump_id, count));
            let bt: f64 = ctx.data(|d| d.get_temp(Id::new("shield-bump-t")).unwrap_or(-10.0));
            let k = anim::since(&ctx, bt, 0.26);
            // Goat's bump: the shield grows to 1.35× at 40%, then springs back.
            let glow = if k < 1.0 { 1.0 - anim::out_cubic(k) } else { 0.0 };
            let grow = anim::tween(&ctx, Id::new("shield-grow"), if r.hovered() { 1.22 } else { 1.0 }, 0.32, anim::ease_bounce);
            let ir = Rect::from_center_size(pos2(shield.left() + 18.0, cy), Vec2::splat(16.0 * anim::bump(k, 1.35) * grow));
            let fill = if on_t > 0.01 { Some(alpha(pal.accent, 0.30 * on_t)) } else { None };
            icons::draw_filled(&p, ir, icons::SHIELD, col, 1.6, fill);
            p.text(pos2(ir.right() + 5.0, cy), Align2::LEFT_CENTER, &shield_label, bold(13.0), mix(pal.accent2, col, glow * 0.8));
            let tip = if !ab.enabled() {
                "Ad blocker is off. Click for options.".to_string()
            } else if !shield_on {
                "Ad blocker paused on this site. Click for options.".to_string()
            } else {
                format!("{count} ads and trackers blocked on this page. Click to change blocking.")
            };
            if r.on_hover_text(tip).clicked() {
                cmds.push(Cmd::Popup(Popup::Shield, to_screen(&ctx, pos2(shield.right(), shield.bottom() + 2.0))));
            }
        }
        // Menu (cog).
        {
            let r = ui.interact(cog, Id::new("cog"), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new("cog-h"), r.hovered(), 0.32);
            if h > 0.0 {
                p.rect_filled(cog, 8, alpha(pal.bg3, h.min(1.0)));
            }
            // A quarter turn with a little overshoot, then it settles (and turns back on leave).
            let ang = if anim { anim::tween(&ctx, Id::new("cog-rot"), if r.hovered() { std::f32::consts::FRAC_PI_2 } else { 0.0 }, 0.32, anim::ease_bounce) } else { 0.0 };
            icons::draw_rot(&p, Rect::from_center_size(cog.center(), Vec2::splat(17.0)), icons::COG, pal.text, 1.3, ang);
            if r.on_hover_text("Menu").clicked() {
                cmds.push(Cmd::Popup(Popup::Menu, to_screen(&ctx, pos2(cog.right(), cog.bottom() + 2.0))));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_address(&mut self, ui: &mut Ui, rect: Rect, pal: &Palette, anim: bool, cmds: &mut Vec<Cmd>, kind: TabKind, web: bool, internal: bool, url: &str, _title: &str) {
        let tor = kind == TabKind::Tor;
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let id = Id::new("addr");
        let focused = ctx.memory(|m| m.has_focus(id));
        let ring = crate::anim::hover(&ctx, id.with("ring"), focused, 0.2);

        // Field.
        if ring > 0.0 {
            p.rect_stroke(rect.expand(1.5), 18, Stroke::new(3.0, alpha(pal.accent, 0.22 * ring)), StrokeKind::Outside);
        }
        p.rect_filled(rect, 16, pal.bg);
        let border = match kind {
            TabKind::Tor => mix(PURPLE, pal.line, 0.6),
            TabKind::Private => mix(PV, pal.line, 0.55),
            TabKind::Normal => pal.line,
        };
        p.rect_stroke(rect, 16, Stroke::new(1.0, mix(pal.accent, border, ring)), StrokeKind::Inside);

        let cy = rect.center().y;
        let mut x = rect.left() + 12.0;
        // Goat's "Private" / "Tor" pill.
        if let Some(c) = kind_color(kind) {
            let label = if tor { "Tor" } else { "Private" };
            let w = text_width(ui, label, bold(11.0)) + 16.0;
            let b = Rect::from_min_size(pos2(x - 4.0, cy - 9.0), vec2(w, 18.0));
            p.rect_filled(b, 9, alpha(c, 0.22));
            let fg = if tor { Color32::from_rgb(0xd3, 0xb0, 0xff) } else { Color32::from_rgb(0xc4, 0xb5, 0xfd) };
            p.text(b.center(), Align2::CENTER_CENTER, label, bold(11.0), fg);
            x += w + 4.0;
        }
        // "No GPU" pill in a no-hardware-acceleration window.
        if crate::store::no_gpu_window() {
            let label = "No GPU";
            let w = text_width(ui, label, bold(11.0)) + 16.0;
            let b = Rect::from_min_size(pos2(x - 4.0, cy - 9.0), vec2(w, 18.0));
            p.rect_filled(b, 9, alpha(pal.muted, 0.22));
            p.text(b.center(), Align2::CENTER_CENTER, label, bold(11.0), pal.text);
            x += w + 4.0;
        }
        // Security indicator.
        if !focused {
            if url.starts_with("https://") {
                icons::draw(&p, Rect::from_min_size(pos2(x, cy - 6.5), Vec2::splat(13.0)), icons::LOCK, pal.accent, 1.6);
                x += 13.0 + 8.0;
            } else if url.starts_with("http://") && !internal {
                icons::draw(&p, Rect::from_min_size(pos2(x, cy - 6.5), Vec2::splat(13.0)), icons::WARN, pal.warn, 1.6);
                let r = p.text(pos2(x + 17.0, cy), Align2::LEFT_CENTER, "Not secure", font(11.0), pal.warn);
                x = r.right() + 8.0;
            }
        }

        // Right side: "Add to Goat" on store pages, then the star.
        let mut right = rect.right() - 6.0;
        let zoom = self.active_tab().map(|t| t.zoom).unwrap_or(1.0);
        let zoom_btn = (zoom - 1.0).abs() > 0.001;
        if web {
            let star = Rect::from_min_size(pos2(right - 26.0, cy - 13.0), Vec2::splat(26.0));
            right -= 30.0;
            let bookmarked = self.sh.store.lock().unwrap().is_bookmarked(url);
            let r = ui.interact(star, id.with("star"), Sense::click());
            if r.hovered() {
                p.rect_filled(star, 6, pal.bg3);
            }
            let col = if bookmarked { pal.accent } else if r.hovered() { pal.text } else { pal.muted };
            // Pop when it becomes bookmarked.
            let pop_id = id.with("star-pop");
            let was: bool = ctx.data(|d| d.get_temp(pop_id).unwrap_or(bookmarked));
            if bookmarked && !was && anim {
                let t = now(&ctx); // not inside the closure: egui's lock isn't re-entrant
                ctx.data_mut(|d| d.insert_temp(pop_id.with("t"), t));
            }
            ctx.data_mut(|d| d.insert_temp(pop_id, bookmarked));
            let t0: f64 = ctx.data(|d| d.get_temp(pop_id.with("t")).unwrap_or(-10.0));
            let k = anim::since(&ctx, t0, 0.42);
            // Goat's star-pop: grows to 1.45× and springs back when you bookmark.
            let ir = Rect::from_center_size(star.center(), Vec2::splat(15.0 * anim::bump(k, 1.45)));
            // Fill fades in/out smoothly.
            let fill = anim::hover(&ctx, id.with("star-fill"), bookmarked, 0.25);
            icons::draw_filled(&p, ir, icons::STAR, col, 1.6, if fill > 0.01 { Some(alpha(pal.accent, fill)) } else { None });
            if r.on_hover_text(if bookmarked { "Remove bookmark (Ctrl+D)" } else { "Bookmark this page (Ctrl+D)" }).clicked() {
                cmds.push(Cmd::ToggleBookmark);
            }
            if !kind.is_private_like() && crate::extinstall::parse_store_url(url).is_some() {
                let label = "Add to Goat";
                let w = 10.0 + 14.0 + 6.0 + text_width(ui, label, bold(12.0)) + 12.0;
                let chip = Rect::from_min_size(pos2(right - w, cy - 12.0), vec2(w, 24.0));
                right -= w + 6.0;
                let r = ui.interact(chip, id.with("addext"), Sense::click());
                paint::accent_fill(&p, chip, 12, pal);
                if r.hovered() {
                    p.rect_filled(chip, 12, alpha(Color32::WHITE, 0.12));
                }
                icons::draw(&p, Rect::from_min_size(pos2(chip.left() + 10.0, cy - 7.0), Vec2::splat(14.0)), icons::PUZZLE, pal.ink, 1.5);
                p.text(pos2(chip.left() + 30.0, cy), Align2::LEFT_CENTER, label, bold(12.0), pal.ink);
                if r.on_hover_text("Install this extension in Goat").clicked() {
                    cmds.push(Cmd::InstallExtension);
                }
            }
        }

        // Speaker while the tab plays sound (or isn't at 100% volume): click for its volume slider.
        let (playing, vol) = self.active_tab().map(|t| (t.playing, t.volume)).unwrap_or((false, 1.0));
        if (playing || (vol - 1.0).abs() > 0.001) && !focused {
            let sr = Rect::from_min_size(pos2(right - 26.0, cy - 13.0), Vec2::splat(26.0));
            right -= 30.0;
            let r = ui.interact(sr, id.with("vol"), Sense::click());
            let open = matches!(self.popup, Some((Popup::Volume, _)));
            if r.hovered() || open {
                p.rect_filled(sr, 6, pal.bg3);
            }
            let col = if (vol - 1.0).abs() > 0.001 { pal.accent } else if r.hovered() { pal.text } else { pal.muted };
            icons::draw(&p, Rect::from_center_size(sr.center(), Vec2::splat(15.0)), if vol <= 0.001 { icons::SPEAKER_OFF } else { icons::SPEAKER }, col, 1.5);
            if r.on_hover_text(format!("Tab volume: {}%", (vol * 100.0).round())).clicked() {
                cmds.push(Cmd::Popup(Popup::Volume, to_screen(&ctx, pos2(sr.right() + 12.0, rect.bottom() + 4.0))));
            }
        }
        // Magnifier while the page isn't at 100%: click for the zoom bubble.
        if zoom_btn && !focused {
            let zr = Rect::from_min_size(pos2(right - 26.0, cy - 13.0), Vec2::splat(26.0));
            right -= 30.0;
            let r = ui.interact(zr, id.with("zoom"), Sense::click());
            if r.hovered() {
                p.rect_filled(zr, 6, pal.bg3);
            }
            let zoom_in = zoom > 1.0;
            let ir = Rect::from_center_size(zr.center(), Vec2::splat(15.0));
            icons::draw(&p, ir, icons::ZOOM, if r.hovered() { pal.text } else { pal.muted }, 1.5);
            // + or − inside the lens.
            let c = pos2(ir.left() + ir.width() * 7.0 / 16.0, ir.top() + ir.height() * 7.0 / 16.0);
            let col = if r.hovered() { pal.text } else { pal.muted };
            p.line_segment([pos2(c.x - 2.2, c.y), pos2(c.x + 2.2, c.y)], Stroke::new(1.3, col));
            if zoom_in {
                p.line_segment([pos2(c.x, c.y - 2.2), pos2(c.x, c.y + 2.2)], Stroke::new(1.3, col));
            }
            if r.on_hover_text(format!("Zoom: {}%", (zoom * 100.0).round())).clicked() {
                cmds.push(Cmd::ShowZoom);
            }
        }

        // The text field.
        if !self.addr_editing {
            self.addr = crate::pages::display_url(url);
        }
        let mut select_all = false;
        // Back to the address bar after a click in the suggestions: keep the typed text as it is.
        let mut keep_text = false;
        if self.addr_refocus {
            self.addr_refocus = false;
            keep_text = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.memory_mut(|m| m.request_focus(id));
            self.addr_editing = true;
            ctx.data_mut(|d| d.remove::<f64>(Id::new("sugg-close-at")));
            ctx.data_mut(|d| d.remove::<f64>(Id::new("unfocused-at")));
        }
        if self.addr_focus_req {
            self.addr_focus_req = false;
            ctx.memory_mut(|m| m.request_focus(id));
            if let Some(t) = self.active_tab() {
                let _ = t.webview.focus_parent();
            }
            select_all = true;
        }
        let text_rect = Rect::from_min_max(pos2(x, rect.top() + 2.0), pos2(right - 4.0, rect.bottom() - 2.0));
        let hint = match kind {
            TabKind::Tor => "Search through Tor or enter address",
            TabKind::Private => "Search privately or enter address",
            TabKind::Normal => "Search or enter address",
        };
        let before = self.addr.clone();
        let mut out = ui
            .scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
                TextEdit::singleline(&mut self.addr)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .font(font(13.5))
                    .text_color(pal.text)
                    .hint_text(egui::RichText::new(hint).color(pal.muted).font(font(13.5)))
                    .desired_width(text_rect.width())
                    .vertical_align(egui::Align::Center)
                    .min_size(vec2(text_rect.width(), text_rect.height()))
                    .show(ui)
            })
            .inner;
        if keep_text || ctx.data(|d| d.get_temp::<bool>(Id::new("addr-keep-text")).unwrap_or(false)) {
            // Cursor at the end, nothing selected.
            let n = self.addr.chars().count();
            out.state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(n))));
            out.state.store(&ctx, id);
            // Keep this up until the focus has actually come back.
            let still = !out.response.gained_focus() && !focused;
            ctx.data_mut(|d| {
                if still {
                    d.insert_temp(Id::new("addr-keep-text"), true);
                } else {
                    d.remove::<bool>(Id::new("addr-keep-text"));
                }
            });
        } else if select_all || (out.response.gained_focus() && !self.addr_editing) {
            self.addr_editing = true;
            let n = self.addr.chars().count();
            out.state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(n))));
            out.state.store(&ctx, id);
            // Fresh focus (e.g. a new tab): no suggestions until you type. The old list from the
            // last edit would otherwise flash up and vanish.
            self.sugg_for = self.addr.clone();
            self.sugg.clear();
            self.sugg_sel = None;
        }
        if out.response.changed() || before != self.addr {
            self.addr_editing = true;
        }

        // Suggestions from history and bookmarks.
        if focused && self.addr_editing {
            if self.addr != self.sugg_for {
                self.sugg_for = self.addr.clone();
                self.sugg = self.sh.store.lock().unwrap().suggest(&self.addr, 8);
                // The top suggestion starts selected: Enter goes there. The "search" row above it
                // (sugg_sel None) uses the typed text instead.
                self.sugg_sel = if self.sugg.is_empty() { None } else { Some(0) };
            }
            let n = self.sugg.len();
            // The list shows whenever something is typed: the search row is always there.
            if !self.addr.trim().is_empty() {
                // Positions: None (search row), then 0..n.
                if n > 0 && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                    self.sugg_sel = match self.sugg_sel {
                        None => Some(0),
                        Some(s) if s + 1 < n => Some(s + 1),
                        Some(_) => None,
                    };
                }
                if n > 0 && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                    self.sugg_sel = match self.sugg_sel {
                        None => Some(n - 1),
                        Some(0) => None,
                        Some(s) => Some(s - 1),
                    };
                }
                if !matches!(self.popup, Some((Popup::Suggest, _)) | Some((Popup::Menu | Popup::Shield | Popup::Extensions, _))) {
                    self.popup = Some((Popup::Suggest, to_screen(&ctx, pos2(rect.left(), rect.bottom() + 4.0))));
                }
            } else if matches!(self.popup, Some((Popup::Suggest, _))) {
                self.popup = None;
            }
        }

        if out.response.lost_focus() {
            let (enter, esc) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            let ctrl = ctx.input(|i| i.modifiers.command);
            if enter {
                let target = match self.sugg_sel.and_then(|s| self.sugg.get(s)).filter(|_| !ctrl) {
                    Some((u, _)) => u.clone(),
                    None if ctrl && !self.addr.contains(['.', ' ', ':', '/']) => format!("www.{}.com", self.addr.trim()),
                    None => self.addr.clone(),
                };
                cmds.push(Cmd::Navigate(target));
                self.addr_editing = false;
                if matches!(self.popup, Some((Popup::Suggest, _))) {
                    self.popup = None;
                }
            } else if esc {
                self.addr_editing = false;
                if matches!(self.popup, Some((Popup::Suggest, _))) {
                    self.popup = None;
                }
                cmds.push(Cmd::FocusContent);
            } else {
                // Clicked elsewhere: leave suggestions up briefly so a click on one still lands.
                let t = now(&ctx);
                ctx.data_mut(|d| d.insert_temp(Id::new("sugg-close-at"), t + 0.3));
            }
        }
        // Suggestions only live while the address bar is being edited.
        if !self.addr_editing && matches!(self.popup, Some((Popup::Suggest, _))) {
            self.popup = None;
        }
        if !focused && matches!(self.popup, Some((Popup::Suggest, _))) {
            let at: f64 = ctx.data(|d| d.get_temp(Id::new("sugg-close-at")).unwrap_or(0.0));
            if now(&ctx) > at {
                self.popup = None;
                self.addr_editing = false;
            } else {
                ctx.request_repaint();
            }
        }
    }

    fn draw_findbar(&mut self, ui: &mut Ui, r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        p.rect_filled(r, 16, pal.bg);
        p.rect_stroke(r, 16, Stroke::new(1.0, pal.line), StrokeKind::Inside);
        let id = Id::new("findq");
        if self.find_focus_req {
            self.find_focus_req = false;
            ctx.memory_mut(|m| m.request_focus(id));
            if let Some(t) = self.active_tab() {
                let _ = t.webview.focus_parent();
            }
        }
        let tr = Rect::from_min_size(pos2(r.left() + 10.0, r.top() + 2.0), vec2(160.0, r.height() - 4.0));
        let out = ui
            .scope_builder(UiBuilder::new().max_rect(tr), |ui| {
                TextEdit::singleline(&mut self.find_q)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .font(font(13.0))
                    .text_color(pal.text)
                    .hint_text(egui::RichText::new("Find in page").color(pal.muted))
                    .vertical_align(egui::Align::Center)
                    .min_size(tr.size())
                    .desired_width(tr.width())
                    .show(ui)
            })
            .inner;
        if out.response.changed() {
            cmds.push(Cmd::Find(false));
        }
        if out.response.lost_focus() {
            let (enter, esc, shift) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), i.modifiers.shift));
            if enter {
                cmds.push(Cmd::Find(shift));
                ctx.memory_mut(|m| m.request_focus(id));
            } else if esc {
                cmds.push(Cmd::FindOpen(false));
            }
        }
        let count = match (self.find_q.is_empty(), self.find_count) {
            (true, _) | (_, None) => String::new(),
            (false, Some(1)) => "1 match".into(),
            (false, Some(n)) => format!("{n} matches"),
        };
        p.text(pos2(tr.right() + 60.0, r.center().y), Align2::RIGHT_CENTER, count, font(12.0), pal.muted);
        let mut bx = r.right() - 4.0 - 26.0 * 3.0;
        for (icon, cmd, tip) in [(icons::UP, Cmd::Find(true), "Previous"), (icons::DOWN, Cmd::Find(false), "Next"), (icons::CLOSE, Cmd::FindOpen(false), "Close (Esc)")] {
            let b = Rect::from_min_size(pos2(bx, r.center().y - 13.0), Vec2::splat(26.0));
            if icon_button(ui, b, Id::new(("findbtn", tip)), icon, pal, true, 1.6).on_hover_text(tip).clicked() {
                cmds.push(cmd);
            }
            bx += 26.0;
        }
    }

    // ------------------------------------------------------------------ bookmarks bar

    fn draw_bookmarks(&mut self, ui: &mut Ui, bar: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().with_clip_rect(bar);
        p.rect_filled(bar, 0, nav_bg(pal));
        p.line_segment([bar.left_bottom() - vec2(0.0, 0.5), bar.right_bottom() - vec2(0.0, 0.5)], Stroke::new(1.0, pal.line));
        let bms: Vec<(u64, String, String)> = self.sh.store.lock().unwrap().data.bookmarks.iter().map(|b| (b.id, b.title.clone(), b.url.clone())).collect();
        let cy = bar.center().y;
        if bms.is_empty() {
            p.text(pos2(bar.left() + 14.0, cy), Align2::LEFT_CENTER, "Press Ctrl+D or click the star in the address bar to bookmark a page.", font(12.0), pal.muted);
            return;
        }
        let mut x = bar.left() + 8.0;
        for (id, title, url) in bms {
            let tw = text_width(ui, &title, font(12.5)).min(180.0 - 6.0 - 16.0 - 6.0 - 9.0);
            let w = 6.0 + 16.0 + 6.0 + tw + 9.0;
            if x + w > bar.right() - 8.0 {
                break;
            }
            let r = Rect::from_min_size(pos2(x, cy - 13.0), vec2(w, 26.0));
            x += w + 2.0;
            let resp = ui.interact(r, Id::new(("bm", id)), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new(("bmh", id)), resp.hovered(), 0.12);
            if h > 0.0 {
                p.rect_filled(r, 7, alpha(pal.bg3, h));
            }
            let ic = Rect::from_min_size(pos2(r.left() + 6.0, cy - 8.0), Vec2::splat(16.0));
            if let Some(tex) = self.site_icon(&url) {
                p.image(tex, ic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                p.rect_filled(ic, 4, pal.bg3);
                p.text(ic.center(), Align2::CENTER_CENTER, name_letter(&title, &url), bold(9.5), pal.muted);
            }
            text_elided(&p, pos2(ic.right() + 6.0, cy), &title, font(12.5), pal.text, tw + 1.0, Align2::LEFT_CENTER);
            let ctrl = ctx.input(|i| i.modifiers.command);
            if resp.clicked() {
                cmds.push(Cmd::OpenUrl(url.clone(), ctrl));
            }
            if resp.middle_clicked() {
                cmds.push(Cmd::OpenUrl(url.clone(), true));
            }
            if resp.secondary_clicked() {
                let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(r.left_bottom());
                cmds.push(Cmd::Popup(Popup::BmMenu(id), to_screen(&ctx, at)));
            }
            resp.on_hover_text(format!("{title}\n{url}"));
        }
    }

    // ------------------------------------------------------------------ toast

    /// The download bar: one card per download (newest first), "Show all" and close on the right.
    fn draw_download_bar(&mut self, ui: &mut Ui, bar: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        p.rect_filled(bar, 0, pal.bg2);
        p.line_segment([bar.left_top(), bar.right_top()], Stroke::new(1.0, pal.line));
        // Right end: Show all, close.
        let close = Rect::from_center_size(pos2(bar.right() - 20.0, bar.center().y), Vec2::splat(26.0));
        let cr = ui.interact(close, Id::new("dlbar-close"), Sense::click()).on_hover_text("Close the download bar");
        if cr.hovered() {
            p.rect_filled(close, 7, pal.bg3);
        }
        icons::draw(&p, Rect::from_center_size(close.center(), Vec2::splat(14.0)), icons::CLOSE, pal.muted, 1.5);
        if cr.clicked() {
            cmds.push(Cmd::DownloadBarClose);
        }
        let all_w = text_width(ui, "Show all", font(12.0)) + 20.0;
        let all = Rect::from_center_size(pos2(close.left() - 6.0 - all_w / 2.0, bar.center().y), vec2(all_w, 26.0));
        let ar = ui.interact(all, Id::new("dlbar-all"), Sense::click());
        p.rect_filled(all, 7, if ar.hovered() { pal.bg3 } else { alpha(pal.bg3, 0.0) });
        p.rect_stroke(all, 7, Stroke::new(1.0, pal.line), StrokeKind::Inside);
        p.text(all.center(), Align2::CENTER_CENTER, "Show all", font(12.0), pal.text);
        if ar.clicked() {
            cmds.push(Cmd::OpenPage("downloads"));
        }
        let mb = |b: i64| -> String {
            let b = b.max(0) as f64;
            if b >= 1e9 { format!("{:.2} GB", b / 1e9) } else if b >= 1e6 { format!("{:.1} MB", b / 1e6) } else { format!("{:.0} KB", (b / 1e3).max(1.0)) }
        };
        let mut x = bar.left() + 8.0;
        let card_w = 280.0f32;
        let mut animating = false;
        let t_now = now(&ctx);
        for d in &self.dl_items {
            if x + card_w > all.left() - 8.0 {
                break;
            }
            let r = Rect::from_min_size(pos2(x, bar.top() + 5.0), vec2(card_w, bar.height() - 10.0));
            x += card_w + 6.0;
            let id = Id::new(("dl", d.id));
            let resp = ui.interact(r, id, Sense::click());
            let h = anim::hover(&ctx, id.with("h"), resp.hovered(), 0.12);
            p.rect_filled(r, 9, alpha(pal.bg3, 0.5 + 0.5 * h));
            // Born: slide up into place.
            let born = ((t_now - d.born) / 0.35).clamp(0.0, 1.0) as f32;
            if born < 1.0 {
                animating = true;
            }
            let mut pp = p.clone();
            pp.set_opacity(anim::out_cubic(born));
            let r = r.translate(vec2(0.0, 8.0 * (1.0 - anim::out_cubic(born))));
            let name = std::path::Path::new(&d.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Download".into());
            let frac = if d.total > 0 { (d.received as f64 / d.total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
            // Icon: a progress ring while running, a tick when done.
            let ic = Rect::from_center_size(pos2(r.left() + 20.0, r.center().y), Vec2::splat(22.0));
            let (icon, col) = match &d.state {
                DlState::Done => (icons::CHECK, pal.accent),
                DlState::Failed { .. } | DlState::Cancelled => (icons::WARN, pal.muted),
                DlState::Paused => (icons::PAUSE, pal.muted),
                DlState::Running => (icons::DOWNLOAD, pal.accent),
            };
            if matches!(d.state, DlState::Running | DlState::Paused) {
                pp.circle_stroke(ic.center(), 10.0, Stroke::new(2.0, pal.line));
                let sweep = if d.total > 0 { frac * std::f32::consts::TAU } else { std::f32::consts::FRAC_PI_2 };
                let a0 = if d.total > 0 { -std::f32::consts::FRAC_PI_2 } else { (t_now * 4.0) as f32 };
                if d.total <= 0 && d.state == DlState::Running {
                    animating = true;
                }
                let n = ((sweep / std::f32::consts::TAU) * 40.0).ceil().max(2.0) as usize;
                let pts: Vec<Pos2> = (0..=n).map(|k| ic.center() + Vec2::angled(a0 + sweep * k as f32 / n as f32) * 10.0).collect();
                pp.add(egui::Shape::line(pts, Stroke::new(2.0, if d.state == DlState::Paused { pal.muted } else { pal.accent })));
                icons::draw(&pp, Rect::from_center_size(ic.center(), Vec2::splat(11.0)), icon, col, 1.5);
            } else {
                icons::draw(&pp, Rect::from_center_size(ic.center(), Vec2::splat(15.0)), icon, col, 1.6);
            }
            let status = match &d.state {
                DlState::Running => {
                    let size = if d.total > 0 { format!("{} of {}", mb(d.received), mb(d.total)) } else { mb(d.received) };
                    if d.speed > 1.0 {
                        let left = if d.total > 0 { ((d.total - d.received).max(0) as f64 / d.speed) as u64 } else { 0 };
                        let left = if d.total <= 0 { String::new() } else if left >= 3600 { format!(" · {} h {} min left", left / 3600, left % 3600 / 60) } else if left >= 60 { format!(" · {} min left", (left + 30) / 60) } else { format!(" · {left} s left") };
                        format!("{size} · {}/s{left}", mb(d.speed as i64))
                    } else {
                        size
                    }
                }
                DlState::Paused => format!("Paused · {}{}", mb(d.received), if d.total > 0 { format!(" of {}", mb(d.total)) } else { String::new() }),
                DlState::Done => format!("{} · Done", mb(d.total.max(d.received))),
                DlState::Cancelled => "Cancelled".into(),
                DlState::Failed { reason, .. } => reason.clone(),
            };
            // Buttons on hover.
            let mut btns: Vec<(&'static str, Icon, &str)> = Vec::new();
            match &d.state {
                DlState::Running => {
                    btns.push(("pause", icons::PAUSE, "Pause"));
                    btns.push(("cancel", icons::CLOSE, "Cancel"));
                }
                DlState::Paused => {
                    btns.push(("resume", icons::PLAY, "Resume"));
                    btns.push(("cancel", icons::CLOSE, "Cancel"));
                }
                DlState::Done => {
                    if cfg!(windows) {
                        btns.push(("folder", icons::FOLDER, "Show in folder"));
                    }
                    btns.push(("remove", icons::CLOSE, "Remove from bar"));
                }
                DlState::Failed { can_resume, .. } => {
                    if *can_resume {
                        btns.push(("resume", icons::RELOAD, "Try again"));
                    }
                    btns.push(("remove", icons::CLOSE, "Remove from bar"));
                }
                DlState::Cancelled => btns.push(("remove", icons::CLOSE, "Remove from bar")),
            }
            let bw = if h > 0.01 { btns.len() as f32 * 26.0 } else { 0.0 };
            let tx = ic.right() + 10.0;
            let tw = r.right() - 8.0 - bw - tx;
            text_elided(&pp, pos2(tx, r.center().y - 8.0), &name, bold(12.0), if matches!(d.state, DlState::Cancelled) { pal.muted } else { pal.text }, tw, Align2::LEFT_CENTER);
            let scol = if matches!(d.state, DlState::Failed { .. }) { pal.danger } else { pal.muted };
            text_elided(&pp, pos2(tx, r.center().y + 8.0), &status, font(11.0), scol, tw, Align2::LEFT_CENTER);
            let mut on_button = false;
            if h > 0.01 {
                let mut bx = r.right() - 6.0 - 24.0;
                for (k, (what, icon, tip)) in btns.iter().enumerate().rev() {
                    let b = Rect::from_min_size(pos2(bx, r.center().y - 12.0), Vec2::splat(24.0));
                    bx -= 26.0;
                    let br = ui.interact(b, id.with(("b", k)), Sense::click()).on_hover_text(*tip);
                    if br.hovered() {
                        on_button = true;
                        pp.rect_filled(b, 6, pal.bg);
                    }
                    icons::draw(&pp, Rect::from_center_size(b.center(), Vec2::splat(13.0)), icon, alpha(pal.text, h), 1.5);
                    if br.clicked() {
                        cmds.push(Cmd::Download(d.id, what));
                    }
                }
            }
            // Click the card: open the file once it's there.
            if resp.clicked() && !on_button && d.state == DlState::Done {
                cmds.push(Cmd::Download(d.id, "open"));
            }
            if resp.hovered() && d.state == DlState::Done && !on_button {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        if animating {
            ctx.request_repaint();
        }
    }

    fn draw_toast(&mut self, ui: &mut Ui, full: Rect, pal: &Palette, anim: bool, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let t = now(&ctx);
        let Some(toast) = &self.toast else { return };
        if t > toast.until {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
        let tk = if anim { ((t - toast.born) / 0.4) as f32 } else { 1.0 };
        let e = anim::spring(tk);
        // Fade out over the last 0.2s.
        let fade = (((toast.until - t) / 0.2) as f32).clamp(0.0, 1.0);
        if e < 1.0 {
            ctx.request_repaint();
        }
        let text = toast.text.clone();
        let action = toast.action.clone();
        let tw = text_width(ui, &text, font(12.0)).min(full.width() * 0.5);
        let aw = action.as_ref().map(|(l, _)| text_width(ui, l, bold(12.0)) + 22.0).unwrap_or(0.0);
        let w = 24.0 + tw + aw;
        let r0 = Rect::from_min_size(pos2(full.right() - CONTROLS_W - 8.0 - w, full.top() + 5.0), vec2(w, 28.0));
        // Goat's toast-in: drop 8px from above and grow from 96%, springy.
        let r = Rect::from_center_size(r0.center() - vec2(0.0, 8.0 * (1.0 - e)), r0.size() * (0.96 + 0.04 * e));
        let mut p = ui.painter().clone();
        p.set_opacity(anim::out_cubic(tk * 1.6) * fade);
        paint::shadow(&p, r, 8.0, pal.dark);
        p.rect_filled(r, 8, pal.bg3);
        p.rect_stroke(r, 8, Stroke::new(1.0, pal.line), StrokeKind::Inside);
        text_elided(&p, pos2(r.left() + 12.0, r.center().y), &text, font(12.0), pal.text, tw + 1.0, Align2::LEFT_CENTER);
        if let Some((label, cmd)) = action {
            let b = Rect::from_min_size(pos2(r.right() - aw - 4.0, r.top() + 4.0), vec2(aw, 20.0));
            let resp = ui.interact(b, Id::new("toast-act"), Sense::click());
            paint::accent_fill(&p, b, 6, pal);
            p.text(b.center(), Align2::CENTER_CENTER, label, bold(12.0), pal.ink);
            if resp.clicked() {
                cmds.push(cmd);
                self.toast = None;
            }
        }
    }
}
