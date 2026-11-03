//! Palette, type and widgets, with the same measures as the C# version's Theme.xaml.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Id, Pos2, Rect, Response,
    Sense, Stroke, StrokeKind, Ui, Vec2,
};

pub const PAPER: Color32 = Color32::from_rgb(0xF1, 0xF1, 0xEF);
pub const SURFACE: Color32 = Color32::from_rgb(0xE4, 0xE4, 0xE1);
pub const LINE: Color32 = Color32::from_rgb(0xD3, 0xD3, 0xCF);
pub const MUTED: Color32 = Color32::from_rgb(0x73, 0x73, 0x73);
pub const INK: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
pub const INK_HOVER: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const MUTED_ON_INK: Color32 = Color32::from_rgb(0x8C, 0x8C, 0x8C);
pub const SIGNAL: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x00);

pub fn sans(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}
pub fn icons(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

/// WPF's IsEnabled=False: the control fades to the given opacity.
pub fn fade(c: Color32, enabled: bool, opacity: f32) -> Color32 {
    if enabled { c } else { c.gamma_multiply(opacity) }
}

/// Segoe UI Variable Text at two weights, Consolas, Segoe Fluent Icons; YaHei for CJK titles.
pub fn install(ctx: &egui::Context) {
    let dir = PathBuf::from(std::env::var_os("WINDIR").unwrap_or("C:\\Windows".into())).join("Fonts");
    let read = |file: &str| std::fs::read(dir.join(file)).ok().map(Arc::new);
    let mut defs = FontDefinitions::default();
    let mut put = |name: &str, data: FontData, families: &[FontFamily]| {
        defs.font_data.insert(name.into(), Arc::new(data));
        for f in families {
            defs.families.entry(f.clone()).or_default().insert(0, name.into());
        }
    };
    let semi = FontFamily::Name("semibold".into());
    let icon = FontFamily::Name("icons".into());
    if let Some(b) = read("msyh.ttc") {
        put("cjk", FontData::from_blob(b, 0), &[FontFamily::Proportional, FontFamily::Monospace, semi.clone()]);
    }
    if let Some(b) = read("consola.ttf") {
        // egui sizes a face by its line box, WPF by its em; Consolas comes out ~10% small without this.
        put("mono", FontData::from_blob(b, 0).tweak(FontTweak { scale: 1.1, ..Default::default() }), &[FontFamily::Monospace]);
    }
    if let Some(b) = read("SegUIVar.ttf") {
        let weight = |w: f32| FontTweak { coords: egui::epaint::text::VariationCoords::new([("wght", w), ("opsz", 10.5)]), ..Default::default() };
        put("ui", FontData::from_blob(b.clone(), 0).tweak(weight(400.0)), &[FontFamily::Proportional]);
        put("ui-semibold", FontData::from_blob(b, 0).tweak(weight(600.0)), std::slice::from_ref(&semi));
    }
    if let Some(b) = read("SegoeIcons.ttf").or_else(|| read("segmdl2.ttf")) {
        put("icons", FontData::from_blob(b, 0), &[icon]);
    }
    ctx.set_fonts(defs);

    ctx.set_theme(egui::Theme::Light);
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        *v = egui::Visuals::light();
        v.panel_fill = PAPER;
        v.window_fill = INK;
        v.window_stroke = Stroke::NONE;
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = egui::epaint::Shadow::NONE;
        v.popup_shadow = egui::epaint::Shadow::NONE;
        v.override_text_color = Some(PAPER); // only tooltips draw text through egui itself
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.scroll.floating = true;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.corner_radius = CornerRadius::ZERO;
            w.bg_fill = LINE;
            w.fg_stroke = Stroke::new(1.0, LINE);
        }
        v.widgets.hovered.bg_fill = MUTED;
        v.widgets.active.bg_fill = INK;
    });
}

/// Ink tooltip, 11 px, as in the C# ToolTip style.
pub fn tip(resp: Response, text: &str) -> Response {
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(text).font(sans(11.0)).color(PAPER));
    })
}

// ---- type -------------------------------------------------------------------------------------

/// "01  Source": T.Num (mono 11, muted, 10 px gap) then T.Section (13 SemiBold).
pub fn section(ui: &Ui, row: Rect, num: &str, title: &str) {
    let p = ui.painter();
    let n = p.text(row.left_center() + Vec2::new(0.0, 0.5), Align2::LEFT_CENTER, num, mono(11.0), MUTED);
    p.text(Pos2::new(n.max.x + 10.0, row.center().y), Align2::LEFT_CENTER, title, semibold(13.0), INK);
}

