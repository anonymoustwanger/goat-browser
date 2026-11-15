//! Smooth animation helpers: eased tweens with Goat's curves (ease-out and a slight "spring"
//! overshoot), driven by egui's clock. Turned off by Settings → Animations.

use egui::{Context, Id};

/// cubic-bezier(.16, 1, .3, 1)-like: fast start, soft landing.
pub fn out_expo(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t >= 1.0 { 1.0 } else { 1.0 - 2f32.powf(-10.0 * t) }
}

pub fn out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn in_cubic(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(3)
}

/// CSS cubic-bezier(x1, y1, x2, y2) evaluated at time `t`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return t;
    }
    let b = |a: f32, c: f32, s: f32| 3.0 * a * s * (1.0 - s).powi(2) + 3.0 * c * s * s * (1.0 - s) + s.powi(3);
    // Solve x(s) = t (x is monotonic for these curves).
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if b(x1, x2, mid) < t { lo = mid } else { hi = mid }
    }
    b(y1, y2, (lo + hi) / 2.0)
}

/// Hover grow: shoots up fast (exponential), overshoots ~14%, bounces back once and settles.
pub fn ease_bounce(t: f32) -> f32 {
    if t >= 1.0 {
        return 1.0;
    }
    let t = t.max(0.0);
    1.0 - (-5.0 * t).exp() * (2.6 * std::f32::consts::PI * t).cos()
}

/// Goat's --ease-spring: cubic-bezier(.34, 1.56, .64, 1). Overshoots ~10%, then settles.
pub fn spring(t: f32) -> f32 {
    bezier(0.34, 1.56, 0.64, 1.0, t)
}

/// Panel pop-in: scale from `from` to 1 as a quick, lightly damped spring. It reaches full size
/// in ~0.05s, overshoots about 3%, and settles by ~0.25s.
pub fn pop(t: f32, from: f32) -> f32 {
    if t <= 0.0 {
        return from;
    }
    let (zeta, omega) = (0.36f32, 52.0f32);
    let wd = omega * (1.0 - zeta * zeta).sqrt();
    let decay = (-zeta * omega * t).exp();
    if decay < 0.01 {
        return 1.0;
    }
    1.0 - (1.0 - from) * decay * ((wd * t).cos() + zeta * omega / wd * (wd * t).sin())
}

/// Goat's "bump"/"pop": grows to `peak`× by 40%, then springs back to 1× (slight dip).
pub fn bump(t: f32, peak: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t >= 1.0 {
        1.0
    } else if t < 0.4 {
        1.0 + (peak - 1.0) * out_cubic(t / 0.4)
    } else {
        1.0 + (peak - 1.0) * (1.0 - spring((t - 0.4) / 0.6))
    }
}


#[derive(Clone, Copy)]
struct Tw {
    from: f32,
    to: f32,
    start: f64,
    dur: f32,
}

pub fn enabled(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("goatr-anim")).unwrap_or(true))
}

pub fn set_enabled(ctx: &Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new("goatr-anim"), on));
}

/// Animate towards `target` over `dur` seconds with `ease`. Retargeting mid-flight continues
/// smoothly from the current value.
pub fn tween(ctx: &Context, id: Id, target: f32, dur: f32, ease: fn(f32) -> f32) -> f32 {
    if !enabled(ctx) {
        ctx.data_mut(|d| d.insert_temp(id, Tw { from: target, to: target, start: 0.0, dur }));
        return target;
    }
    let now = ctx.input(|i| i.time);
    let tw: Option<Tw> = ctx.data(|d| d.get_temp(id));
    // First sighting: remember the resting value, so the first change animates too.
    let mut tw = match tw {
        Some(t) => t,
        None => {
            let t = Tw { from: target, to: target, start: now - 10.0, dur };
            ctx.data_mut(|d| d.insert_temp(id, t));
            t
        }
    };
    let value_at = |tw: &Tw| {
        let k = ((now - tw.start) as f32 / tw.dur.max(0.001)).clamp(0.0, 1.0);
        tw.from + (tw.to - tw.from) * ease(k)
    };
    if (tw.to - target).abs() > 1e-4 {
        tw = Tw { from: value_at(&tw), to: target, start: now, dur };
        ctx.data_mut(|d| d.insert_temp(id, tw));
    }
    let k = ((now - tw.start) as f32 / tw.dur.max(0.001)).clamp(0.0, 1.0);
    if k < 1.0 {
        // Pop-ups are drawn as part of Goat's main window, so keep that redrawing too.
        ctx.request_repaint();
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    tw.from + (tw.to - tw.from) * ease(k)
}

/// 0→1 when `on`, eased; for hover highlights and fades.
pub fn hover(ctx: &Context, id: Id, on: bool, dur: f32) -> f32 {
    tween(ctx, id, if on { 1.0 } else { 0.0 }, dur, out_cubic)
}

/// Progress 0..1 of a one-shot animation that started at `start` (egui time).
pub fn since(ctx: &Context, start: f64, dur: f32) -> f32 {
    if !enabled(ctx) {
        return 1.0;
    }
    let k = ((ctx.input(|i| i.time) - start) as f32 / dur).clamp(0.0, 1.0);
    if k < 1.0 {
        // Pop-ups are drawn as part of Goat's main window, so keep that redrawing too.
        ctx.request_repaint();
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    k
}
