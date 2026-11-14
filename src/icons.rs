//! Native icon drawing: a tiny SVG-path interpreter (M L H V C S Q A Z, absolute + relative)
//! that renders Goat's 16×16 line icons with egui strokes.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

#[derive(Clone, Copy)]
pub enum El {
    Path(&'static str),
    Circle(f32, f32, f32),
    /// x, y, w, h, rx
    Rect(f32, f32, f32, f32, f32),
}
use El::*;

pub type Icon = &'static [El];

pub const BACK: Icon = &[Path("M10 3 5 8l5 5")];
pub const FORWARD: Icon = &[Path("m6 3 5 5-5 5")];
pub const RELOAD: Icon = &[Path("M13 8a5 5 0 1 1-1.6-3.7M13 2.5v3h-3")];
pub const STOP: Icon = &[Path("M4 4l8 8M12 4l-8 8")];
pub const CLOSE: Icon = STOP;
pub const PLUS: Icon = &[Path("M8 3v10M3 8h10")];
pub const STAR: Icon = &[Path("m8 1.8 1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z")];
pub const SHIELD: Icon = &[Path("M8 1.8 2.8 3.7v4c0 3.2 2.2 5.4 5.2 6.5 3-1.1 5.2-3.3 5.2-6.5v-4z")];
pub const SHIELD_TICK: Icon = &[Path("M8 1.8 2.8 3.7v4c0 3.2 2.2 5.4 5.2 6.5 3-1.1 5.2-3.3 5.2-6.5v-4z"), Path("m5.7 8.1 1.6 1.6 3.1-3.3")];
pub const PUZZLE: Icon = &[Path("M6.2 2.5h1.6a1.3 1.3 0 0 1 2.6 0H12c.6 0 1 .4 1 1v1.9a1.3 1.3 0 0 1 0 2.6v2.5c0 .6-.4 1-1 1H9.6a1.3 1.3 0 0 0-2.6 0H4.5c-.6 0-1-.4-1-1V9.1a1.3 1.3 0 0 0 0-2.6V3.5c0-.6.4-1 1-1z")];
pub const COG: Icon = &[Circle(8.0, 8.0, 2.0), Path("M8.1 1.3h-.3a1.3 1.3 0 0 0-1.3 1.3v.1a1.3 1.3 0 0 1-.7 1.2l-.3.2a1.3 1.3 0 0 1-1.3 0l-.1-.1a1.3 1.3 0 0 0-1.8.5l-.1.3a1.3 1.3 0 0 0 .5 1.8l.1.1a1.3 1.3 0 0 1 .7 1.1v.3a1.3 1.3 0 0 1-.7 1.2l-.1.1a1.3 1.3 0 0 0-.5 1.8l.1.3a1.3 1.3 0 0 0 1.8.5l.1-.1a1.3 1.3 0 0 1 1.3 0l.3.2a1.3 1.3 0 0 1 .7 1.2v.1a1.3 1.3 0 0 0 1.3 1.3h.3a1.3 1.3 0 0 0 1.3-1.3v-.1a1.3 1.3 0 0 1 .7-1.2l.3-.2a1.3 1.3 0 0 1 1.3 0l.1.1a1.3 1.3 0 0 0 1.8-.5l.1-.3a1.3 1.3 0 0 0-.5-1.8l-.1-.1a1.3 1.3 0 0 1-.7-1.2v-.3a1.3 1.3 0 0 1 .7-1.2l.1-.1a1.3 1.3 0 0 0 .5-1.8l-.1-.3a1.3 1.3 0 0 0-1.8-.5l-.1.1a1.3 1.3 0 0 1-1.3 0l-.3-.2a1.3 1.3 0 0 1-.7-1.2v-.1a1.3 1.3 0 0 0-1.3-1.3z")];
pub const LOCK: Icon = &[Rect(3.5, 7.0, 9.0, 6.5, 1.5), Path("M5.5 7V5.5a2.5 2.5 0 0 1 5 0V7")];
pub const WARN: Icon = &[Path("M8 2 1.8 13h12.4z"), Path("M8 6.5v3M8 11.3v.2")];
pub const ONION: Icon = &[Circle(8.0, 9.0, 5.0), Circle(8.0, 9.0, 3.0), Path("M8 4c0-1.2.6-2 1.6-2.4")];
pub const CLOCK: Icon = &[Circle(8.0, 8.0, 6.0), Path("M8 4.8V8l2.2 1.6")];
pub const BAR: Icon = &[Rect(2.0, 3.0, 12.0, 10.0, 2.0), Path("M2 6.5h12")];
pub const DOWNLOAD: Icon = &[Path("M8 2.5v8M4.8 7.5 8 10.7l3.2-3.2M3 13.5h10")];
pub const ZOOM: Icon = &[Circle(7.0, 7.0, 4.5), Path("m10.4 10.4 3.3 3.3")];
pub const SEARCH: Icon = ZOOM;
pub const EXPAND: Icon = &[Path("M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10")];
pub const PRINT: Icon = &[Path("M4.5 6V2.5h7V6M4.5 11.5h-2v-5h11v5h-2"), Rect(4.5, 9.5, 7.0, 4.0, 0.0)];
pub const TRASH: Icon = &[Path("M2.5 4.5h11M6.5 4.5V3h3v1.5M4 4.5l.7 9h6.6l.7-9")];
pub const INFO: Icon = &[Circle(8.0, 8.0, 6.0), Path("M8 7.2v3.6M8 5.2v.2")];
pub const POWER: Icon = &[Path("M8 2v5.5M4.6 4.2a5 5 0 1 0 6.8 0")];
pub const PALETTE: Icon = &[Path("M8 2a6 6 0 1 0 0 12c.9 0 1.3-.6 1.1-1.3-.3-.9.3-1.7 1.2-1.7H12a2.5 2.5 0 0 0 2-2.5C14 4.6 11.3 2 8 2z"), Circle(5.0, 7.0, 0.6), Circle(8.0, 5.0, 0.6), Circle(11.0, 7.0, 0.6)];
pub const COPY: Icon = &[Rect(5.0, 5.0, 8.5, 8.5, 1.5), Path("M3 10.5V3.5c0-.6.4-1 1-1h7")];
pub const EDIT: Icon = &[Path("M10.5 2.5 13.5 5.5 6 13H3v-3z")];
pub const MASK: Icon = &[Path("M1.5 6.5c2-1.4 4.2-1.6 6.5-.4 2.3-1.2 4.5-1 6.5.4-.2 2.6-1.6 4.4-3.6 4.4-1.3 0-2.2-.8-2.9-1.9-.7 1.1-1.6 1.9-2.9 1.9-2 0-3.4-1.8-3.6-4.4z"), Circle(5.0, 7.8, 0.9), Circle(11.0, 7.8, 0.9)];
pub const USER: Icon = &[Circle(8.0, 5.5, 2.8), Path("M2.8 14c.6-2.8 2.7-4.4 5.2-4.4s4.6 1.6 5.2 4.4")];
pub const EXTERNAL: Icon = &[Path("M9 2.5h4.5V7M13.5 2.5 7.5 8.5M12 9.5v3.5a.5.5 0 0 1-.5.5h-8a.5.5 0 0 1-.5-.5v-8a.5.5 0 0 1 .5-.5H7")];
pub const PIN: Icon = &[Path("M9.6 1.9 14 6.3l-1.3.4-2.9 2.9-.4 3.1-1.1 1.1-2.2-2.2-3 3-.9-.1v-.9l3-3-2.2-2.2 1.1-1.1 3.1-.4 2.9-2.9z")];
pub const UP: Icon = &[Path("m4 10 4-4 4 4")];
pub const DOWN: Icon = &[Path("m4 6 4 4 4-4")];
pub const CHECK: Icon = &[Path("m3.5 8.5 3 3 6-7")];
pub const GLOBE: Icon = &[Circle(8.0, 8.0, 6.0), Path("M2 8h12M8 2c1.8 1.7 2.6 3.7 2.6 6S9.8 12.3 8 14C6.2 12.3 5.4 10.3 5.4 8S6.2 3.7 8 2z")];
// Window buttons (10×10 glyphs in a 16 box).
pub const WIN_MIN: Icon = &[Path("M3 8.5h10")];
pub const WIN_MAX: Icon = &[Rect(3.5, 3.5, 9.0, 9.0, 0.5)];
pub const WIN_RESTORE: Icon = &[Rect(3.5, 5.5, 7.0, 7.0, 0.5), Path("M5.5 5.5V3.5h7v7h-2")];
pub const WIN_CLOSE: Icon = &[Path("M3.5 3.5l9 9M12.5 3.5l-9 9")];

/// Parse a path into flattened subpaths (in icon units). Returns (points, closed).
pub fn flatten(d: &str) -> Vec<(Vec<Pos2>, bool)> {
    let toks = tokenize(d);
    let mut out: Vec<(Vec<Pos2>, bool)> = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    let mut p = Pos2::ZERO;
    let mut start = Pos2::ZERO;
    let mut last_ctrl: Option<Pos2> = None;
    let mut i = 0;
    let mut cmd = 'M';
    let num = |toks: &[Tok], i: &mut usize| -> f32 {
        match toks.get(*i) {
            Some(Tok::N(v)) => {
                *i += 1;
                *v
            }
            _ => 0.0,
        }
    };
    while i < toks.len() {
        if let Tok::C(c) = toks[i] {
            cmd = c;
            i += 1;
            if c == 'z' || c == 'Z' {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), true));
                }
                p = start;
                last_ctrl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { p.to_vec2() } else { Vec2::ZERO };
        match cmd.to_ascii_uppercase() {
            'M' => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), false));
                }
                let x = num(&toks, &mut i);
                let y = num(&toks, &mut i);
                p = Pos2::new(x, y) + base;
                start = p;
                cur.push(p);
                // Further pairs after M are implicit L.
                cmd = if rel { 'l' } else { 'L' };
                last_ctrl = None;
            }
            'L' => {
                let x = num(&toks, &mut i);
                let y = num(&toks, &mut i);
                p = Pos2::new(x, y) + base;
                cur.push(p);
                last_ctrl = None;
            }
            'H' => {
                let x = num(&toks, &mut i);
                p = Pos2::new(if rel { p.x + x } else { x }, p.y);
                cur.push(p);
                last_ctrl = None;
            }
            'V' => {
                let y = num(&toks, &mut i);
                p = Pos2::new(p.x, if rel { p.y + y } else { y });
                cur.push(p);
                last_ctrl = None;
            }
            'C' | 'S' => {
                let (c1, c2, e);
                if cmd.to_ascii_uppercase() == 'C' {
                    c1 = Pos2::new(num(&toks, &mut i), num(&toks, &mut i)) + base;
                } else {
                    c1 = last_ctrl.map(|c| p + (p - c)).unwrap_or(p);
                }
                c2 = Pos2::new(num(&toks, &mut i), num(&toks, &mut i)) + base;
                e = Pos2::new(num(&toks, &mut i), num(&toks, &mut i)) + base;
                for k in 1..=10 {
                    let t = k as f32 / 10.0;
                    let u = 1.0 - t;
                    let q = p.to_vec2() * (u * u * u) + c1.to_vec2() * (3.0 * u * u * t) + c2.to_vec2() * (3.0 * u * t * t) + e.to_vec2() * (t * t * t);
                    cur.push(q.to_pos2());
                }
                last_ctrl = Some(c2);
                p = e;
            }
            'Q' => {
                let c1 = Pos2::new(num(&toks, &mut i), num(&toks, &mut i)) + base;
                let e = Pos2::new(num(&toks, &mut i), num(&toks, &mut i)) + base;
                for k in 1..=8 {
                    let t = k as f32 / 8.0;
                    let u = 1.0 - t;
                    let q = p.to_vec2() * (u * u) + c1.to_vec2() * (2.0 * u * t) + e.to_vec2() * (t * t);
                    cur.push(q.to_pos2());
                }
                last_ctrl = None;
                p = e;
            }
            'A' => {
                let rx = num(&toks, &mut i);
                let ry = num(&toks, &mut i);
                let rot = num(&toks, &mut i);
                let large = flag(&toks, &mut i);
                let sweep = flag(&toks, &mut i);
                let e = Pos2::new(num(&toks, &mut i), num(&toks, &mut i)) + base;
                arc(&mut cur, p, rx, ry, rot, large, sweep, e);
                p = e;
                last_ctrl = None;
            }
            _ => {
                i += 1;
            }
        }
    }
    if cur.len() > 1 {
        out.push((cur, false));
    }
    out
}

