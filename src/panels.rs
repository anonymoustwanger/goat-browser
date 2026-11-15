//! Pop-up panels, each a small borderless native window floating over the page:
//! the main menu, the Shield (ad blocker) panel, the extensions panel, tab/bookmark
//! context menus and address-bar suggestions. Styled after Goat Browser's panels.

use std::sync::atomic::Ordering;

use egui::{pos2, vec2, Align2, Color32, Id, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, ViewportBuilder, ViewportId};

use crate::crashlog;
use crate::TabKind;
use crate::adblock::Level;
use crate::chrome::{bold, font, text_elided, text_width, PURPLE};
use crate::icons::{self, Icon};
use crate::paint::{self, alpha, mix};
use crate::themes::{self, Palette};
use crate::anim;
use crate::chrome::PV;
use crate::{now, App, Cmd, Popup, INTERNAL_BASE};

// Pop-ups are see-through windows a little bigger than their panel: room for the panel's own
// shadow and for it to pop slightly past full size when it bounces in.
const PAD_X: f32 = 16.0;
const PAD_TOP: f32 = 16.0;
const PAD_BOTTOM: f32 = 20.0;

/// Goat's main window, so pop-ups can belong to it.
pub static MAIN_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Ask Windows 11 for rounded corners on a pop-up window (found by its unique title).
#[cfg(windows)]
pub fn round_corners(title: &str) {
    use windows::core::HSTRING;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    unsafe {
        if let Ok(hwnd) = FindWindowW(None, &HSTRING::from(title)) {
            // Square window: the panel draws its own rounded corners and shadow.
            let pref = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, std::mem::size_of_val(&pref) as u32);
            // No Windows 11 outline around the (see-through) window: only the panel shows.
            let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
            let _ = DwmSetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR, &none as *const _ as *const _, 4);
            // No Windows open/close animation: it slides the window in from elsewhere.
            let off = windows::core::BOOL(1);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &off as *const _ as *const _, std::mem::size_of_val(&off) as u32);
        }
    }
}

/// Rounded corners, no Windows show/hide animation, and a see-through start for the fade.
#[cfg(windows)]
fn prepare_window(title: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetWindowLongPtrW, GWLP_HWNDPARENT};
    round_corners(title);
    unsafe {
        if let Ok(hwnd) = FindWindowW(None, &HSTRING::from(title)) {
            // Owned by Goat's window: stays above Goat, but not above other apps.
            let owner = MAIN_HWND.load(std::sync::atomic::Ordering::Relaxed);
            if owner != 0 {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner);
            }
        }
    }
}


#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Normal,
    Priv,
    Tor,
    Quit,
    Check(bool),
}

struct Item {
    icon: Icon,
    label: String,
    kbd: &'static str,
    cmd: Cmd,
    kind: Kind,
    enabled: bool,
}

fn item(icon: Icon, label: &str, kbd: &'static str, cmd: Cmd) -> Item {
    Item { icon, label: label.into(), kbd, cmd, kind: Kind::Normal, enabled: true }
}

enum Row {
    Item(Item),
    Zoom,
    Themes,
    Sep,
}

const ROW_H: f32 = 31.0;
const ZOOM_H: f32 = 40.0;
const THEMES_H: f32 = 38.0;

fn rows_height(rows: &[Row]) -> f32 {
    6.0 * 2.0
        + rows
            .iter()
            .map(|r| match r {
                Row::Item(_) => ROW_H,
                Row::Zoom => ZOOM_H,
                Row::Themes => THEMES_H,
                Row::Sep => 9.0,
            })
            .sum::<f32>()
}

/// Card background filling the pop-up window.
fn card(p: &Painter, r: Rect, pal: &Palette) {
    paint::shadow(p, r, 12.0, pal.dark);
    p.rect_filled(r, 12, pal.bg2);
    p.rect_stroke(r, 12, Stroke::new(1.0, pal.line), StrokeKind::Inside);
}

/// Goat's toggle switch.
fn switch(ui: &mut Ui, p: &Painter, rect: Rect, id: Id, on: bool, pal: &Palette, enabled: bool) -> egui::Response {
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let t = anim::tween(ui.ctx(), id.with("t"), if on { 1.0 } else { 0.0 }, 0.2, anim::out_cubic);
    let knob_t = anim::tween(ui.ctx(), id.with("k"), if on { 1.0 } else { 0.0 }, 0.32, anim::spring);
    let op = if enabled { 1.0 } else { 0.5 };
    let r = rect.height() / 2.0;
    p.rect_filled(rect, r as u8, alpha(pal.line, op));
    if t > 0.0 {
        let mut pp = p.clone();
        pp.set_opacity(t * op);
        paint::accent_fill(&pp, rect, r as u8, pal);
    }
    let knob = rect.height() - 6.0;
    let x = rect.left() + 3.0 + (rect.width() - 6.0 - knob) * knob_t;
    let kc = Pos2::new(x + knob / 2.0, rect.center().y);
    p.circle_filled(kc + vec2(0.0, 1.0), knob / 2.0, alpha(Color32::BLACK, 0.25));
    p.circle_filled(kc, knob / 2.0, Color32::WHITE);
    resp
}

