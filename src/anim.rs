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
    let mut tw = tw.unwrap_or(Tw { from: target, to: target, start: now - 10.0, dur });
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
        ctx.request_repaint();
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
        ctx.request_repaint();
    }
    k
}
