//! Drawing helpers: colour mixing, gradients, rounded gradient shapes, shadows.

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Vec2};

/// `t` of `a` mixed with `1-t` of `b` (like CSS color-mix(in srgb, a t%, b)).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 * t + y as f32 * (1.0 - t)).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn sample(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
            return lerp_premul(a.1, b.1, k);
        }
    }
    stops[stops.len() - 1].1
}

fn lerp_premul(a: Color32, b: Color32, k: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Horizontal multi-stop gradient filling a plain rect (exact: one strip per stop).
pub fn hgrad(p: &Painter, rect: Rect, stops: &[(f32, Color32)]) {
    let mut mesh = Mesh::default();
    let mut xs: Vec<f32> = stops.iter().map(|s| s.0).collect();
    if xs[0] > 0.0 {
        xs.insert(0, 0.0);
    }
    if *xs.last().unwrap() < 1.0 {
        xs.push(1.0);
    }
    for (i, t) in xs.iter().enumerate() {
        let x = rect.left() + rect.width() * t;
        let c = sample(stops, *t);
        mesh.vertices.push(Vertex { pos: Pos2::new(x, rect.top()), uv: WHITE_UV, color: c });
        mesh.vertices.push(Vertex { pos: Pos2::new(x, rect.bottom()), uv: WHITE_UV, color: c });
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b, b + 1);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// Outline of a rounded rect as a polygon (clockwise from the top-left corner).
pub fn rrect_points(rect: Rect, r: CornerRadius) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut out = Vec::new();
    let mut add = |center: Pos2, rad: f32, a0: f32, sharp: Pos2| {
        if rad <= 0.5 {
            out.push(sharp);
        } else {
            // Enough segments that the curve looks round at any size.
            let n = ((rad * 0.9) as usize).clamp(4, 24);
            for i in 0..=n {
                out.push(center + Vec2::angled(a0 + (i as f32 / n as f32) * FRAC_PI_2) * rad);
            }
        }
    };
    let (nw, ne, se, sw) = (r.nw as f32, r.ne as f32, r.se as f32, r.sw as f32);
    add(rect.left_top() + Vec2::new(nw, nw), nw, PI, rect.left_top());
    add(rect.right_top() + Vec2::new(-ne, ne), ne, -FRAC_PI_2, rect.right_top());
    add(rect.right_bottom() + Vec2::new(-se, -se), se, 0.0, rect.right_bottom());
    add(rect.left_bottom() + Vec2::new(sw, -sw), sw, FRAC_PI_2, rect.left_bottom());
    out
}

/// Linear gradient (angle in CSS degrees: 90 = left→right, 135 = top-left→bottom-right) in a rounded rect.
pub fn grad_rrect(p: &Painter, rect: Rect, r: impl Into<CornerRadius>, stops: &[(f32, Color32)], angle_deg: f32) {
    let pts = rrect_points(rect, r.into());
    let a = (angle_deg - 90.0).to_radians();
    let dir = Vec2::new(a.cos(), a.sin());
    // CSS gradient line length so corners hit 0 and 1.
    let len = (rect.width() * dir.x.abs() + rect.height() * dir.y.abs()).max(1.0);
    let c = rect.center();
    let t_of = |q: Pos2| 0.5 + (q - c).dot(dir) / len;
    let colors: Vec<Color32> = pts.iter().map(|q| sample(stops, t_of(*q))).collect();
    fan_smooth(p, c, sample(stops, 0.5), &pts, &colors);
}

/// Fill a polygon as a fan from `center`, with a one-pixel soft edge (like egui's own
/// shapes) so its outline isn't jagged.
fn fan_smooth(p: &Painter, center: Pos2, center_color: Color32, pts: &[Pos2], colors: &[Color32]) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let feather = 1.0 / p.ctx().pixels_per_point();
    // Outward direction depends on winding.
    let area: f32 = (0..n).map(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        a.x * b.y - b.x * a.y
    }).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let out_dir = |i: usize| -> Vec2 {
        let prev = pts[(i + n - 1) % n];
        let next = pts[(i + 1) % n];
        let e1 = (pts[i] - prev).normalized();
        let e2 = (next - pts[i]).normalized();
        let n1 = Vec2::new(e1.y, -e1.x) * sign;
        let n2 = Vec2::new(e2.y, -e2.x) * sign;
        let m = (n1 + n2).normalized();
        let cos = m.dot(n1).max(0.3);
        if m.is_finite() { m / cos } else { n1 }
    };
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex { pos: center, uv: WHITE_UV, color: center_color });
    for i in 0..n {
        let d = out_dir(i) * (feather * 0.5);
        mesh.vertices.push(Vertex { pos: pts[i] - d, uv: WHITE_UV, color: colors[i] });
        mesh.vertices.push(Vertex { pos: pts[i] + d, uv: WHITE_UV, color: Color32::TRANSPARENT });
    }
    let n = n as u32;
    for i in 0..n {
        let j = (i + 1) % n;
        let (ii, io, ji, jo) = (1 + 2 * i, 2 + 2 * i, 1 + 2 * j, 2 + 2 * j);
        mesh.add_triangle(0, ii, ji);
        mesh.add_triangle(ii, io, ji);
        mesh.add_triangle(io, jo, ji);
    }
    p.add(Shape::mesh(mesh));
}

/// The two-colour accent gradient (or flat accent when gradients are off).
pub fn accent_fill(p: &Painter, rect: Rect, r: impl Into<CornerRadius>, pal: &crate::themes::Palette) {
    if pal.gradients {
        grad_rrect(p, rect, r, &[(0.0, pal.accent), (1.0, pal.accent2)], 135.0);
    } else {
        p.rect_filled(rect, r, pal.accent);
    }
}

/// Fill a star-shaped polygon (fan from its centroid), e.g. icons with concave outlines.
pub fn fill_fan(p: &Painter, pts: &[Pos2], color: Color32) {
    if pts.len() < 3 {
        return;
    }
    let c = pts.iter().fold(Vec2::ZERO, |a, q| a + q.to_vec2()) / pts.len() as f32;
    fan_smooth(p, c.to_pos2(), color, pts, &vec![color; pts.len()]);
}

/// Soft drop shadow under a rounded card.
pub fn shadow(p: &Painter, rect: Rect, radius: f32, dark: bool) {
    let base = if dark { 0.30 } else { 0.14 };
    for i in 1..=8 {
        let grow = i as f32 * 1.6;
        let a = base * (1.0 - i as f32 / 9.0) * 0.35;
        p.rect_filled(rect.expand(grow).translate(Vec2::new(0.0, 4.0)), (radius + grow) as u8, alpha(Color32::BLACK, a));
    }
}