impl App {
    pub fn draw_popups(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        let t_now = now(ctx);
        let anim = self.animations();
        if let Some((_, _, t0)) = self.popup_closing {
            if !anim || t_now - t0 > 0.09 || self.popup.is_some() {
                self.popup_closing = None;
            }
        }
        let (popup, at, closing) = match (self.popup, self.popup_closing) {
            (Some((p, a)), _) => (p, a, None),
            (None, Some((p, a, t0))) => (p, a, Some(t0)),
            _ => {
                self.popup_ready_for = None; // the next pop-up gets a fresh window
                return;
            }
        };
        let pal = self.palette();

        // Build the content first so the window can be sized to fit.
        let card_w;
        let card_h;
        let mut rows: Vec<Row> = Vec::new();
        match popup {
            Popup::Menu => {
                rows = self.menu_rows();
                card_w = 320.0;
                card_h = rows_height(&rows);
            }
            Popup::TabMenu(id) => {
                rows = vec![
                    Row::Item(item(icons::PLUS, "New tab", "Ctrl+T", Cmd::NewTab)),
                    Row::Sep,
                    Row::Item(item(icons::RELOAD, "Reload", "", Cmd::TabReload(id))),
                    Row::Item(item(icons::COPY, "Duplicate", "", Cmd::TabDup(id))),
                    Row::Sep,
                    Row::Item(item(icons::CLOSE, "Close tab", "Ctrl+W", Cmd::CloseTab(id))),
                    Row::Item(item(icons::CLOSE, "Close other tabs", "", Cmd::TabCloseOthers(id))),
                    Row::Item(item(icons::CLOSE, "Close tabs to the right", "", Cmd::TabCloseRight(id))),
                    Row::Sep,
                    Row::Item(item(icons::RELOAD, "Reopen closed tab", "Ctrl+Shift+T", Cmd::ReopenTab)),
                ];
                card_w = 260.0;
                card_h = rows_height(&rows);
            }
            Popup::BmMenu(id) => {
                let url = self.sh.store.lock().unwrap().data.bookmarks.iter().find(|b| b.id == id).map(|b| b.url.clone()).unwrap_or_default();
                rows = vec![
                    Row::Item(item(icons::GLOBE, "Open", "", Cmd::OpenUrl(url.clone(), false))),
                    Row::Item(item(icons::PLUS, "Open in new tab", "", Cmd::OpenUrl(url, true))),
                    Row::Sep,
                    Row::Item(item(icons::EDIT, "Edit…", "", Cmd::OpenPage("bookmarks"))),
                    Row::Item(item(icons::TRASH, "Delete", "", Cmd::BmDelete(id))),
                    Row::Sep,
                    Row::Item(item(icons::STAR, "Bookmark manager", "Ctrl+Shift+O", Cmd::OpenPage("bookmarks"))),
                    Row::Item(item(icons::BAR, "Hide bookmarks bar", "Ctrl+Shift+B", Cmd::ToggleBar)),
                ];
                card_w = 250.0;
                card_h = rows_height(&rows);
            }
            Popup::Shield => {
                card_w = 340.0;
                card_h = self.shield_height();
            }
            Popup::Extensions => {
                card_w = 320.0;
                card_h = self.extensions_height();
            }
            Popup::Suggest => {
                card_w = self.addr_rect.width();
                card_h = 8.0 + self.sugg.len() as f32 * 36.0;
            }
            Popup::PageMenu => {
                rows = self.page_menu_rows();
                card_w = 280.0;
                card_h = rows_height(&rows);
            }
            Popup::TorCircuit => {
                let v = self.tor_circuit.lock().unwrap();
                let rows = if v.hops.is_empty() { 0 } else { v.hops.len() + 2 };
                card_w = 330.0;
                card_h = 16.0 + 24.0 + 8.0 + if rows > 0 { rows as f32 * 38.0 } else { 52.0 } + 12.0 + 34.0 + 16.0;
            }
            Popup::Volume => {
                card_w = 300.0;
                card_h = 112.0;
            }
            Popup::Zoom => {
                card_w = 290.0;
                card_h = 52.0;
            }
            Popup::AppLink => {
                card_w = 400.0;
                card_h = 20.0 + 26.0 + 10.0 + 40.0 + 30.0 + 18.0 + 36.0 + 18.0;
            }
            Popup::ExtConfirm => {
                card_w = 400.0;
                let n = self.pending_ext.as_ref().map(|p| p.can.len()).unwrap_or(0);
                card_h = 20.0 + 26.0 + 22.0 + if n == 0 { 24.0 } else { 26.0 + n as f32 * 24.0 } + 18.0 + 36.0 + 18.0;
            }
        }
        let win = vec2(card_w + PAD_X * 2.0, card_h + PAD_TOP + PAD_BOTTOM);
        // Right-aligned to the button for toolbar panels; at the pointer for context menus.
        let pos = match popup {
            Popup::Menu | Popup::Shield | Popup::Extensions | Popup::Zoom | Popup::Volume | Popup::TorCircuit => pos2(at.x - win.x + PAD_X, at.y - 4.0 - PAD_TOP),
            Popup::Suggest => pos2(at.x - PAD_X, at.y - PAD_TOP),
            Popup::ExtConfirm | Popup::AppLink => pos2(at.x - win.x / 2.0, at.y + 6.0 - PAD_TOP),
            _ => pos2(at.x - PAD_X + 2.0, at.y - PAD_TOP + 2.0),
        };
        // Keep the whole pop-up inside Goat's window (and the screen).
        let bounds = ctx
            .input(|i| i.viewport().inner_rect)
            .unwrap_or(Rect::from_min_size(Pos2::ZERO, ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1920.0, 1080.0))));
        // (The see-through padding may hang over the edge; the panel itself stays inside.)
        let pos = pos2(
            pos.x.min(bounds.right() - win.x + PAD_X - 4.0).max(bounds.left() - PAD_X + 4.0),
            pos.y.min(bounds.bottom() - win.y + PAD_BOTTOM - 4.0).max(bounds.top() - PAD_TOP),
        );
        let name = match popup {
            Popup::Menu => "menu",
            Popup::Shield => "shield",
            Popup::Extensions => "ext",
            Popup::TabMenu(_) => "tabmenu",
            Popup::BmMenu(_) => "bmmenu",
            Popup::Suggest => "suggest",
            Popup::ExtConfirm => "addext",
            Popup::AppLink => "applink",
            Popup::Zoom => "zoom",
            Popup::PageMenu => "pagemenu",
            Popup::Volume => "volume",
            Popup::TorCircuit => "torcircuit",
        };
        let title = format!("Goat pop-up {name}");
        let vid = ViewportId::from_hash_of(match popup {
            Popup::Menu => "menu",
            Popup::Shield => "shield",
            Popup::Extensions => "ext",
            Popup::TabMenu(_) => "tabmenu",
            Popup::BmMenu(_) => "bmmenu",
            Popup::Suggest => "suggest",
            Popup::ExtConfirm => "addext",
            Popup::AppLink => "applink",
            Popup::Zoom => "zoom",
            Popup::PageMenu => "pagemenu",
            Popup::Volume => "volume",
            Popup::TorCircuit => "torcircuit",
        });
        // A new pop-up is created hidden; once Windows' own show animation is switched off it's
        // shown and fades/grows in from its centre.
        if self.popup_ready_for != Some(popup) {
            self.popup_ready_for = Some(popup);
            self.popup_ready = !cfg!(windows);
            self.popup_born = t_now;
            self.popup_vis_frames = 0;
        }
        let ready = self.popup_ready || closing.is_some();
        // Shown only once it has drawn twice, so it never appears empty or black.
        let shown = closing.is_some() || (ready && self.popup_vis_frames >= 1);
        let builder = ViewportBuilder::default()
            .with_title(title.clone())
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            .with_active(popup != Popup::Suggest && popup != Popup::Zoom)
            .with_position(pos)
            .with_inner_size(win)
            .with_visible(shown);
        // Windows keeps pop-ups above Goat by making Goat their owner (see prepare_window).
        #[cfg(not(windows))]
        let builder = builder.with_always_on_top();

        let mut close = false;
        // Closed by clicking somewhere else: vanish at once instead of fading.
        let mut blurred = false;
        #[allow(unused_mut)]
        let mut became_ready = false;
        let born = self.popup_born;
        let title = &title;
        let _ = title;
        crashlog::phase(match popup {
            Popup::Menu => "pop-up window: menu",
            Popup::Shield => "pop-up window: shield",
            Popup::Extensions => "pop-up window: extensions",
            Popup::TabMenu(_) => "pop-up window: tab menu",
            Popup::BmMenu(_) => "pop-up window: bookmark menu",
            Popup::Suggest => "pop-up window: address suggestions",
            Popup::ExtConfirm => "pop-up window: add extension",
            Popup::AppLink => "pop-up window: open app",
            Popup::Zoom => "pop-up window: zoom",
            Popup::PageMenu => "pop-up window: page menu",
            Popup::Volume => "pop-up window: volume",
            Popup::TorCircuit => "pop-up window: tor circuit",
        });
        ctx.show_viewport_immediate(vid, builder, |ui, _class| {
            let pctx = ui.ctx().clone();
            // The zoom bubble closes on its own unless the pointer is on it.
            if closing.is_none() && popup == Popup::Zoom {
                let t = now(&pctx);
                let area = pctx.content_rect();
                let on_it = pctx.input(|i| i.pointer.hover_pos()).is_some_and(|q| area.shrink(2.0).contains(q));
                if on_it {
                    self.zoom_bubble_until = self.zoom_bubble_until.max(t + 1.0);
                }
                if t > self.zoom_bubble_until {
                    close = true;
                } else {
                    // Immediate pop-ups only redraw with Goat's window, so wake that.
                    pctx.request_repaint_after_for(std::time::Duration::from_millis(100), egui::ViewportId::ROOT);
                }
            }
            if closing.is_none() && popup != Popup::Suggest && popup != Popup::Zoom {
                match pctx.input(|i| i.viewport().focused) {
                    Some(true) => self.popup_had_focus = true,
                    // "Add extension?" stays until you answer.
                    Some(false) if self.popup_had_focus && popup != Popup::ExtConfirm && popup != Popup::AppLink => {
                        close = true;
                        blurred = true;
                    }
                    _ => {}
                }
                // A click into the page or another app only reaches this pop-up's window, and
                // pop-ups are drawn with Goat's window: keep that drawing so it notices at once.
                pctx.request_repaint_after_for(std::time::Duration::from_millis(30), egui::ViewportId::ROOT);
                // If the window never got focus (e.g. the click went elsewhere), don't linger.
                if !self.popup_had_focus && now(&pctx) - born > 1.5 {
                    pctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
            if pctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
                close = true;
            }
            let full = ui.max_rect();
            // While it fades out it no longer takes clicks.
            if closing.is_some() {
                ui.disable();
            }
            #[cfg(windows)]
            if !ready {
                // First frame: turn off Windows' animation, start fully transparent, then show.
                prepare_window(&title);
                became_ready = true;
                pctx.request_repaint();
            }
            // Goat's pop: the panel springs from 88% to just past full size and settles, fast.
            // The window is see-through around the panel, so only the panel is ever visible.
            let warm = self.popup_vis_frames >= 1;
            if ready && closing.is_none() {
                self.popup_vis_frames = self.popup_vis_frames.saturating_add(1);
                if self.popup_vis_frames == 1 {
                    self.popup_born = now(&pctx);
                }
                if !warm {
                    pctx.request_repaint();
                }
            }
            let (opacity, scale) = match closing {
                None if !ready || !warm => (0.0, 0.88),
                None if anim => {
                    let t = (now(&pctx) - self.popup_born) as f32;
                    if t < 0.28 {
                        pctx.request_repaint();
                    }
                    ((t / 0.05).clamp(0.0, 1.0), anim::pop(t, 0.88))
                }
                Some(t0) => {
                    let k = ((now(&pctx) - t0) as f32 / 0.09).clamp(0.0, 1.0);
                    pctx.request_repaint();
                    (1.0 - anim::in_cubic(k), 1.0 - 0.06 * anim::in_cubic(k))
                }
                None => (1.0, 1.0),
            };
            let frame_r = Rect::from_min_size(full.min + vec2(PAD_X, PAD_TOP), vec2(card_w, card_h));
            let card_r = frame_r;
            // Contents grow from the centre on their own layer (only this pop-up's layer).
            let layer = egui::LayerId::new(egui::Order::Foreground, Id::new(("popup-content", name)));
            let c = frame_r.center().to_vec2();
            pctx.set_transform_layer(layer, egui::emath::TSTransform::from_translation(c) * egui::emath::TSTransform::from_scaling(scale) * egui::emath::TSTransform::from_translation(-c));
            let mut bgp = ui.painter().clone().with_layer_id(layer);
            bgp.set_opacity(opacity);
            card(&bgp, frame_r, &pal);
            let p = bgp.with_clip_rect(frame_r.shrink(1.0));
            let mut local = Vec::new();
            match popup {
                Popup::Menu | Popup::TabMenu(_) | Popup::BmMenu(_) | Popup::PageMenu => self.draw_rows(ui, &p, card_r, &rows, &pal, &mut local),
                Popup::Shield => self.draw_shield(ui, &p, card_r, &pal, anim, &mut local),
                Popup::Extensions => self.draw_extensions(ui, &p, card_r, &pal, &mut local),
                Popup::Suggest => self.draw_suggest(ui, &p, card_r, &pal, &mut local),
                Popup::ExtConfirm => self.draw_ext_confirm(ui, &p, card_r, &pal, &mut local),
                Popup::AppLink => self.draw_app_link(ui, &p, card_r, &pal, &mut local),
                Popup::Zoom => self.draw_zoom(ui, &p, card_r, &pal, &mut local),
                Popup::Volume => self.draw_volume(ui, &p, card_r, &pal, &mut local),
                Popup::TorCircuit => self.draw_tor_circuit(ui, &p, card_r, &pal, &mut local),
            }
            if closing.is_some() {
                return;
            }
            // Most actions close the panel; the shield's own switches keep it open.
            for c in local {
                let keep = matches!(c, Cmd::SetAdblock(_) | Cmd::SetLevel(_) | Cmd::ToggleSiteAdblock | Cmd::ZoomIn | Cmd::ZoomOut | Cmd::ZoomReset | Cmd::SetTheme(_))
                    || popup == Popup::Zoom
                    || popup == Popup::Volume;
                if !keep {
                    close = true;
                }
                cmds.push(c);
            }
        });
        if became_ready {
            self.popup_vis_frames = 0;
            self.popup_ready = true;
            self.popup_born = now(ctx);
        }
        if close && closing.is_none() {
            if popup == Popup::ExtConfirm && self.pending_ext.is_some() {
                cmds.push(Cmd::ExtConfirm(false)); // closed without answering = no
            }
            if popup == Popup::AppLink && self.pending_app.is_some() {
                cmds.push(Cmd::AppLinkAnswer(false, false));
            }
            self.popup = None;
            self.popup_closing = if blurred { None } else { Some((popup, at, t_now)) };
            self.popup_closed_at = Some((popup, t_now));
        }
    }

    /// Chrome-style "Add “X”?" with what the extension will be able to do.
    fn draw_ext_confirm(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let Some(pe) = self.pending_ext.clone() else { return };
        let ctx = ui.ctx().clone();
        let x0 = card_r.left() + 20.0;
        let w = card_r.width() - 40.0;
        let mut y = card_r.top() + 20.0;
        let ic = Rect::from_min_size(pos2(x0, y), Vec2::splat(26.0));
        p.rect_filled(ic, 7, alpha(pal.accent, 0.16));
        icons::draw(p, ic.shrink(5.0), icons::PUZZLE, pal.accent, 1.4);
        text_elided(p, pos2(ic.right() + 10.0, ic.center().y), &format!("Add “{}”?", pe.name), bold(15.0), pal.text, w - 40.0, Align2::LEFT_CENTER);
        y += 26.0 + 10.0;
        if !pe.version.is_empty() {
            p.text(pos2(x0, y + 6.0), Align2::LEFT_CENTER, format!("Version {}", pe.version), font(12.0), pal.muted);
        }
        y += 22.0;
        if pe.can.is_empty() {
            p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, "It asks for no special permissions.", font(13.0), pal.text);
            y += 24.0;
        } else {
            p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, "It can:", bold(13.0), pal.text);
            y += 26.0;
            for line in &pe.can {
                p.circle_filled(pos2(x0 + 6.0, y + 9.0), 2.5, pal.muted);
                text_elided(p, pos2(x0 + 18.0, y + 9.0), line, font(13.0), pal.text, w - 18.0, Align2::LEFT_CENTER);
                y += 24.0;
            }
        }
        y += 18.0;
        let bw = 120.0;
        let add = Rect::from_min_size(pos2(card_r.right() - 20.0 - bw, y), vec2(bw, 34.0));
        let cancel = Rect::from_min_size(pos2(add.left() - 10.0 - 90.0, y), vec2(90.0, 34.0));
        let ra = ui.interact(add, Id::new("ext-confirm-add"), Sense::click());
        paint::accent_fill(p, add, 9, pal);
        if ra.hovered() {
            p.rect_filled(add, 9, alpha(Color32::WHITE, 0.10));
        }
        p.text(add.center(), Align2::CENTER_CENTER, "Add extension", bold(13.0), pal.ink);
        let rc = ui.interact(cancel, Id::new("ext-confirm-cancel"), Sense::click());
        let h = crate::anim::hover(&ctx, Id::new("ext-confirm-cancel-h"), rc.hovered(), 0.12);
        p.rect_filled(cancel, 9, mix(pal.bg2, pal.bg3, h));
        p.rect_stroke(cancel, 9, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
        p.text(cancel.center(), Align2::CENTER_CENTER, "Cancel", font(13.0), pal.text);
        let (enter, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if ra.clicked() || enter {
            cmds.push(Cmd::ExtConfirm(true));
        } else if rc.clicked() || esc {
            cmds.push(Cmd::ExtConfirm(false));
        }
    }

    /// Goat's right-click menu for pages: Goat's look, with the engine doing copy/paste/save.
    fn page_menu_rows(&self) -> Vec<Row> {
        let Some(m) = self.page_menu.clone() else { return vec![] };
        let mut rows: Vec<Row> = Vec::new();
        let native = |rows: &mut Vec<Row>, icon: Icon, label: &str, kbd: &'static str, name: &str| {
            if let Some((id, enabled)) = m.native(name) {
                let mut it = item(icon, label, kbd, Cmd::PageNative(id));
                it.enabled = enabled;
                rows.push(Row::Item(it));
            }
        };
        let web = |u: &str| u.starts_with("http://") || u.starts_with("https://");
        if !m.link.is_empty() {
            rows.push(Row::Item(item(icons::PLUS, "Open link in new tab", "", Cmd::OpenUrl(m.link.clone(), true))));
            if web(&m.link) {
                let mut it = item(icons::MASK, "Open link in private window", "", Cmd::OpenPrivate(m.link.clone()));
                it.kind = Kind::Priv;
                rows.push(Row::Item(it));
            }
            rows.push(Row::Sep);
            native(&mut rows, icons::DOWNLOAD, "Save link as…", "", "saveLinkAs");
            rows.push(Row::Item(item(icons::COPY, "Copy link address", "", Cmd::CopyText(m.link.clone()))));
            rows.push(Row::Sep);
        }
        if !m.src.is_empty() && !m.src.starts_with("data:") || m.native("saveImageAs").is_some() {
            if web(&m.src) {
                rows.push(Row::Item(item(icons::EXTERNAL, "Open image in new tab", "", Cmd::OpenUrl(m.src.clone(), true))));
            }
            native(&mut rows, icons::DOWNLOAD, "Save image as…", "", "saveImageAs");
            native(&mut rows, icons::COPY, "Copy image", "", "copyImage");
            if web(&m.src) {
                rows.push(Row::Item(item(icons::COPY, "Copy image address", "", Cmd::CopyText(m.src.clone()))));
            }
            rows.push(Row::Sep);
        }
        if m.editable {
            native(&mut rows, icons::BACK, "Undo", "Ctrl+Z", "undo");
            native(&mut rows, icons::FORWARD, "Redo", "Ctrl+Y", "redo");
            rows.push(Row::Sep);
            native(&mut rows, icons::EDIT, "Cut", "Ctrl+X", "cut");
            native(&mut rows, icons::COPY, "Copy", "Ctrl+C", "copy");
            native(&mut rows, icons::DOWN, "Paste", "Ctrl+V", "paste");
            native(&mut rows, icons::DOWN, "Paste as plain text", "Ctrl+Shift+V", "pasteAndMatchStyle");
            native(&mut rows, icons::EXPAND, "Select all", "Ctrl+A", "selectAll");
            rows.push(Row::Sep);
        } else if !m.selection.trim().is_empty() {
            if m.native("copy").is_some() {
                native(&mut rows, icons::COPY, "Copy", "Ctrl+C", "copy");
            } else {
                rows.push(Row::Item(item(icons::COPY, "Copy", "Ctrl+C", Cmd::CopyText(m.selection.clone()))));
            }
            let q: String = m.selection.split_whitespace().collect::<Vec<_>>().join(" ");
            let short: String = if q.chars().count() > 24 { q.chars().take(22).collect::<String>() + "…" } else { q.clone() };
            rows.push(Row::Item(item(icons::SEARCH, &format!("Search the web for “{short}”"), "", Cmd::OpenUrl(crate::pages::resolve_input(&self.settings(), &q), true))));
            rows.push(Row::Sep);
        }
        if m.link.is_empty() && m.src.is_empty() && !m.editable && m.selection.trim().is_empty() {
            rows.push(Row::Item(item(icons::BACK, "Back", "Alt+←", Cmd::Back)));
            rows.push(Row::Item(item(icons::FORWARD, "Forward", "Alt+→", Cmd::Forward)));
            rows.push(Row::Item(item(icons::RELOAD, "Reload", "Ctrl+R", Cmd::Reload)));
            rows.push(Row::Sep);
            native(&mut rows, icons::DOWNLOAD, "Save as…", "Ctrl+S", "saveAs");
            rows.push(Row::Item(item(icons::PRINT, "Print…", "Ctrl+P", Cmd::Print)));
            native(&mut rows, icons::EXPAND, "Select all", "Ctrl+A", "selectAll");
            rows.push(Row::Sep);
        }
        match m.native("inspectElement") {
            Some((id, _)) => rows.push(Row::Item(item(icons::SEARCH, "Inspect", "F12", Cmd::PageNative(id)))),
            None => rows.push(Row::Item(item(icons::SEARCH, "Inspect", "F12", Cmd::Devtools))),
        }
        // No double or leading separators.
        let mut out: Vec<Row> = Vec::new();
        for r in rows {
            let sep = matches!(r, Row::Sep);
            if sep && (out.is_empty() || matches!(out.last(), Some(Row::Sep))) {
                continue;
            }
            out.push(r);
        }
        out
    }

    /// Tor Browser-style circuit: This browser → entry → middle → exit → the site.
    fn draw_tor_circuit(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let v = self.tor_circuit.lock().unwrap().clone();
        let purple = Color32::from_rgb(0xb5, 0x7c, 0xf5);
        let x0 = card_r.left() + 18.0;
        let mut y = card_r.top() + 16.0;
        icons::draw(p, Rect::from_min_size(pos2(x0, y + 3.0), Vec2::splat(16.0)), icons::ONION, purple, 1.4);
        text_elided(p, pos2(x0 + 24.0, y + 11.0), "Tor circuit for this site", bold(14.0), pal.text, card_r.width() - 60.0, Align2::LEFT_CENTER);
        y += 24.0 + 8.0;
        if v.loading || !v.error.is_empty() || v.hops.is_empty() {
            let msg = if v.loading { "Looking up the relays…".to_string() } else if v.error.is_empty() { "No circuit found.".to_string() } else { v.error.clone() };
            let g = p.layout(msg, font(12.5), pal.muted, card_r.width() - 36.0);
            p.galley(pos2(x0, y + 6.0), g, pal.muted);
            if v.loading {
                ctx.request_repaint();
            }
            y += 52.0;
        } else {
            let n = v.hops.len();
            let mut rows: Vec<(String, String)> = vec![("This browser".into(), String::new())];
            for (i, h) in v.hops.iter().enumerate() {
                let role = if i == 0 { "Guard" } else if i + 1 == n { "Exit" } else { "Relay" };
                rows.push((format!("{}  ·  {role}", crate::tor::country_name(&h.country)), h.ip.clone()));
            }
            rows.push((v.host.clone(), String::new()));
            let dot_x = x0 + 6.0;
            let first_y = y + 19.0;
            let last_y = y + 19.0 + (rows.len() - 1) as f32 * 38.0;
            p.line_segment([pos2(dot_x, first_y), pos2(dot_x, last_y)], Stroke::new(2.0, alpha(purple, 0.5)));
            for (i, (title, sub)) in rows.iter().enumerate() {
                let cy = y + 19.0;
                let end = i == 0 || i + 1 == rows.len();
                p.circle_filled(pos2(dot_x, cy), if end { 5.0 } else { 4.0 }, if end { pal.text } else { purple });
                if sub.is_empty() {
                    text_elided(p, pos2(x0 + 22.0, cy), title, if end { bold(13.0) } else { font(13.0) }, pal.text, card_r.width() - 60.0, Align2::LEFT_CENTER);
                } else {
                    text_elided(p, pos2(x0 + 22.0, cy - 8.0), title, font(13.0), pal.text, card_r.width() - 60.0, Align2::LEFT_CENTER);
                    text_elided(p, pos2(x0 + 22.0, cy + 9.0), sub, font(11.5), pal.muted, card_r.width() - 60.0, Align2::LEFT_CENTER);
                }
                y += 38.0;
            }
        }
        y += 12.0;
        let br = Rect::from_min_max(pos2(x0, y), pos2(card_r.right() - 18.0, y + 34.0));
        let r = ui.interact(br, Id::new("tor-newcirc"), Sense::click());
        let h = crate::anim::hover(&ctx, Id::new("tor-newcirc-h"), r.hovered(), 0.12);
        p.rect_filled(br, 9, mix(alpha(purple, 0.18), alpha(purple, 0.30), h));
        p.text(br.center(), Align2::CENTER_CENTER, "New circuits (reloads the page)", bold(13.0), pal.text);
        if r.clicked() {
            cmds.push(Cmd::NewTorCircuit);
        }
    }

    /// The tab's volume: 0–600% slider (snaps to 100%), mute and reset.
    fn draw_volume(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let vol = self.active_tab().map(|t| t.volume).unwrap_or(1.0);
        let x0 = card_r.left() + 16.0;
        let x1 = card_r.right() - 16.0;
        let top = card_r.top() + 14.0;
        p.text(pos2(x0, top + 10.0), Align2::LEFT_CENTER, "Tab volume", font(13.0), pal.muted);
        p.text(pos2(x1, top + 10.0), Align2::RIGHT_CENTER, format!("{}%", (vol * 100.0).round()), bold(15.0), if vol > 1.0 { pal.accent } else { pal.text });
        // Slider.
        let track = Rect::from_min_max(pos2(x0 + 6.0, top + 34.0), pos2(x1 - 6.0, top + 40.0));
        let hit = track.expand2(vec2(8.0, 12.0));
        let resp = ui.interact(hit, Id::new("vol-slider"), Sense::click_and_drag());
        let frac = |v: f32| (v / 6.0).clamp(0.0, 1.0);
        p.rect_filled(track, 3, pal.bg3);
        let shown = crate::anim::tween(&ctx, Id::new("vol-shown"), frac(vol), 0.12, crate::anim::out_cubic);
        let fill = Rect::from_min_max(track.min, pos2(track.left() + track.width() * shown, track.max.y));
        if fill.width() > 1.0 {
            paint::accent_fill(p, fill, 3, pal);
        }
        // 100% mark.
        let mark_x = track.left() + track.width() * frac(1.0);
        p.line_segment([pos2(mark_x, track.top() - 4.0), pos2(mark_x, track.bottom() + 4.0)], Stroke::new(1.5, pal.muted));
        let knob = pos2(track.left() + track.width() * shown, track.center().y);
        let grow = crate::anim::hover(&ctx, Id::new("vol-knob"), resp.hovered() || resp.dragged(), 0.15);
        p.circle_filled(knob, 7.0 + 2.0 * grow, pal.text);
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.dragged() || resp.clicked() {
            if let Some(pt) = resp.interact_pointer_pos() {
                let mut v = ((pt.x - track.left()) / track.width()).clamp(0.0, 1.0) * 6.0;
                if (v - 1.0).abs() < 0.12 {
                    v = 1.0; // snap to 100%
                }
                let v = (v * 20.0).round() / 20.0;
                if (v - vol).abs() > 0.001 {
                    cmds.push(Cmd::SetVolume(v));
                }
            }
        }
        // Mute / 100%.
        let by = top + 60.0;
        let mut x = x1;
        let mut btn = |w: f32, key: &str, label: &str, cmd: Cmd| {
            let r = Rect::from_min_size(pos2(x - w, by), vec2(w, 28.0));
            x -= w + 6.0;
            let rr = ui.interact(r, Id::new(("vol-btn", key)), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new(("vol-btn-h", key)), rr.hovered(), 0.12);
            p.rect_filled(r, 8, mix(pal.bg2, pal.bg3, h));
            p.rect_stroke(r, 8, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, label, font(12.5), pal.text);
            if rr.clicked() {
                cmds.push(cmd);
            }
        };
        btn(64.0, "reset", "100%", Cmd::SetVolume(1.0));
        if vol > 0.001 {
            btn(64.0, "mute", "Mute", Cmd::SetVolume(0.0));
        } else {
            btn(64.0, "unmute", "Unmute", Cmd::SetVolume(1.0));
        }
        p.text(pos2(x0, by + 14.0), Align2::LEFT_CENTER, "Up to 600%", font(11.5), pal.muted);
    }

    /// Chrome-style zoom bubble: "Zoom: 125%  [−] [+]  Reset".
    fn draw_zoom(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let zoom = self.active_tab().map(|t| t.zoom).unwrap_or(1.0);
        let cy = card_r.center().y;
        let x0 = card_r.left() + 16.0;
        p.text(pos2(x0, cy), Align2::LEFT_CENTER, "Zoom", font(13.0), pal.muted);
        p.text(pos2(x0 + 44.0, cy), Align2::LEFT_CENTER, format!("{}%", (zoom * 100.0).round()), bold(14.0), pal.text);
        let mut x = card_r.right() - 12.0;
        let mut btn = |w: f32, key: &str, label: &str, cmd: Cmd, tip: &str, accent: bool| {
            let r = Rect::from_min_size(pos2(x - w, cy - 15.0), vec2(w, 30.0));
            x -= w + 6.0;
            let resp = ui.interact(r, Id::new(("zoom-bubble", key)), Sense::click());
            let h = crate::anim::hover(&ctx, Id::new(("zoom-bubble-h", key)), resp.hovered(), 0.12);
            if accent {
                paint::accent_fill(p, r, 8, pal);
                if h > 0.0 {
                    p.rect_filled(r, 8, alpha(Color32::WHITE, 0.10 * h));
                }
            } else {
                p.rect_filled(r, 8, mix(pal.bg2, pal.bg3, h));
                p.rect_stroke(r, 8, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
            }
            p.text(r.center(), Align2::CENTER_CENTER, label, if label.len() == 1 || label == "−" { font(16.0) } else { bold(12.5) }, if accent { pal.ink } else { pal.text });
            if resp.on_hover_text(tip).clicked() {
                cmds.push(cmd);
            }
        };
        btn(64.0, "reset", "Reset", Cmd::ZoomReset, "Back to 100% (Ctrl+0)", true);
        btn(32.0, "in", "+", Cmd::ZoomIn, "Zoom in (Ctrl +)", false);
        btn(32.0, "out", "−", Cmd::ZoomOut, "Zoom out (Ctrl −)", false);
    }

    /// "Open Roblox?": a site wants to start an app on this PC.
    fn draw_app_link(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let Some(pa) = self.pending_app.clone() else { return };
        let ctx = ui.ctx().clone();
        let x0 = card_r.left() + 20.0;
        let w = card_r.width() - 40.0;
        let mut y = card_r.top() + 20.0;
        let ic = Rect::from_min_size(pos2(x0, y), Vec2::splat(26.0));
        p.rect_filled(ic, 7, alpha(pal.accent, 0.16));
        icons::draw(p, ic.shrink(5.0), icons::EXTERNAL, pal.accent, 1.4);
        text_elided(p, pos2(ic.right() + 10.0, ic.center().y), &format!("Open {}?", pa.app), bold(15.0), pal.text, w - 40.0, Align2::LEFT_CENTER);
        y += 26.0 + 10.0;
        let who = if pa.site.is_empty() { "This page".to_string() } else { pa.site.clone() };
        text_elided(p, pos2(x0, y + 10.0), &format!("{who} wants to open this application."), font(13.0), pal.text, w, Align2::LEFT_CENTER);
        text_elided(p, pos2(x0, y + 30.0), &format!("{}:…", pa.scheme), font(11.5), pal.muted, w, Align2::LEFT_CENTER);
        y += 40.0;
        // "Always allow" checkbox.
        let cb = Rect::from_min_size(pos2(x0, y + 6.0), Vec2::splat(16.0));
        let row = Rect::from_min_max(pos2(x0, y), pos2(card_r.right() - 20.0, y + 28.0));
        let rr = ui.interact(row, Id::new("applink-always"), Sense::click());
        if rr.clicked() {
            if let Some(pp) = self.pending_app.as_mut() {
                pp.always = !pp.always;
            }
        }
        let on = self.pending_app.as_ref().is_some_and(|x| x.always);
        p.rect_filled(cb, 4, if on { pal.accent } else { pal.bg });
        p.rect_stroke(cb, 4, Stroke::new(1.0, if on { pal.accent } else { pal.line }), egui::StrokeKind::Inside);
        if on {
            icons::draw(p, cb.shrink(2.0), icons::CHECK, pal.ink, 2.0);
        }
        if !pa.site.is_empty() {
            text_elided(p, pos2(cb.right() + 10.0, cb.center().y), &format!("Always allow {} to open these links", pa.site), font(12.5), pal.text, w - 30.0, Align2::LEFT_CENTER);
        }
        y += 30.0 + 18.0;
        let bw = 130.0;
        let open_r = Rect::from_min_size(pos2(card_r.right() - 20.0 - bw, y), vec2(bw, 34.0));
        let cancel = Rect::from_min_size(pos2(open_r.left() - 10.0 - 90.0, y), vec2(90.0, 34.0));
        let ro = ui.interact(open_r, Id::new("applink-open"), Sense::click());
        paint::accent_fill(p, open_r, 9, pal);
        if ro.hovered() {
            p.rect_filled(open_r, 9, alpha(Color32::WHITE, 0.10));
        }
        text_elided(p, open_r.center(), &format!("Open {}", pa.app), bold(13.0), pal.ink, bw - 16.0, Align2::CENTER_CENTER);
        let rc = ui.interact(cancel, Id::new("applink-cancel"), Sense::click());
        let h = crate::anim::hover(&ctx, Id::new("applink-cancel-h"), rc.hovered(), 0.12);
        p.rect_filled(cancel, 9, mix(pal.bg2, pal.bg3, h));
        p.rect_stroke(cancel, 9, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
        p.text(cancel.center(), Align2::CENTER_CENTER, "Cancel", font(13.0), pal.text);
        let (enter, esc) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if ro.clicked() || enter {
            cmds.push(Cmd::AppLinkAnswer(true, on));
        } else if rc.clicked() || esc {
            cmds.push(Cmd::AppLinkAnswer(false, false));
        }
    }

    fn menu_rows(&self) -> Vec<Row> {
        let s = self.settings();
        let web = self.active_tab().is_some_and(|t| t.url.starts_with("http") && !t.url.starts_with(INTERNAL_BASE));
        let in_private = self.window_kind == TabKind::Private;
        let in_tor = self.window_kind == TabKind::Tor;
        let mut private = item(icons::MASK, if in_private { "New private tab" } else { "New private window" }, "Ctrl+Shift+N", Cmd::NewPrivateTab);
        private.kind = Kind::Priv;
        let mut tor = item(icons::ONION, if in_tor { "New Tor tab" } else { "New Tor window" }, "Alt+Shift+N", Cmd::NewTorTab);
        tor.kind = Kind::Tor;
        let mut bm = item(icons::STAR, "Bookmark this page", "Ctrl+D", Cmd::ToggleBookmark);
        bm.enabled = web;
        let mut bar = item(icons::BAR, "Show bookmarks bar", "Ctrl+Shift+B", Cmd::ToggleBar);
        bar.kind = Kind::Check(s.show_bookmarks_bar);
        let mut print = item(icons::PRINT, "Print…", "Ctrl+P", Cmd::Print);
        print.enabled = web;
        let mut quit = item(icons::POWER, "Exit", "", Cmd::WinClose);
        quit.kind = Kind::Quit;
        vec![
            Row::Item(item(icons::PLUS, "New tab", "Ctrl+T", Cmd::NewTab)),
            Row::Item(private),
            Row::Item(tor),
            Row::Item(item(icons::WIN_MAX, "New window without GPU", "", Cmd::NewNoGpuWindow)),
            Row::Sep,
            Row::Item(item(icons::CLOCK, "History", "Ctrl+H", Cmd::OpenPage("history"))),
            Row::Item(bm),
            Row::Item(bar),
            Row::Item(item(icons::DOWNLOAD, "Downloads", "Ctrl+J", Cmd::OpenPage("downloads"))),
            Row::Sep,
            Row::Zoom,
            Row::Sep,
            Row::Item(item(icons::SEARCH, "Find in page", "Ctrl+F", Cmd::FindOpen(true))),
            Row::Item(print),
            Row::Sep,
            Row::Item(item(icons::SHIELD, "Ad blocker", "", Cmd::Popup(Popup::Shield, Pos2::ZERO))),
            Row::Item(item(icons::PUZZLE, "Extensions", "", Cmd::Popup(Popup::Extensions, Pos2::ZERO))),
            Row::Item(item(icons::TRASH, "Clear browsing data…", "", Cmd::OpenPage("settings"))),
            Row::Sep,
            Row::Item(item(icons::USER, "Profiles", "", Cmd::OpenPage("settings#profiles"))),
            Row::Item(item(icons::COG, "Settings", "Ctrl+,", Cmd::OpenPage("settings"))),
            Row::Item(match (&self.update, &self.update_manual) {
                (Some(u), _) => item(icons::DOWNLOAD, &format!("Update to Goat {}", u.version), "", Cmd::InstallUpdate),
                (_, Some(v)) => item(icons::DOWNLOAD, &format!("Download Goat {v}"), "", Cmd::InstallUpdate),
                _ => item(icons::RELOAD, "Check for updates", "", Cmd::CheckUpdate),
            }),
            Row::Item(item(icons::INFO, &format!("About Goat   v{}", env!("CARGO_PKG_VERSION")), "", Cmd::OpenPage("settings#about"))),
            Row::Item(quit),
        ]
    }

    fn draw_rows(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, rows: &[Row], pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let inner = card_r.shrink(6.0);
        let mut y = inner.top();
        for (i, row) in rows.iter().enumerate() {
            match row {
                Row::Sep => {
                    p.line_segment([pos2(inner.left() + 4.0, y + 4.5), pos2(inner.right() - 4.0, y + 4.5)], Stroke::new(1.0, pal.line));
                    y += 9.0;
                }
                Row::Item(it) => {
                    let r = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), ROW_H));
                    y += ROW_H;
                    let resp = ui.interact(r, Id::new(("row", i)), if it.enabled { Sense::click() } else { Sense::hover() });
                    let hov = it.enabled && resp.hovered();
                    let h = crate::anim::hover(&ctx, Id::new(("rowh", i)), hov, 0.12);
                    if h > 0.0 {
                        p.rect_filled(r, 8, alpha(pal.bg3, h));
                    }
                    let quit_h = it.kind == Kind::Quit && hov;
                    let text_c = if !it.enabled { alpha(pal.muted, 0.55) } else if quit_h { pal.danger } else { pal.text };
                    let icon_c = match it.kind {
                        _ if !it.enabled => alpha(pal.muted, 0.55),
                        Kind::Quit if hov => pal.danger,
                        Kind::Tor => if hov { Color32::from_rgb(0xd3, 0xb0, 0xff) } else { Color32::from_rgb(0xb5, 0x7c, 0xf5) },
                        Kind::Priv => if hov { Color32::from_rgb(0xc4, 0xb5, 0xfd) } else { PV },
                        Kind::Check(true) => pal.accent,
                        _ if hov => pal.accent,
                        _ => pal.muted,
                    };
                    // Lay the label out at every size the hover bounce uses before it's needed, so
                    // its letters are already in the font texture (new letters appearing mid-
                    // animation made a thin line flash on Windows).
                    for k in 0..4 {
                        let _ = p.layout_no_wrap(it.label.clone(), font(13.0 + 0.5 * k as f32), text_c);
                    }
                    // Hovered row: icon and label grow a little, with a bounce.
                    let g = crate::anim::tween(&ctx, Id::new(("rowg", i)), if hov { 1.0 } else { 0.0 }, 0.32, crate::anim::ease_bounce);
                    let ir = Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), Vec2::splat(16.0 * (1.0 + 0.22 * g)));
                    let fill = if let Kind::Check(true) = it.kind { Some(alpha(pal.accent, 0.25)) } else { None };
                    icons::draw_filled(p, ir, it.icon, icon_c, 1.5, fill);
                    let kbd_w = if it.kbd.is_empty() { 0.0 } else { text_width(ui, it.kbd, font(11.5)) + 12.0 };
                    // Grow the label only if it fits at full size; long labels stay put (eliding
                    // them mid-animation made them flicker).
                    let budget = r.width() - 50.0 - kbd_w;
                    let tx = pos2(r.left() + 36.0 + 3.0 * g, r.center().y);
                    if text_width(ui, &it.label, font(14.5)) <= budget {
                        p.text(tx, Align2::LEFT_CENTER, &it.label, font(((13.0 * (1.0 + 0.1 * g)) * 2.0).round() / 2.0), text_c);
                    } else {
                        text_elided(p, tx, &it.label, font(13.0), text_c, budget, Align2::LEFT_CENTER);
                    }
                    if !it.kbd.is_empty() {
                        p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, it.kbd, font(11.5), pal.muted);
                    }
                    if let Kind::Check(true) = it.kind {
                        if it.kbd.is_empty() {
                            icons::draw(p, Rect::from_min_size(pos2(r.right() - 26.0, r.center().y - 8.0), Vec2::splat(16.0)), icons::CHECK, pal.accent, 1.8);
                        }
                    }
                    if resp.clicked() {
                        let cmd = match &it.cmd {
                            // "Ad blocker" in the menu opens the Shield panel under the shield button.
                            Cmd::Popup(Popup::Shield, _) => {
                                let at = self.popup.map(|x| x.1).unwrap_or_default();
                                Cmd::Popup(Popup::Shield, pos2(at.x - 60.0, at.y))
                            }
                            c => c.clone(),
                        };
                        cmds.push(cmd);
                    }
                }
                Row::Zoom => {
                    let r = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), ZOOM_H));
                    y += ZOOM_H;
                    let cy = r.center().y;
                    icons::draw(p, Rect::from_min_size(pos2(r.left() + 10.0, cy - 8.0), Vec2::splat(16.0)), icons::ZOOM, pal.muted, 1.5);
                    p.text(pos2(r.left() + 36.0, cy), Align2::LEFT_CENTER, "Zoom", font(13.0), pal.text);
                    let zoom = self.active_tab().map(|t| t.zoom).unwrap_or(1.0);
                    let label = format!("{}%", (zoom * 100.0).round());
                    let mut x = r.right() - 4.0;
                    let mut btn = |w: f32, key: &str, draw: &dyn Fn(&Painter, Rect, Color32), cmd: Cmd, tip: &str| {
                        let b = Rect::from_min_size(pos2(x - w, cy - 14.0), vec2(w, 28.0));
                        x -= w + 4.0;
                        let resp = ui.interact(b, Id::new(("zoom", key)), Sense::click());
                        p.rect_filled(b, 8, if resp.hovered() { pal.bg3 } else { pal.bg });
                        p.rect_stroke(b, 8, Stroke::new(1.0, pal.line), StrokeKind::Inside);
                        draw(p, b, pal.text);
                        if resp.on_hover_text(tip).clicked() {
                            cmds.push(cmd);
                        }
                    };
                    btn(32.0, "fs", &|p, b, c| icons::draw(p, Rect::from_center_size(b.center(), Vec2::splat(14.0)), icons::EXPAND, c, 1.5), Cmd::Fullscreen, "Full screen (F11)");
                    btn(32.0, "in", &|p, b, c| { p.text(b.center(), Align2::CENTER_CENTER, "+", font(15.0), c); }, Cmd::ZoomIn, "Zoom in (Ctrl+=)");
                    let lbl = label.clone();
                    btn(58.0, "reset", &move |p, b, c| { p.text(b.center(), Align2::CENTER_CENTER, &lbl, font(12.5), c); }, Cmd::ZoomReset, "Reset to 100% (Ctrl+0)");
                    btn(32.0, "out", &|p, b, c| { p.text(b.center(), Align2::CENTER_CENTER, "−", font(15.0), c); }, Cmd::ZoomOut, "Zoom out (Ctrl+−)");
                }
                Row::Themes => {
                    let r = Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), THEMES_H));
                    y += THEMES_H;
                    let cy = r.center().y;
                    icons::draw(p, Rect::from_min_size(pos2(r.left() + 10.0, cy - 8.0), Vec2::splat(16.0)), icons::PALETTE, pal.muted, 1.5);
                    p.text(pos2(r.left() + 36.0, cy), Align2::LEFT_CENTER, "Theme", font(13.0), pal.text);
                    let cur = self.settings().theme;
                    let mut x = r.right() - 10.0 - 18.0;
                    let dark = self.sh.system_dark.load(Ordering::Relaxed);
                    let mut ids: Vec<&str> = themes::all().iter().map(|t| t.id).collect();
                    ids.insert(0, "system");
                    // As many dots as fit; the rest are in Settings → Appearance.
                    let fit = (((r.width() - 80.0) / 22.0) as usize).max(9);
                    if ids.len() > fit {
                        // Keep the current theme visible.
                        let cur_i = ids.iter().position(|t| *t == cur);
                        ids.truncate(fit);
                        if let Some(i) = cur_i.filter(|i| *i >= fit) {
                            let all: Vec<&str> = themes::all().iter().map(|t| t.id).collect();
                            ids[fit - 1] = all[i - 1];
                        }
                    }
                    for tid in ids.into_iter().rev() {
                        let th = themes::get(tid, dark);
                        let tp = th.palette(true);
                        let b = Rect::from_center_size(pos2(x + 9.0, cy), Vec2::splat(18.0));
                        x -= 22.0;
                        let resp = ui.interact(b, Id::new(("theme", tid)), Sense::click());
                        if tid == "system" {
                            p.circle_filled(b.center(), 9.0, themes::hex("#eef0f3"));
                            let half: Vec<Pos2> = (0..=16).map(|k| b.center() + Vec2::angled(-std::f32::consts::FRAC_PI_2 + k as f32 / 16.0 * std::f32::consts::PI) * 9.0).collect();
                            paint::fill_fan(p, &half, themes::hex("#101214"));
                        } else {
                            paint::grad_rrect(p, b, 9, &[(0.0, tp.accent), (1.0, tp.accent2)], 135.0);
                        }
                        let ring = if cur == tid { pal.text } else if resp.hovered() { pal.muted } else { alpha(pal.line, 0.0) };
                        p.circle_stroke(b.center(), 10.5, Stroke::new(1.5, ring));
                        let name = if tid == "system" { "System" } else { th.name };
                        if resp.on_hover_text(name).clicked() {
                            cmds.push(Cmd::SetTheme(tid.to_string()));
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ Shield panel

    fn shield_site(&self) -> (String, bool, u32) {
        let (host, web, count) = match self.active_tab() {
            Some(t) => (
                crate::adblock::host_of(&t.page_url.lock().unwrap()).trim_start_matches("www.").to_string(),
                t.url.starts_with("http") && !t.url.starts_with(INTERNAL_BASE),
                t.blocked.load(Ordering::Relaxed),
            ),
            None => (String::new(), false, 0),
        };
        (if web { host } else { String::new() }, web, count)
    }

    fn shield_height(&self) -> f32 {
        let (site, _, _) = self.shield_site();
        let header = 72.0;
        let site_h = if site.is_empty() { 14.0 + 34.0 + 4.0 + 18.0 + 14.0 } else { 14.0 + 34.0 + 4.0 + 18.0 + 8.0 + 50.0 + 14.0 };
        let level = 14.0 + 16.0 + 8.0 + 38.0 + 10.0 + 18.0 + 8.0 + 22.0 + 14.0;
        header + 1.0 + site_h + 1.0 + level + 1.0 + 44.0
    }

    fn draw_shield(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, anim: bool, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let ab = self.sh.adblock.clone();
        let on = ab.enabled();
        let (site, _web, count) = self.shield_site();
        let allowed = !site.is_empty() && ab.is_allowed(&site);
        let s = self.settings();
        let level = ab.level();
        let pr = p.with_clip_rect(card_r.shrink(1.0));

        // Header.
        let head = Rect::from_min_size(card_r.min, vec2(card_r.width(), 72.0));
        if pal.gradients {
            paint::grad_rrect(&pr, head, egui::CornerRadius { nw: 16, ne: 16, sw: 0, se: 0 },
                &[(0.0, mix(pal.accent, pal.bg2, 0.18)), (0.6, pal.bg2), (1.0, mix(pal.accent2, pal.bg2, 0.12))], 135.0);
        }
        pr.line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, pal.line));
        let badge = Rect::from_min_size(pos2(head.left() + 16.0, head.top() + 16.0), Vec2::splat(40.0));
        let grey = !on || allowed;
        let mut bp = pr.clone();
        if grey {
            bp.set_opacity(0.55);
            bp.rect_filled(badge, 12, pal.muted);
        } else {
            paint::shadow(&bp, badge.shrink(4.0), 12.0, pal.dark);
            paint::accent_fill(&bp, badge, 12, pal);
        }
        let badge_icon = if grey { icons::SHIELD } else { icons::SHIELD_TICK };
        icons::draw(&bp, Rect::from_center_size(badge.center(), Vec2::splat(26.0)), badge_icon, pal.ink, 1.8);
        pr.text(pos2(badge.right() + 12.0, head.top() + 26.0), Align2::LEFT_CENTER, "Goat Shield", bold(15.0), pal.text);
        let status = if !on {
            "Off everywhere".to_string()
        } else if !ab.ready() {
            "Loading blocklists…".into()
        } else if allowed {
            format!("Paused on {site}")
        } else {
            "Protecting you".into()
        };
        text_elided(&pr, pos2(badge.right() + 12.0, head.top() + 46.0), &status, font(12.5), pal.muted, 190.0, Align2::LEFT_CENTER);
        let sw = Rect::from_min_size(pos2(head.right() - 16.0 - 40.0, head.center().y - 12.0), vec2(40.0, 24.0));
        if switch(ui, p, sw, Id::new("ab-master"), on, pal, true).on_hover_text("Turn the ad blocker on or off everywhere").clicked() {
            cmds.push(Cmd::SetAdblock(!on));
        }

        // This site.
        let mut y = head.bottom() + 1.0 + 14.0;
        let x0 = card_r.left() + 16.0;
        let w = card_r.width() - 32.0;
        let mut sp = pr.clone();
        if site.is_empty() {
            sp.set_opacity(0.5);
        }
        let shown = if on && !allowed { count } else { 0 };
        // Count up animation.
        let disp = if shown == 0 { anim::tween(&ctx, Id::new("ab-count"), 0.0, 0.0, anim::out_cubic) } else { anim::tween(&ctx, Id::new("ab-count"), shown as f32, 0.26, anim::out_cubic) };
        let _ = anim;
        let num = format!("{}", disp.round() as u32);
        let num_r = sp.text(pos2(x0, y + 34.0), Align2::LEFT_BOTTOM, &num, bold(34.0), if on && !allowed { pal.accent } else { pal.muted });
        sp.text(pos2(num_r.right() + 8.0, y + 30.0), Align2::LEFT_BOTTOM, "blocked on this page", font(13.0), pal.muted);
        y += 34.0 + 4.0;
        text_elided(&sp, pos2(x0, y + 9.0), if site.is_empty() { "Not a website" } else { &site }, font(12.0), pal.muted, w, Align2::LEFT_CENTER);
        y += 18.0;
        if !site.is_empty() {
            y += 8.0;
            let row = Rect::from_min_size(pos2(x0, y), vec2(w, 50.0));
            let resp = ui.interact(row, Id::new("ab-siterow"), if on { Sense::click() } else { Sense::hover() });
            pr.rect_filled(row, 10, pal.bg);
            text_elided(&pr, pos2(row.left() + 12.0, row.top() + 17.0), &format!("Block on {site}"), bold(13.0), pal.text, w - 70.0, Align2::LEFT_CENTER);
            pr.text(pos2(row.left() + 12.0, row.top() + 34.0), Align2::LEFT_CENTER, "Turn off if the site looks broken.", font(11.5), pal.muted);
            let ssw = Rect::from_min_size(pos2(row.right() - 12.0 - 34.0, row.center().y - 10.0), vec2(34.0, 20.0));
            let sresp = switch(ui, p, ssw, Id::new("ab-site"), !allowed, pal, on);
            if on && (resp.clicked() || sresp.clicked()) {
                cmds.push(Cmd::ToggleSiteAdblock);
            }
            y += 50.0;
        }
        y += 14.0;
        pr.line_segment([pos2(card_r.left(), y), pos2(card_r.right(), y)], Stroke::new(1.0, pal.line));
        y += 1.0 + 14.0;

        // Blocking level.
        pr.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, "BLOCKING LEVEL", bold(11.0), pal.muted);
        y += 16.0 + 8.0;
        let seg = Rect::from_min_size(pos2(x0, y), vec2(w, 38.0));
        let mut segp = pr.clone();
        if !on {
            segp.set_opacity(0.5);
        }
        segp.rect_filled(seg, 10, pal.bg);
        segp.rect_stroke(seg, 10, Stroke::new(1.0, pal.line), StrokeKind::Inside);
        let inner = seg.shrink(3.0);
        let bw = inner.width() / 3.0;
        let idx = level as usize as f32;
        let ti = anim::tween(&ctx, Id::new("ab-thumb"), idx, 0.38, anim::spring);
        let thumb = Rect::from_min_size(pos2(inner.left() + bw * ti, inner.top()), vec2(bw, inner.height()));
        paint::shadow(&segp, thumb.shrink(3.0), 7.0, pal.dark);
        paint::accent_fill(&segp, thumb, 7, pal);
        for (k, (lvl, label)) in [(Level::Ads, "Ads"), (Level::Standard, "Standard"), (Level::Strict, "Strict")].into_iter().enumerate() {
            let b = Rect::from_min_size(pos2(inner.left() + bw * k as f32, inner.top()), vec2(bw, inner.height()));
            let resp = ui.interact(b, Id::new(("ab-lvl", k)), if on { Sense::click() } else { Sense::hover() });
            let sel = lvl == level;
            let near = (1.0 - (ti - k as f32).abs()).clamp(0.0, 1.0);
            let h = anim::hover(&ctx, Id::new(("ab-lvl-h", k)), resp.hovered() && on && !sel, 0.15);
            let c = mix(pal.ink, mix(pal.text, pal.muted, h), near);
            segp.text(b.center(), Align2::CENTER_CENTER, label, bold(13.0), c);
            if resp.clicked() && !sel && on {
                cmds.push(Cmd::SetLevel(lvl));
            }
        }
        y += 38.0 + 10.0;
        let (desc, has, not): (&str, &[&str], &[&str]) = match level {
            Level::Ads => ("Lightest. Removes ads, lets trackers through.", &["Ads"], &["Trackers", "Cookie banners & popups"]),
            Level::Standard => ("Removes ads and trackers. Recommended.", &["Ads", "Trackers"], &["Cookie banners & popups"]),
            Level::Strict => ("Also hides cookie banners and popups.", &["Ads", "Trackers", "Cookie banners & popups"], &[]),
        };
        text_elided(&pr, pos2(x0, y + 9.0), desc, font(12.5), pal.text, w, Align2::LEFT_CENTER);
        y += 18.0 + 8.0;
        let mut cx = x0;
        for (label, yes) in has.iter().map(|l| (*l, true)).chain(not.iter().map(|l| (*l, false))) {
            let tw = text_width(ui, label, font(11.5));
            let chip = Rect::from_min_size(pos2(cx, y), vec2(tw + 28.0, 22.0));
            cx += chip.width() + 6.0;
            pr.rect_filled(chip, 11, if yes { alpha(pal.accent, 0.16) } else { pal.bg });
            if yes {
                icons::draw(&pr, Rect::from_min_size(pos2(chip.left() + 6.0, chip.center().y - 6.0), Vec2::splat(12.0)), icons::CHECK, pal.accent, 2.2);
            } else {
                pr.text(pos2(chip.left() + 12.0, chip.center().y), Align2::CENTER_CENTER, "–", bold(11.5), pal.muted);
            }
            pr.text(pos2(chip.left() + 21.0, chip.center().y), Align2::LEFT_CENTER, label, font(11.5), if yes { pal.text } else { pal.muted });
        }
        y += 22.0 + 14.0;
        pr.line_segment([pos2(card_r.left(), y), pos2(card_r.right(), y)], Stroke::new(1.0, pal.line));
        y += 1.0;

        // Footer.
        let fy = y + 22.0;
        let total = s.adblock_total.to_string();
        let tr = pr.text(pos2(x0, fy), Align2::LEFT_CENTER, &total, bold(12.0), pal.text);
        pr.text(pos2(tr.right() + 4.0, fy), Align2::LEFT_CENTER, "blocked all-time", font(12.0), pal.muted);
        let links = [(format!("Allowed sites ({})", s.adblock_allow.len()), "allow"), (if ab.custom_count() > 0 { format!("Custom filters ({})", ab.custom_count()) } else { "Custom filters".into() }, "filters")];
        let mut lx = card_r.right() - 12.0;
        for (label, key) in links.iter().rev() {
            let lw = text_width(ui, label, font(12.0)) + 12.0;
            let b = Rect::from_min_size(pos2(lx - lw, fy - 12.0), vec2(lw, 24.0));
            lx -= lw + 2.0;
            let resp = ui.interact(b, Id::new(("ab-link", *key)), Sense::click());
            if resp.hovered() {
                pr.rect_filled(b, 6, pal.bg);
            }
            pr.text(b.center(), Align2::CENTER_CENTER, label, font(12.0), pal.accent);
            if resp.clicked() {
                cmds.push(Cmd::OpenPage("settings"));
            }
        }
    }

    // ------------------------------------------------------------------ Extensions panel

    fn extensions_height(&self) -> f32 {
        let off = self.settings().disabled_extensions;
        let list: Vec<_> = crate::extinstall::list().into_iter().filter(|e| !off.contains(&e.folder)).collect();
        let n = list.len().min(6);
        let store_page = self.active_tab().is_some_and(|t| crate::extinstall::parse_store_url(&t.url).is_some());
        56.0 + if store_page { 52.0 } else { 0.0 } + if n == 0 { 54.0 } else { n as f32 * 44.0 + 8.0 } + 1.0 + 56.0
    }

    fn draw_extensions(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let x0 = card_r.left() + 16.0;
        let w = card_r.width() - 32.0;
        let mut y = card_r.top() + 18.0;
        let off = self.settings().disabled_extensions;
        let list: Vec<_> = crate::extinstall::list().into_iter().filter(|e| !off.contains(&e.folder)).collect();
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, "Extensions", bold(15.0), pal.text);
        let sub = match list.len() {
            0 => "None installed".to_string(),
            1 => "1 installed".into(),
            n => format!("{n} installed"),
        };
        p.text(pos2(card_r.right() - 16.0, y + 8.0), Align2::RIGHT_CENTER, sub, font(12.0), pal.muted);
        y += 38.0;
        if self.active_tab().is_some_and(|t| crate::extinstall::parse_store_url(&t.url).is_some()) {
            let b = Rect::from_min_size(pos2(x0, y), vec2(w, 40.0));
            let resp = ui.interact(b, Id::new("ext-add"), Sense::click());
            paint::accent_fill(p, b, 10, pal);
            if resp.hovered() {
                p.rect_filled(b, 10, alpha(Color32::WHITE, 0.10));
            }
            icons::draw(p, Rect::from_min_size(pos2(b.left() + 14.0, b.center().y - 8.0), Vec2::splat(16.0)), icons::PUZZLE, pal.ink, 1.5);
            p.text(pos2(b.left() + 40.0, b.center().y), Align2::LEFT_CENTER, "Add this extension to Goat", bold(13.0), pal.ink);
            if resp.clicked() {
                cmds.push(Cmd::InstallExtension);
            }
            y += 52.0;
        }
        if list.is_empty() {
            p.text(pos2(x0, y + 14.0), Align2::LEFT_CENTER, "Add ad blockers, password managers and more", font(12.5), pal.muted);
            p.text(pos2(x0, y + 32.0), Align2::LEFT_CENTER, "from the Chrome Web Store.", font(12.5), pal.muted);
            y += 54.0;
        } else {
            y += 4.0;
            let pins = self.settings().pinned_extensions;
            let anchor = Rect::from_min_size(pos2(self.content_rect.right() - 60.0, self.content_rect.top() - 30.0), Vec2::splat(28.0));
            for (i, e) in list.iter().take(6).enumerate() {
                let r = Rect::from_min_size(pos2(card_r.left() + 6.0, y), vec2(card_r.width() - 12.0, 44.0));
                y += 44.0;
                // Goat's row: [icon name] … [options] [pin]
                let pin_b = Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), Vec2::splat(28.0));
                let opt_b = Rect::from_center_size(pos2(r.right() - 52.0, r.center().y), Vec2::splat(28.0));
                let main_r = Rect::from_min_max(r.min, pos2(if e.options.is_empty() { pin_b.left() } else { opt_b.left() } - 4.0, r.bottom()));
                let resp = ui.interact(main_r, Id::new(("ext", i)), Sense::click());
                let h = crate::anim::hover(&ctx, Id::new(("exth", i)), resp.hovered(), 0.12);
                if h > 0.0 {
                    p.rect_filled(main_r, 8, alpha(pal.bg3, h));
                }
                let ic = Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 14.0), Vec2::splat(28.0));
                match self.ext_icon(&ctx, e) {
                    Some(tex) => {
                        p.image(tex.id(), ic.shrink(3.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        p.rect_filled(ic, 8, alpha(pal.accent, 0.16));
                        icons::draw(p, ic.shrink(6.0), icons::PUZZLE, pal.accent, 1.4);
                    }
                }
                text_elided(p, pos2(ic.right() + 10.0, r.center().y - 7.0), &e.name, font(13.0), pal.text, main_r.width() - 52.0, Align2::LEFT_CENTER);
                let sub = if !e.valid { "Broken (no manifest.json)".to_string() } else if e.popup.is_empty() { format!("v{}", e.version) } else { format!("v{} · click to open", e.version) };
                p.text(pos2(ic.right() + 10.0, r.center().y + 9.0), Align2::LEFT_CENTER, sub, font(11.5), if e.valid { pal.muted } else { pal.danger });
                if resp.on_hover_text(if e.popup.is_empty() { e.name.clone() } else { format!("Open {}", e.name) }).clicked() && e.valid {
                    cmds.push(Cmd::ExtOpen(e.folder.clone(), anchor));
                    cmds.push(Cmd::ClosePopup);
                }
                if !e.options.is_empty() {
                    let ro = ui.interact(opt_b, Id::new(("extopt", i)), Sense::click());
                    let ho = crate::anim::hover(&ctx, Id::new(("extopth", i)), ro.hovered(), 0.12);
                    if ho > 0.0 {
                        p.rect_filled(opt_b, 8, alpha(pal.bg3, ho));
                    }
                    icons::draw(p, Rect::from_center_size(opt_b.center(), Vec2::splat(16.0)), icons::COG, if ro.hovered() { pal.text } else { pal.muted }, 1.4);
                    if ro.on_hover_text(format!("{} options", e.name)).clicked() {
                        cmds.push(Cmd::ExtOptions(e.folder.clone()));
                        cmds.push(Cmd::ClosePopup);
                    }
                }
                let pinned = pins.contains(&e.folder);
                let rp = ui.interact(pin_b, Id::new(("extpin", i)), Sense::click());
                let hp = crate::anim::hover(&ctx, Id::new(("extpinh", i)), rp.hovered(), 0.12);
                if hp > 0.0 {
                    p.rect_filled(pin_b, 8, alpha(pal.bg3, hp));
                }
                let col = if pinned { pal.accent } else if rp.hovered() { pal.text } else { pal.muted };
                icons::draw_filled(p, Rect::from_center_size(pin_b.center(), Vec2::splat(15.0)), icons::PIN, col, 1.2, pinned.then(|| alpha(pal.accent, 0.35)));
                if rp.on_hover_text(if pinned { "Unpin from toolbar" } else { "Pin to toolbar" }).clicked() {
                    cmds.push(Cmd::ExtPin(e.folder.clone()));
                }
            }
            y += 4.0;
        }
        p.line_segment([pos2(card_r.left(), y), pos2(card_r.right(), y)], Stroke::new(1.0, pal.line));
        y += 1.0 + 12.0;
        let half = (w - 8.0) / 2.0;
        let get = Rect::from_min_size(pos2(x0, y), vec2(half, 32.0));
        let manage = Rect::from_min_size(pos2(x0 + half + 8.0, y), vec2(half, 32.0));
        let r1 = ui.interact(get, Id::new("ext-get"), Sense::click());
        paint::accent_fill(p, get, 8, pal);
        if r1.hovered() {
            p.rect_filled(get, 8, alpha(Color32::WHITE, 0.10));
        }
        p.text(get.center(), Align2::CENTER_CENTER, "Get extensions", bold(12.5), pal.ink);
        let r2 = ui.interact(manage, Id::new("ext-manage"), Sense::click());
        p.rect_filled(manage, 8, if r2.hovered() { pal.bg3 } else { pal.bg });
        p.rect_stroke(manage, 8, Stroke::new(1.0, pal.line), StrokeKind::Inside);
        p.text(manage.center(), Align2::CENTER_CENTER, "Manage", bold(12.5), pal.text);
        if r1.clicked() {
            cmds.push(Cmd::OpenUrl("https://chromewebstore.google.com/category/extensions".into(), true));
        }
        if r2.clicked() {
            cmds.push(Cmd::OpenPage("extensions"));
        }
    }

    // ------------------------------------------------------------------ address suggestions

    fn draw_suggest(&mut self, ui: &mut Ui, p: &Painter, card_r: Rect, pal: &Palette, cmds: &mut Vec<Cmd>) {
        let ctx = ui.ctx().clone();
        let mut y = card_r.top() + 4.0;
        let sugg = self.sugg.clone();
        if ctx.input(|i| !i.pointer.any_down() && !i.pointer.any_released()) {
            ctx.data_mut(|d| d.remove::<bool>(Id::new("sugg-x-down")));
        }
        for (i, (url, title)) in sugg.iter().enumerate() {
            let r = Rect::from_min_size(pos2(card_r.left() + 4.0, y), vec2(card_r.width() - 8.0, 36.0));
            y += 36.0;
            let resp = ui.interact(r, Id::new(("sugg", i)), Sense::click());
            let sel = self.sugg_sel == Some(i) || resp.hovered();
            let h = anim::hover(&ctx, Id::new(("sugg-h", i)), sel, 0.14);
            if h > 0.01 {
                p.rect_filled(r, 10, alpha(pal.bg3, h));
            }
            let star = title.starts_with('★');
            let t = title.trim_start_matches('★').trim();
            let ic = Rect::from_min_size(pos2(r.left() + 10.0, r.center().y - 8.0), Vec2::splat(16.0));
            icons::draw(p, ic, if star { icons::STAR } else { icons::CLOCK }, if star { pal.accent } else { pal.muted }, 1.5);
            // × removes a history suggestion (not bookmarks).
            let xr = Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), Vec2::splat(24.0));
            let show_x = !star && (resp.hovered() || self.sugg_sel == Some(i));
            let on_x = ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pt| xr.contains(pt));
            let x_hit = !star && resp.hovered() && on_x;
            if show_x {
                if x_hit {
                    p.rect_filled(xr, 6, alpha(pal.text, 0.10));
                }
                icons::draw(p, xr.shrink(6.0), icons::CLOSE, if x_hit { pal.text } else { pal.muted }, 1.6);
            }
            let tx = ic.right() + 10.0;
            let avail = r.right() - (if star { 10.0 } else { 40.0 }) - tx;
            let tr = text_elided(p, pos2(tx, r.center().y), if t.is_empty() { url } else { t }, font(13.0), pal.text, avail * 0.6, Align2::LEFT_CENTER);
            let short = url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
            text_elided(p, pos2(tr.right() + 10.0, r.center().y), &format!("— {short}"), font(12.0), pal.accent, (tx + avail - tr.right() - 10.0).max(10.0), Align2::LEFT_CENTER);
            let pressed = ctx.input(|i| i.pointer.primary_pressed());
            if x_hit && pressed {
                // The × was pressed: remove it, and ignore the release that follows.
                ctx.data_mut(|d| d.insert_temp(Id::new("sugg-x-down"), true));
                cmds.push(Cmd::ForgetSuggestion(url.clone()));
                break;
            }
            let x_down: bool = ctx.data(|d| d.get_temp(Id::new("sugg-x-down")).unwrap_or(false));
            if resp.clicked() && x_down {
                ctx.data_mut(|d| d.remove::<bool>(Id::new("sugg-x-down")));
                continue;
            }
            // Act on press: the click also takes focus from the address bar.
            if !(on_x && !star) && !x_down && (resp.hovered() && pressed || resp.clicked()) {
                cmds.push(Cmd::Navigate(url.clone()));
                self.addr_editing = false;
                break;
            }
        }
        let _ = PURPLE;
    }
}