/// Text cut to `max` px with an ellipsis (WPF CharacterEllipsis). Returns the text and whether it was cut.
pub fn trim(ui: &Ui, text: &str, font: &FontId, max: f32) -> (String, bool) {
    let width = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), INK).size().x);
    if width(text) <= max {
        return (text.to_string(), false);
    }
    let mut t = text.to_string();
    while !t.is_empty() && width(&format!("{t}…")) > max {
        t.pop();
    }
    (format!("{}…", t.trim_end()), true)
}

pub fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font.clone(), INK).size().x)
}

// ---- controls ---------------------------------------------------------------------------------

fn sense(enabled: bool) -> Sense {
    if enabled { Sense::click() } else { Sense::hover() }
}

/// SegFrame + Seg: 1 px ink frame, cells divided by 1 px ink, 12 px labels.
pub fn segmented(ui: &mut Ui, rect: Rect, id: &str, labels: &[&str], value: &mut usize, enabled: bool) -> bool {
    let mut changed = false;
    let w = rect.width() / labels.len() as f32;
    let ink = fade(INK, enabled, 0.4);
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(rect.min + Vec2::new(w * i as f32, 0.0), Vec2::new(w, rect.height()));
        let resp = ui.interact(r, Id::NULL.with((id, i)), sense(enabled));
        let on = *value == i;
        let p = ui.painter();
        if on {
            p.rect_filled(r, 0.0, ink);
        } else if resp.hovered() && enabled {
            p.rect_filled(r, 0.0, SURFACE);
        }
        if i > 0 {
            p.line_segment([Pos2::new(r.min.x, r.min.y), Pos2::new(r.min.x, r.max.y)], Stroke::new(1.0, ink));
        }
        p.text(r.center(), Align2::CENTER_CENTER, *label, sans(12.0), if on { fade(PAPER, enabled, 0.4) } else { ink });
        if resp.clicked() && !on {
            *value = i;
            changed = true;
        }
    }
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside);
    changed
}

/// Switch: 30 x 16 square track, 8 px square knob, 3 px inset; ink when on.
pub fn switch(ui: &mut Ui, center_right: Pos2, id: &str, value: &mut bool, enabled: bool) -> Response {
    let r = Rect::from_min_size(Pos2::new(center_right.x - 30.0, center_right.y - 8.0), Vec2::new(30.0, 16.0));
    let resp = ui.interact(r, Id::NULL.with(("switch", id)), sense(enabled));
    if resp.clicked() {
        *value = !*value;
    }
    // A short slide, like the 120 ms storyboard.
    let t = ui.ctx().animate_bool_with_time(Id::NULL.with(("switch-t", id)), *value, 0.12);
    let ink = fade(INK, enabled, 0.35);
    let p = ui.painter();
    if *value {
        p.rect_filled(r, 0.0, ink);
    }
    p.rect_stroke(r, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside);
    let knob = Rect::from_min_size(Pos2::new(r.min.x + 4.0 + 14.0 * t, r.center().y - 4.0), Vec2::splat(8.0));
    p.rect_filled(knob, 0.0, if *value { fade(PAPER, enabled, 0.35) } else { ink });
    resp
}

/// T.Caption label at the left of `row` and a switch at its right edge.
pub fn switch_row(ui: &mut Ui, row: Rect, label: &str, value: &mut bool, enabled: bool) {
    ui.painter().text(row.left_center(), Align2::LEFT_CENTER, label, sans(12.0), MUTED);
    switch(ui, row.right_center(), label, value, enabled);
}

/// Ghost: 12 px ink text, underlined on hover.
pub fn ghost(ui: &mut Ui, left_center: Pos2, label: &str, enabled: bool) -> (bool, Rect) {
    let w = text_width(ui, label, &sans(12.0));
    let r = Rect::from_min_size(left_center - Vec2::new(0.0, 10.0), Vec2::new(w, 20.0));
    let resp = ui.interact(r, Id::NULL.with(("ghost", label)), sense(enabled));
    let c = fade(INK, enabled, 0.4);
    let p = ui.painter();
    let t = p.text(r.left_center(), Align2::LEFT_CENTER, label, sans(12.0), c);
    if resp.hovered() && enabled {
        p.line_segment([Pos2::new(t.min.x, t.max.y), Pos2::new(t.max.x, t.max.y)], Stroke::new(1.0, c));
    }
    (enabled && resp.clicked(), r)
}