#[derive(Clone, Copy, Debug)]
enum Tok {
    C(char),
    N(f32),
}

fn flag(toks: &[Tok], i: &mut usize) -> bool {
    match toks.get(*i) {
        Some(Tok::N(v)) => {
            *i += 1;
            *v != 0.0
        }
        _ => false,
    }
}

fn tokenize(d: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    let mut in_arc_flags = 0usize; // arc flags may be written without separators ("0 0 1" or "011")
    let mut arc_idx = 0usize;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() {
            out.push(Tok::C(c));
            in_arc_flags = if c == 'a' || c == 'A' { 1 } else { 0 };
            arc_idx = 0;
            i += 1;
        } else if c == '-' || c == '.' || c.is_ascii_digit() {
            // Flags (4th and 5th arc numbers) are single digits.
            if in_arc_flags == 1 && (arc_idx % 7 == 3 || arc_idx % 7 == 4) && (c == '0' || c == '1') {
                out.push(Tok::N(if c == '1' { 1.0 } else { 0.0 }));
                arc_idx += 1;
                i += 1;
                continue;
            }
            let st = i;
            let mut seen_dot = false;
            if b[i] == b'-' {
                i += 1;
            }
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if let Ok(v) = d[st..i].parse::<f32>() {
                out.push(Tok::N(v));
                arc_idx += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<Pos2>, p0: Pos2, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, p1: Pos2) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-4 || ry < 1e-4 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (cs, sn) = (phi.cos(), phi.sin());
    let d = (p0 - p1) / 2.0;
    let x1p = cs * d.x + sn * d.y;
    let y1p = -sn * d.x + cs * d.y;
    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        rx *= lam.sqrt();
        ry *= lam.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let mid = (p0.to_vec2() + p1.to_vec2()) / 2.0;
    let c = Vec2::new(cs * cxp - sn * cyp, sn * cxp + cs * cyp) + mid;
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        a
    };
    let t1 = ang(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dt = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
    if !sweep && dt > 0.0 {
        dt -= std::f32::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f32::consts::TAU;
    }
    let n = ((dt.abs() / 0.25).ceil() as usize).clamp(2, 32);
    for k in 1..=n {
        let t = t1 + dt * k as f32 / n as f32;
        let (x, y) = (rx * t.cos(), ry * t.sin());
        out.push(Pos2::new(cs * x - sn * y + c.x, sn * x + cs * y + c.y));
    }
}

/// Map icon units (0..16) into `rect`.
fn mapper(rect: Rect) -> impl Fn(Pos2) -> Pos2 {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - Vec2::splat(8.0 * s);
    move |q: Pos2| o + q.to_vec2() * s
}

fn element_points(el: &El) -> Vec<(Vec<Pos2>, bool)> {
    match *el {
        Path(d) => flatten(d),
        Circle(cx, cy, r) => {
            let n = 28;
            vec![((0..n).map(|k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                Pos2::new(cx + r * a.cos(), cy + r * a.sin())
            }).collect(), true)]
        }
        Rect(x, y, w, h, rx) => {
            let r = egui::Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
            vec![(crate::paint::rrect_points(r, egui::CornerRadius::same(rx.round() as u8)), true)]
        }
    }
}

