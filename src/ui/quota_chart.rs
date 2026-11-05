//! One week of account quota: what has been used, and where the pace so far leads.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, pango};

use crate::providers::AgentProvider;
use crate::usage::quota_history::{Outlook, QuotaSample, QuotaWeek, Reading};

const HEIGHT: i32 = 128;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct QuotaChart {
        pub week: RefCell<Option<QuotaWeek>>,
        pub hover: Cell<Option<f64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QuotaChart {
        const NAME: &'static str = "AgtkQuotaChart";
        type Type = super::QuotaChart;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_css_name("quota-chart");
            class.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for QuotaChart {}

    impl WidgetImpl for QuotaChart {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (240, 400, -1, -1),
                _ => (HEIGHT, HEIGHT, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(week) = self.week.borrow().as_ref() {
                draw(&self.obj(), snapshot, week, self.hover.get());
            }
        }
    }
}

glib::wrapper! {
    pub struct QuotaChart(ObjectSubclass<imp::QuotaChart>) @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl QuotaChart {
    fn new() -> Self {
        glib::Object::new()
    }
}

/// A provider's week: what has been used and the outlook, then the chart. Hovering the
/// chart puts the reading under the pointer in place of the outlook.
#[derive(Clone)]
pub(super) struct QuotaWeekView {
    pub root: gtk::Box,
    title: gtk::Label,
    note: gtk::Label,
    chart: QuotaChart,
}

impl QuotaWeekView {
    pub fn new() -> Self {
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        let note = gtk::Label::new(None);
        note.set_xalign(0.0);
        note.add_css_class("dim-label");
        let chart = QuotaChart::new();
        chart.set_margin_top(6);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.set_margin_start(16);
        root.set_margin_end(16);
        root.set_margin_bottom(12);
        root.append(&title);
        root.append(&note);
        root.append(&chart);
        root.set_visible(false);
        let motion = gtk::EventControllerMotion::new();
        let on_enter = note.clone();
        motion.connect_enter(move |motion, x, _| hover(motion, &on_enter, Some(x)));
        let on_motion = note.clone();
        motion.connect_motion(move |motion, x, _| hover(motion, &on_motion, Some(x)));
        let on_leave = note.clone();
        motion.connect_leave(move |motion| hover(motion, &on_leave, None));
        chart.add_controller(motion);
        Self {
            root,
            title,
            note,
            chart,
        }
    }

    /// Show a new reading. Without one the last week stays, until it is over.
    pub fn set(&self, week: Option<QuotaWeek>, now: u64) {
        let imp = self.chart.imp();
        let Some(week) = week.or_else(|| imp.week.take().filter(|week| week.end > now)) else {
            self.root.set_visible(false);
            return;
        };
        let title = format!(
            "{} this week: {:.0}% used",
            name(week.provider),
            week.latest.used
        );
        let note = outlook_note(&week);
        self.title.set_text(&title);
        if imp.hover.get().is_none() {
            self.note.set_text(&note);
        }
        self.chart
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{title}. {note}"
            ))]);
        if matches!(week.outlook(), Outlook::RunsOut(_) | Outlook::LimitReached) {
            self.chart.add_css_class("warning");
        } else {
            self.chart.remove_css_class("warning");
        }
        imp.week.replace(Some(week));
        self.chart.queue_draw();
        self.root.set_visible(true);
    }
}

fn hover(motion: &gtk::EventControllerMotion, note: &gtk::Label, x: Option<f64>) {
    let Some(chart) = motion.widget().and_downcast::<QuotaChart>() else {
        return;
    };
    chart.imp().hover.set(x);
    chart.queue_draw();
    if let Some(week) = chart.imp().week.borrow().as_ref() {
        note.set_text(&match x {
            Some(x) => hover_note(week, Plot::new(&chart, week).time(x)),
            None => outlook_note(week),
        });
    }
}

fn name(provider: AgentProvider) -> &'static str {
    match provider {
        AgentProvider::Claude => "Claude",
        AgentProvider::Codex => "Codex",
    }
}

fn outlook_note(week: &QuotaWeek) -> String {
    match week.outlook() {
        Outlook::LimitReached => "Limit reached until the reset".to_owned(),
        Outlook::TooEarly => "Too early in the week to project".to_owned(),
        Outlook::AtReset(end) => format!("At this pace: {end:.0}% used by the reset"),
        Outlook::RunsOut(at) => format!("At this pace it runs out {}", clock(at)),
    }
}