/// Outline: 30 px tall, 16 px side padding, 1 px ink; fills ink on hover.
pub fn outline(ui: &mut Ui, top_left: Pos2, label: &str, enabled: bool) -> (bool, Rect) {
    let w = text_width(ui, label, &sans(12.0)) + 32.0;
    let r = Rect::from_min_size(top_left, Vec2::new(w, 30.0));
    let resp = ui.interact(r, Id::NULL.with(("outline", label)), sense(enabled));
    let ink = fade(INK, enabled, 0.4);
    let hot = resp.hovered() && enabled;
    let p = ui.painter();
    if hot {
        p.rect_filled(r, 0.0, INK);
    }
    p.rect_stroke(r, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, label, sans(12.0), if hot { PAPER } else { ink });
    (enabled && resp.clicked(), r)
}

/// Picker: 30 px, 1 px line border (ink on hover or open), chevron at the right, list below.
pub fn picker(ui: &mut Ui, rect: Rect, id: &str, items: &[String], value: &mut Option<String>, empty: &str, enabled: bool) {
    let pid = Id::NULL.with(("picker", id));
    let mut open = ui.data(|d| d.get_temp::<bool>(pid).unwrap_or(false)) && enabled;
    let resp = ui.interact(rect, pid.with("button"), sense(enabled));
    if resp.clicked() {
        open = !open;
    }
    let p = ui.painter();
    let border = if (resp.hovered() || open) && enabled { INK } else { LINE };
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, fade(border, enabled, 0.4)), StrokeKind::Inside);
    let label = value.clone().unwrap_or_else(|| empty.to_string());
    let (shown, _) = trim(ui, &label, &sans(12.0), rect.width() - 40.0);
    let color = if value.is_some() && !items.is_empty() { INK } else { MUTED };
    let p = ui.painter();
    p.text(rect.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, shown, sans(12.0), fade(color, enabled, 0.4));
    p.text(rect.right_center() - Vec2::new(11.0, 0.0), Align2::RIGHT_CENTER, "\u{E70D}", icons(9.0), fade(MUTED, enabled, 0.4));

    if open {
        let item_h = 30.0;
        let height = (items.len() as f32 * item_h + 2.0).min(260.0);
        let area = egui::Area::new(pid.with("list"))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.left_bottom() + Vec2::new(0.0, 2.0))
            .show(ui.ctx(), |ui| {
                let (frame, _) = ui.allocate_exact_size(Vec2::new(rect.width(), height), Sense::hover());
                ui.painter().rect_filled(frame, 0.0, PAPER);
                let inner = frame.shrink(1.0);
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                    egui::ScrollArea::vertical().max_height(inner.height()).show(ui, |ui| {
                        for item in items {
                            let (r, item_resp) = ui.allocate_exact_size(Vec2::new(inner.width(), item_h), Sense::click());
                            let selected = value.as_deref() == Some(item.as_str());
                            let p = ui.painter();
                            if selected {
                                p.rect_filled(r, 0.0, INK);
                            } else if item_resp.hovered() {
                                p.rect_filled(r, 0.0, SURFACE);
                            }
                            let (t, _) = trim(ui, item, &sans(12.0), r.width() - 20.0);
                            ui.painter().text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, t, sans(12.0), if selected { PAPER } else { INK });
                            if item_resp.clicked() {
                                *value = Some(item.clone());
                                open = false;
                            }
                        }
                    });
                });
                ui.painter().rect_stroke(frame, 0.0, Stroke::new(1.0, INK), StrokeKind::Inside);
            });
        // A click anywhere else closes the list.
        if ui.input(|i| i.pointer.any_click()) && !area.response.contains_pointer() && !resp.contains_pointer() {
            open = false;
        }
    }
    ui.data_mut(|d| d.insert_temp(pid, open));
}