/// Draw an icon with stroked lines (width in icon units, like SVG stroke-width).
pub fn draw(p: &Painter, rect: Rect, icon: Icon, color: Color32, width: f32) {
    draw_filled(p, rect, icon, color, width, None);
}

/// Draw rotated by `angle` radians around the icon's centre.
pub fn draw_rot(p: &Painter, rect: Rect, icon: Icon, color: Color32, width: f32, angle: f32) {
    let m = mapper(rect);
    let s = rect.width().min(rect.height()) / 16.0;
    let c = rect.center();
    let rot = egui::emath::Rot2::from_angle(angle);
    let stroke = Stroke::new(width * s, color);
    for el in icon {
        for (pts, closed) in element_points(el) {
            let pts: Vec<Pos2> = pts.into_iter().map(|q| c + rot * (m(q) - c)).collect();
            p.add(if closed { Shape::closed_line(pts, stroke) } else { Shape::line(pts, stroke) });
        }
    }
}

/// Like `draw`, optionally filling closed shapes first.
pub fn draw_filled(p: &Painter, rect: Rect, icon: Icon, color: Color32, width: f32, fill: Option<Color32>) {
    let m = mapper(rect);
    let s = rect.width().min(rect.height()) / 16.0;
    let stroke = Stroke::new(width * s, color);
    for el in icon {
        for (pts, closed) in element_points(el) {
            let pts: Vec<Pos2> = pts.into_iter().map(&m).collect();
            if let (Some(f), true) = (fill, closed) {
                crate::paint::fill_fan(p, &pts, f);
            }
            if closed {
                p.add(Shape::closed_line(pts, stroke));
            } else {
                p.add(Shape::line(pts, stroke));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_all() {
        for icon in [BACK, RELOAD, STAR, SHIELD, PUZZLE, COG, LOCK, POWER, EXPAND, PRINT] {
            for el in icon {
                for (pts, _) in element_points(el) {
                    assert!(pts.len() >= 2);
                    for q in pts {
                        assert!(q.x > -2.0 && q.x < 18.0 && q.y > -2.0 && q.y < 18.0, "{q:?}");
                    }
                }
            }
        }
    }
}