fn hover_note(week: &QuotaWeek, at: u64) -> String {
    let when = clock(at);
    match week.reading_at(at) {
        Reading::Recorded(used) => format!("{when}: {used:.0}% used"),
        Reading::Projected(used) => format!("{when}: about {used:.0}% at this pace"),
        Reading::NotRecorded if at > week.latest.at => format!("{when}: too early to project"),
        Reading::NotRecorded => format!("{when}: not recorded"),
    }
}

fn local(at: u64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(at as i64).ok()
}

fn clock(at: u64) -> String {
    local(at)
        .and_then(|time| time.format("%a %H:%M").ok())
        .map(String::from)
        .unwrap_or_default()
}

/// Local midnights after `start` and before `end`.
fn midnights(start: u64, end: u64) -> Vec<u64> {
    let mut midnights = Vec::new();
    let Some(mut day) = local(start).and_then(|time| {
        glib::DateTime::new(
            &time.timezone(),
            time.year(),
            time.month(),
            time.day_of_month(),
            0,
            0,
            0.0,
        )
        .ok()
    }) else {
        return midnights;
    };
    while let Ok(next) = day.add_days(1) {
        let Ok(at) = u64::try_from(next.to_unix()) else {
            break;
        };
        if at >= end {
            break;
        }
        midnights.push(at);
        day = next;
    }
    midnights
}

/// Maps the week onto the widget, leaving room for the percentage and day labels.
#[derive(Clone, Copy)]
struct Plot {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    start: u64,
    end: u64,
}

impl Plot {
    fn new(widget: &impl IsA<gtk::Widget>, week: &QuotaWeek) -> Self {
        let (label_width, label_height) = widget.create_pango_layout(Some("100%")).pixel_size();
        Self {
            left: f64::from(label_width) + 8.0,
            right: f64::from(widget.width()) - 6.0,
            top: f64::from(label_height) / 2.0 + 1.0,
            bottom: f64::from(widget.height() - label_height) - 6.0,
            start: week.start,
            end: week.end,
        }
    }

    fn x(&self, at: u64) -> f64 {
        let along = at.saturating_sub(self.start) as f64 / (self.end - self.start).max(1) as f64;
        self.left + (self.right - self.left) * along.min(1.0)
    }

    fn y(&self, used: f64) -> f64 {
        self.bottom - (self.bottom - self.top) * used.clamp(0.0, 100.0) / 100.0
    }

    fn point(&self, sample: QuotaSample) -> (f64, f64) {
        (self.x(sample.at), self.y(sample.used))
    }

    fn time(&self, x: f64) -> u64 {
        let along = ((x - self.left) / (self.right - self.left).max(1.0)).clamp(0.0, 1.0);
        self.start + ((self.end - self.start) as f64 * along) as u64
    }
}