/// Fader: 18 px tall; 2 px ink before the thumb, 1 px line after, a 4 x 14 ink thumb.
pub fn fader(ui: &mut Ui, track: Rect, id: &str, value: &mut f32, range: (f32, f32), step: f32, enabled: bool) {
    let resp = ui.interact(track, Id::NULL.with(("fader", id)), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    if let Some(pos) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let t = ((pos.x - track.min.x - 2.0) / (track.width() - 4.0)).clamp(0.0, 1.0);
        *value = ((range.0 + t * (range.1 - range.0)) / step).round() * step;
    }
    let t = (*value - range.0) / (range.1 - range.0);
    let x = track.min.x + 2.0 + t * (track.width() - 4.0);
    let y = track.center().y;
    let ink = fade(INK, enabled, 0.4);
    let p = ui.painter();
    p.line_segment([Pos2::new(x, y), Pos2::new(track.max.x, y)], Stroke::new(1.0, LINE));
    p.rect_filled(Rect::from_min_max(Pos2::new(track.min.x, y - 1.0), Pos2::new(x, y + 1.0)), 0.0, ink);
    p.rect_filled(Rect::from_center_size(Pos2::new(x, y), Vec2::new(4.0, 14.0)), 0.0, ink);
}

/// LevelMeter: segments with 2 px gaps; the top tenth lights in signal.
pub fn meter(ui: &Ui, rect: Rect, level: f32, segments: usize) {
    let gap = 2.0;
    let w = (rect.width() - gap * (segments - 1) as f32) / segments as f32;
    let lit = (level.clamp(0.0, 1.0) * segments as f32).round() as usize;
    let hot = (segments as f32 * 0.9).ceil() as usize;
    let p = ui.painter();
    for i in 0..segments {
        let x0 = (rect.min.x + i as f32 * (w + gap)).round();
        let x1 = (rect.min.x + i as f32 * (w + gap) + w).round().max(x0 + 1.0);
        let c = if i >= lit { LINE } else if i >= hot { SIGNAL } else { INK };
        p.rect_filled(Rect::from_min_max(Pos2::new(x0, rect.min.y), Pos2::new(x1, rect.max.y)), 0.0, c);
    }
}

/// Corner picker: a 48 x 32 ink frame with four 12 x 8 cells, 3 px from its edges.
pub fn corners(ui: &mut Ui, rect: Rect, value: &mut usize, enabled: bool) {
    let ink = fade(INK, enabled, 0.4);
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, ink), StrokeKind::Inside);
    let cell = Vec2::new(12.0, 8.0);
    for i in 0..4 {
        let x = if i % 2 == 0 { rect.min.x + 3.0 } else { rect.max.x - 3.0 - cell.x };
        let y = if i < 2 { rect.min.y + 3.0 } else { rect.max.y - 3.0 - cell.y };
        let r = Rect::from_min_size(Pos2::new(x, y), cell);
        let resp = ui.interact(r.expand(2.0), Id::NULL.with(("corner", i)), sense(enabled));
        if resp.clicked() {
            *value = i;
        }
        let p = ui.painter();
        if *value == i {
            p.rect_filled(r, 0.0, ink);
        }
        p.rect_stroke(r, 0.0, Stroke::new(1.0, if *value == i { ink } else { fade(MUTED, enabled, 0.4) }), StrokeKind::Inside);
    }
}

/// A 34 px list row: 10 px padding, 12.5 px title, mono 11 meta at 55% opacity.
pub fn row(ui: &mut Ui, title: &str, meta: &str, full: &str, selected: bool, enabled: bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), sense(enabled));
    let fg = if selected { PAPER } else { INK };
    {
        let p = ui.painter();
        if selected {
            p.rect_filled(r, 0.0, INK);
        } else if resp.hovered() && enabled {
            p.rect_filled(r, 0.0, SURFACE);
        }
        p.line_segment([Pos2::new(r.min.x, r.max.y - 0.5), Pos2::new(r.max.x, r.max.y - 0.5)], Stroke::new(1.0, if selected { INK } else { LINE }));
    }
    let meta_w = if meta.is_empty() { 0.0 } else { text_width(ui, meta, &mono(11.0)) + 14.0 };
    let (text, cut) = trim(ui, title, &sans(12.5), r.width() - 20.0 - meta_w);
    let p = ui.painter();
    p.text(r.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, text, sans(12.5), fg);
    if !meta.is_empty() {
        p.text(r.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, meta, mono(11.0), fg.gamma_multiply(0.55));
    }
    let clicked = resp.clicked();
    if cut || full != title {
        tip(resp, full);
    }
    clicked
}

/// The largest rect of the given aspect that fits, centred.
pub fn fit(area: Rect, w: f32, h: f32) -> Rect {
    let k = (area.width() / w).min(area.height() / h);
    Rect::from_center_size(area.center(), Vec2::new(w * k, h * k))
}