fn draw(widget: &QuotaChart, snapshot: &gtk::Snapshot, week: &QuotaWeek, hover: Option<f64>) {
    let plot = Plot::new(widget, week);
    if plot.right <= plot.left || plot.bottom <= plot.top {
        return;
    }
    // Lines take the chart's color, the accent or warning; text and grid take the popover's.
    let line = widget.color();
    let ink = widget
        .parent()
        .map_or_else(|| widget.color(), |parent| parent.color());
    let midnights = midnights(week.start, week.end);
    let bounds = graphene::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32);
    {
        let cr = snapshot.append_cairo(&bounds);
        let paint = |color: &gtk::gdk::RGBA, alpha: f64| {
            cr.set_source_rgba(
                color.red().into(),
                color.green().into(),
                color.blue().into(),
                f64::from(color.alpha()) * alpha,
            );
        };
        cr.set_line_width(1.0);
        paint(&ink, 0.15);
        for used in [0.0, 50.0, 100.0] {
            let y = plot.y(used).round() + 0.5;
            cr.move_to(plot.left, y);
            cr.line_to(plot.right, y);
        }
        let _ = cr.stroke();
        paint(&ink, 0.07);
        for midnight in &midnights {
            let x = plot.x(*midnight).round() + 0.5;
            cr.move_to(x, plot.top);
            cr.line_to(x, plot.bottom);
        }
        let _ = cr.stroke();

        let points = week.line();
        let (now_x, now_y) = plot.point(week.latest);
        cr.move_to(plot.x(week.start), plot.y(0.0));
        for sample in points {
            let (x, y) = plot.point(*sample);
            cr.line_to(x, y);
        }
        cr.line_to(now_x, plot.y(0.0));
        cr.close_path();
        paint(&line, 0.1);
        let _ = cr.fill();

        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        // Stretches with no readings are only known at their ends: a faint line joins them.
        for pair in points.windows(2) {
            if QuotaWeek::is_gap(pair[0], pair[1]) {
                let (x, y) = plot.point(pair[0]);
                cr.move_to(x, y);
                let (x, y) = plot.point(pair[1]);
                cr.line_to(x, y);
            }
        }
        cr.set_line_width(1.5);
        paint(&line, 0.4);
        let _ = cr.stroke();
        let (x, y) = plot.point(points[0]);
        cr.move_to(x, y);
        for pair in points.windows(2) {
            let (x, y) = plot.point(pair[1]);
            if QuotaWeek::is_gap(pair[0], pair[1]) {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.set_line_width(2.0);
        paint(&line, 1.0);
        let _ = cr.stroke();

        if let Some(end) = week.projection_end() {
            let (x, y) = plot.point(end);
            cr.move_to(now_x, now_y);
            cr.line_to(x, y);
            cr.set_dash(&[4.0, 4.0], 0.0);
            paint(&line, 0.8);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }

        if let Some(x) = hover {
            let at = plot.time(x);
            let x = plot.x(at).round() + 0.5;
            cr.set_line_width(1.0);
            paint(&ink, 0.4);
            cr.move_to(x, plot.top);
            cr.line_to(x, plot.bottom);
            let _ = cr.stroke();
            if let Reading::Recorded(used) | Reading::Projected(used) = week.reading_at(at) {
                dot(&cr, &line, x, plot.y(used));
            }
        }
        dot(&cr, &line, now_x, now_y);
    }

    let text = |snapshot: &gtk::Snapshot, layout: &pango::Layout, x: f64, y: f64, alpha: f32| {
        let mut color = ink;
        color.set_alpha(ink.alpha() * alpha);
        snapshot.save();
        snapshot.translate(&graphene::Point::new(x as f32, y as f32));
        snapshot.append_layout(layout, &color);
        snapshot.restore();
    };
    for used in [0.0, 50.0, 100.0] {
        let layout = widget.create_pango_layout(Some(&format!("{used:.0}%")));
        let (width, height) = layout.pixel_size();
        let x = plot.left - 6.0 - f64::from(width);
        text(
            snapshot,
            &layout,
            x,
            plot.y(used) - f64::from(height) / 2.0,
            0.6,
        );
    }
    let edges = std::iter::once(week.start)
        .chain(midnights.iter().copied())
        .chain(std::iter::once(week.end))
        .collect::<Vec<_>>();
    for day in edges.windows(2) {
        let Some(name) = local(day[0]).and_then(|time| time.format("%a").ok()) else {
            continue;
        };
        let layout = widget.create_pango_layout(Some(&name));
        let width = f64::from(layout.pixel_size().0);
        let (from, to) = (plot.x(day[0]), plot.x(day[1]));
        if to - from >= width + 4.0 {
            text(
                snapshot,
                &layout,
                (from + to - width) / 2.0,
                plot.bottom + 4.0,
                0.6,
            );
        }
    }

    // Name the latest reading and where the projection ends, unless the pointer is reading.
    if hover.is_some() {
        return;
    }
    // The line arrives from the lower left and the projection leaves to the upper right,
    // so a label goes above left of its point, or below right where that has no room.
    let place = |layout: &pango::Layout, (x, y): (f64, f64)| {
        let (width, height) = layout.pixel_size();
        let (width, height) = (f64::from(width), f64::from(height));
        let (x, y) = if y - height - 4.0 >= 0.0 {
            (x - width - 4.0, y - height - 4.0)
        } else {
            (x + 8.0, y + 6.0)
        };
        let x = x.clamp(plot.left + 2.0, plot.right - width);
        graphene::Rect::new(x as f32, y as f32, width as f32, height as f32)
    };
    let latest = widget.create_pango_layout(Some(&format!("{:.0}%", week.latest.used)));
    let latest_at = place(&latest, plot.point(week.latest));
    text(
        snapshot,
        &latest,
        latest_at.x().into(),
        latest_at.y().into(),
        1.0,
    );
    if let Some(end) = week.projection_end().filter(|end| end.at == week.end) {
        let layout = widget.create_pango_layout(Some(&format!("{:.0}%", end.used)));
        let end_at = place(&layout, plot.point(end));
        if end_at.intersection(&latest_at).is_none() {
            text(snapshot, &layout, end_at.x().into(), end_at.y().into(), 0.8);
        }
    }
}

/// A reading's dot, cut out of what lies under it so it stays clear on the lines.
fn dot(cr: &gtk::cairo::Context, color: &gtk::gdk::RGBA, x: f64, y: f64) {
    cr.set_operator(gtk::cairo::Operator::Clear);
    cr.arc(x, y, 6.0, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
    cr.set_operator(gtk::cairo::Operator::Over);
    cr.set_source_rgba(
        color.red().into(),
        color.green().into(),
        color.blue().into(),
        color.alpha().into(),
    );
    cr.arc(x, y, 4.0, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::quota_history::QuotaSample;

    /// Three days of a week: busy during the day, nothing read at night.
    fn sample_week(now: u64) -> QuotaWeek {
        let start = now - 3 * 86_400 - 5 * 3600;
        let mut samples = Vec::new();
        let mut used: f64 = 0.0;
        let mut at = start + 2 * 3600;
        while at < now {
            let hour = (at - start) / 3600 % 24;
            if (2..14).contains(&hour) {
                used += if hour.is_multiple_of(5) { 0.2 } else { 0.07 };
                samples.push(QuotaSample { at, used });
            }
            at += 300;
        }
        QuotaWeek::new(
            AgentProvider::Claude,
            start + 7 * 86_400,
            QuotaSample { at: now, used },
            samples,
        )
    }

    #[test]
    #[ignore = "requires a private display"]
    fn quota_week_renders() {
        gtk::init().unwrap();
        adw::init().unwrap();
        let display = gtk::gdk::Display::default().unwrap();
        let _style = super::super::style::ChromeStyle::install(&display);
        let now = crate::persist::now_millis() / 1000;
        let directory = std::env::var("AGTK_TEST_CAPTURE_DIR").ok();
        for dark in [false, true] {
            adw::StyleManager::default().set_color_scheme(if dark {
                adw::ColorScheme::ForceDark
            } else {
                adw::ColorScheme::ForceLight
            });
            let view = QuotaWeekView::new();
            let busy = sample_week(now);
            let heavy = QuotaWeek::new(
                AgentProvider::Codex,
                busy.end,
                QuotaSample {
                    at: now,
                    used: (busy.latest.used * 2.2).min(100.0),
                },
                busy.line()[1..busy.line().len() - 1]
                    .iter()
                    .map(|sample| QuotaSample {
                        at: sample.at,
                        used: (sample.used * 2.2).min(100.0),
                    }),
            );
            view.set(Some(busy.clone()), now);
            let warning = QuotaWeekView::new();
            warning.set(Some(heavy), now);
            let hovered = QuotaWeekView::new();
            hovered.set(Some(busy), now);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
            content.set_margin_top(12);
            content.append(&view.root);
            content.append(&warning.root);
            content.append(&hovered.root);
            let window = gtk::Window::builder()
                .default_width(432)
                .child(&content)
                .build();
            window.present();
            let context = glib::MainContext::default();
            let settle = || {
                let until = std::time::Instant::now() + std::time::Duration::from_millis(200);
                while std::time::Instant::now() < until {
                    while context.pending() {
                        context.iteration(false);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            };
            settle();
            let chart = &hovered.chart;
            let week = chart.imp().week.borrow().clone().unwrap();
            let x = f64::from(chart.width()) * 0.3;
            chart.imp().hover.set(Some(x));
            hovered
                .note
                .set_text(&hover_note(&week, Plot::new(chart, &week).time(x)));
            chart.queue_draw();
            settle();
            assert!(view.root.is_visible() && chart.height() == HEIGHT);
            if let Some(directory) = &directory {
                let paintable = gtk::WidgetPaintable::new(Some(&window));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
                let node = snapshot.to_node().expect("chart rendered");
                let renderer = gtk::gsk::CairoRenderer::new();
                renderer.realize_for_display(&display).unwrap();
                let texture = renderer.render_texture(&node, None);
                renderer.unrealize();
                let name = if dark {
                    "quota-week-dark.png"
                } else {
                    "quota-week.png"
                };
                texture
                    .save_to_png(std::path::Path::new(directory).join(name))
                    .unwrap();
            }
            window.close();
        }
    }

    #[test]
    fn midnights_split_the_week_into_local_days() {
        let start = 1_790_000_000;
        let end = start + 7 * 24 * 60 * 60;
        let midnights = midnights(start, end);
        assert!((6..=8).contains(&midnights.len()), "{midnights:?}");
        assert!(midnights.iter().all(|at| (start..end).contains(at)));
        for midnight in &midnights {
            let time = local(*midnight).unwrap();
            assert_eq!((time.hour(), time.minute()), (0, 0));
        }
    }
}
