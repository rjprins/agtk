//! Bottom-row presentation and background collection for usage measurements.

use super::*;
use crate::agent_hooks::{auto_compact_window, context_report_path};
use crate::providers::{AgentProvider, DiscoveryRoots};
use crate::usage::quota::{self, AccountQuota};
use crate::usage::session::{ContextReport, SessionUsage, UsageReader};
use crate::usage::system::{CpuSample, Memory};

type UsageSelection = Option<(String, Option<String>)>;
use std::f64::consts::{FRAC_PI_2, TAU};
use std::time::Instant;

/// Context use that turns the ring to the warning color, in percent.
const CONTEXT_WARNING: f64 = 80.0;

#[derive(Clone)]
pub(super) struct Segment {
    pub button: gtk::MenuButton,
    pub label: gtk::Label,
    content: gtk::Box,
    details: gtk::Label,
}

impl Segment {
    fn new(name: &str, text: &str, icon: &str) -> Self {
        let label = gtk::Label::new(Some(text));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_xalign(0.0);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name(icon));
        content.append(&label);
        let details = gtk::Label::new(Some("Waiting for measurements…"));
        details.set_xalign(0.0);
        details.set_selectable(true);
        details.set_wrap(true);
        details.set_max_width_chars(52);
        details.set_margin_start(16);
        details.set_margin_end(16);
        details.set_margin_top(12);
        details.set_margin_bottom(12);
        let popover = gtk::Popover::builder()
            .position(gtk::PositionType::Top)
            .child(&details)
            .build();
        let details_on_open = details.downgrade();
        popover.connect_map(move |_| {
            let details = details_on_open.clone();
            // GTK selects the whole label when the popover gives it focus.
            // Clear that initial selection after focus, preserving manual copying.
            glib::idle_add_local_once(move || {
                if let Some(details) = details.upgrade() {
                    details.select_region(0, 0);
                }
            });
        });
        let button = gtk::MenuButton::builder()
            .child(&content)
            .popover(&popover)
            .has_frame(false)
            .build();
        button.set_widget_name(name);
        button.update_property(&[gtk::accessible::Property::Label(text)]);
        Self {
            button,
            label,
            content,
            details,
        }
    }

    fn set(&self, text: &str, details: &str) {
        self.label.set_text(text);
        if self.details.text() != details {
            self.details.set_text(details);
        }
        self.button.set_tooltip_text(Some(details));
        self.button
            .update_property(&[gtk::accessible::Property::Label(text)]);
    }
}

/// A ring that fills with the selected session's context, as in Claude's apps.
#[derive(Clone)]
pub(super) struct ContextMeter {
    root: gtk::Box,
    label: gtk::Label,
    ring: gtk::DrawingArea,
    used: Rc<Cell<f64>>,
}

impl ContextMeter {
    fn new() -> Self {
        let used = Rc::new(Cell::new(0.0));
        let ring = gtk::DrawingArea::builder()
            .content_width(14)
            .content_height(14)
            .valign(gtk::Align::Center)
            .build();
        let percent = used.clone();
        ring.set_draw_func(move |ring, cr, width, height| {
            let color = ring.color();
            let paint = |alpha: f32| {
                cr.set_source_rgba(
                    color.red().into(),
                    color.green().into(),
                    color.blue().into(),
                    (color.alpha() * alpha).into(),
                );
            };
            let line = 2.0;
            let (x, y) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
            let radius = x.min(y) - line / 2.0;
            cr.set_line_width(line);
            paint(0.25);
            cr.arc(x, y, radius, 0.0, TAU);
            let _ = cr.stroke();
            if percent.get() > 0.0 {
                paint(1.0);
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                cr.arc(
                    x,
                    y,
                    radius,
                    -FRAC_PI_2,
                    -FRAC_PI_2 + TAU * percent.get() / 100.0,
                );
                let _ = cr.stroke();
            }
        });
        let label = gtk::Label::new(None);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        root.add_css_class("context-meter");
        root.set_margin_start(8);
        root.append(&ring);
        root.append(&label);
        root.set_visible(false);
        Self {
            root,
            label,
            ring,
            used,
        }
    }

    fn set(&self, used: Option<f64>) {
        self.root.set_visible(used.is_some());
        let Some(used) = used.map(|used| used.clamp(0.0, 100.0)) else {
            return;
        };
        if self.used.replace(used) != used {
            self.ring.queue_draw();
        }
        self.label.set_text(&format!("{used:.0}%"));
        if used >= CONTEXT_WARNING {
            self.root.add_css_class("warning");
        } else {
            self.root.remove_css_class("warning");
        }
    }
}

#[derive(Default)]
struct Collector {
    previous_cpu: Option<CpuSample>,
    conversation: Option<(AgentProvider, String)>,
    reader: Option<UsageReader>,
    locate_at: Option<Instant>,
}

#[derive(Clone)]
pub(super) struct StatusBar {
    pub root: gtk::Box,
    pub account: Segment,
    pub session: Segment,
    context: ContextMeter,
    pub system: Segment,
    collector: Rc<RefCell<Option<Collector>>>,
    io: IoWorker,
    quota_io: IoWorker,
    quota_in_flight: Rc<Cell<bool>>,
    quotas: Rc<RefCell<Vec<AccountQuota>>>,
    quota_checked: Rc<Cell<Option<Instant>>>,
    displayed_session: Rc<RefCell<UsageSelection>>,
}

impl StatusBar {
    pub fn new(terminal: &gtk::Stack) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_widget_name("status-bar");
        root.add_css_class("usage-status-bar");
        let layout = super::status_bar_layout::StatusBarLayout::new();
        root.set_layout_manager(Some(layout.clone()));
        let account = Segment::new(
            "account-usage",
            "Account quota …",
            "avatar-default-symbolic",
        );
        let session = Segment::new("session-usage", "No session", "utilities-terminal-symbolic");
        let context = ContextMeter::new();
        session.content.append(&context.root);
        let system = Segment::new("system-usage", "CPU — · RAM —", "computer-symbolic");
        root.append(&account.button);
        root.append(&session.button);
        root.append(&system.button);
        let terminal = terminal.downgrade();
        root.add_tick_callback(move |root, _| {
            if let Some(terminal) = terminal.upgrade()
                && let Some(bounds) = terminal.compute_bounds(root)
            {
                layout.set_center((bounds.x() + bounds.width() / 2.0).round() as i32);
            }
            glib::ControlFlow::Continue
        });
        Self {
            root,
            account,
            session,
            context,
            system,
            collector: Rc::new(RefCell::new(Some(Collector::default()))),
            io: IoWorker::default(),
            quota_io: IoWorker::default(),
            quota_in_flight: Rc::new(Cell::new(false)),
            quotas: Rc::new(RefCell::new(Vec::new())),
            quota_checked: Rc::new(Cell::new(None)),
            displayed_session: Rc::new(RefCell::new(None)),
        }
    }

    fn set_session(&self, text: &str, details: &str, context_used: Option<f64>) {
        self.session.set(text, details);
        self.context.set(context_used);
        if let Some(used) = context_used {
            self.session
                .button
                .update_property(&[gtk::accessible::Property::Label(&format!(
                    "{text}, {used:.0}% of context used"
                ))]);
        }
    }
}

impl Workspace {
    pub(super) fn install_usage_timer(&self) {
        self.refresh_usage();
        self.refresh_account_quota();
        let workspace = self.clone();
        glib::timeout_add_local(Duration::from_secs(2), move || {
            workspace.refresh_usage();
            workspace.render_account_quota();
            if workspace
                .status_bar
                .quota_checked
                .get()
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(60))
            {
                workspace.refresh_account_quota();
            }
            glib::ControlFlow::Continue
        });
    }

    pub(super) fn refresh_usage(&self) {
        let record = self.selected_session_id().and_then(|id| {
            self.sessions
                .borrow()
                .get(&id)
                .map(|session| session.record.clone())
        });
        let key = record
            .as_ref()
            .map(|record| (record.id.clone(), record.conversation_id.clone()));
        if *self.status_bar.displayed_session.borrow() != key {
            *self.status_bar.displayed_session.borrow_mut() = key.clone();
            self.status_bar.set_session(
                if key.is_some() {
                    "Session · loading…"
                } else {
                    "No session"
                },
                "Usage belongs to the selected agent session.",
                None,
            );
        }
        let Some(mut collector) = self.status_bar.collector.borrow_mut().take() else {
            return;
        };
        let discovery = self.provider_discovery();
        let receiver = self.status_bar.io.submit(move || {
            let cpu_sample = fs::read_to_string("/proc/stat")
                .ok()
                .and_then(|value| CpuSample::parse(&value));
            let cpu = cpu_sample.and_then(|sample| {
                collector
                    .previous_cpu
                    .and_then(|previous| sample.percent_since(previous))
            });
            collector.previous_cpu = cpu_sample;
            let memory = fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|value| Memory::parse(&value));
            let conversation = record.as_ref().and_then(|record| {
                Some((
                    AgentProvider::from_kind(record.kind)?,
                    record.conversation_id.clone()?,
                ))
            });
            if collector.conversation != conversation {
                collector.conversation = conversation.clone();
                collector.reader = None;
                collector.locate_at = None;
            }
            if collector.reader.is_none()
                && collector
                    .locate_at
                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(15))
                && let Some((provider, id)) = conversation
            {
                collector.locate_at = Some(Instant::now());
                collector.reader = discovery
                    .log_path(provider, &id)
                    .ok()
                    .flatten()
                    .map(|path| UsageReader::new(path, provider));
            }
            let mut usage = collector
                .reader
                .as_mut()
                .and_then(|reader| reader.read_new().ok());
            if usage.is_none() {
                collector.reader = None;
            }
            if let Some(usage) = usage.as_mut()
                && usage.context_capacity.is_none()
                && let Some(record) = &record
                && let Some(report) = fs::read(context_report_path(&record.socket_path))
                    .ok()
                    .and_then(|report| ContextReport::parse(&report))
            {
                usage.context_capacity = report.window;
                usage.compact_window = report.model.and_then(|model| {
                    auto_compact_window(
                        &DiscoveryRoots::from_environment().claude_config_dir,
                        report.project_dir.as_deref(),
                        &model,
                    )
                });
            }
            (collector, key, record, usage, cpu, memory)
        });
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let Ok((collector, key, record, usage, cpu, memory)) = receiver.await else {
                return;
            };
            *workspace.status_bar.collector.borrow_mut() = Some(collector);
            workspace.render_system_usage(cpu, memory);
            let current = workspace.selected_session_id().and_then(|id| {
                workspace
                    .sessions
                    .borrow()
                    .get(&id)
                    .map(|session| (id, session.record.conversation_id.clone()))
            });
            if current == key {
                workspace.render_session_usage(record.as_ref(), usage.as_ref());
            }
            if current != key || usage.as_ref().is_some_and(|usage| usage.catching_up) {
                workspace.refresh_usage();
            }
        });
    }

    fn render_session_usage(&self, record: Option<&SessionRecord>, usage: Option<&SessionUsage>) {
        let status_bar = &self.status_bar;
        let Some(record) = record else {
            status_bar.set_session(
                "No session",
                "Select an agent session to see its token usage.",
                None,
            );
            return;
        };
        if !matches!(record.kind, SessionKind::Codex | SessionKind::Claude) {
            status_bar.set_session(
                "Session · —",
                "Token usage is available for Claude and Codex sessions.",
                None,
            );
            return;
        }
        let Some(usage) = usage.filter(|usage| usage.available && !usage.catching_up) else {
            let text = if usage.is_some_and(|usage| usage.catching_up) {
                "Session · reading log…"
            } else {
                "Session · waiting for usage"
            };
            status_bar.set_session(
                text,
                "Usage appears after the agent writes token counts to its local conversation log.",
                None,
            );
            return;
        };
        status_bar.set_session(
            &format!(
                "Session {}{} tokens",
                if usage.incomplete { "≥ " } else { "" },
                compact(usage.total())
            ),
            &session_details(&record.name, usage),
            usage.context_fill(),
        );
    }

    fn render_system_usage(&self, cpu: Option<f64>, memory: Option<Memory>) {
        let cpu = cpu
            .map(|percent| format!("{percent:.0}%"))
            .unwrap_or_else(|| "—".to_owned());
        let ram = memory
            .map(|memory| format!("{:.1} GiB", memory.used as f64 / 1073741824.0))
            .unwrap_or_else(|| "—".to_owned());
        let details = format!(
            "Whole-machine load\n\nCPU: {cpu} across all cores\n{}\n\nRAM excludes available and reclaimable memory.\nUpdated every 2 seconds.",
            memory
                .map(|m| format!(
                    "RAM: {:.1} / {:.1} GiB ({:.0}% used)",
                    m.used as f64 / 1073741824.0,
                    m.total as f64 / 1073741824.0,
                    100.0 * m.used as f64 / m.total as f64
                ))
                .unwrap_or_else(|| "RAM unavailable".to_owned())
        );
        self.status_bar
            .system
            .set(&format!("CPU {cpu} · RAM {ram}"), &details);
    }

    fn refresh_account_quota(&self) {
        if self.status_bar.quota_in_flight.replace(true) {
            return;
        }
        let roots = DiscoveryRoots::from_environment();
        let receiver = self.status_bar.quota_io.submit(move || {
            [AgentProvider::Claude, AgentProvider::Codex]
                .into_iter()
                .map(|provider| quota::fetch(provider, &roots))
                .collect::<Vec<_>>()
        });
        let workspace = self.clone();
        glib::spawn_future_local(async move {
            let result = receiver.await;
            workspace.status_bar.quota_in_flight.set(false);
            workspace.status_bar.quota_checked.set(Some(Instant::now()));
            if let Ok(quotas) = result {
                *workspace.status_bar.quotas.borrow_mut() = quotas;
            }
            workspace.render_account_quota();
        });
    }

    fn render_account_quota(&self) {
        let now = crate::persist::now_millis() / 1000;
        let quotas = self.status_bar.quotas.borrow();
        if quotas.is_empty() {
            return;
        }
        let mut chips = Vec::new();
        let mut details = vec!["Account quota remaining".to_owned()];
        let mut warning = false;
        for quota in quotas.iter() {
            let name = match quota.provider {
                AgentProvider::Claude => "Claude",
                AgentProvider::Codex => "Codex",
            };
            if quota.windows.is_empty() {
                details.push(format!("{name}: {}", quota.status));
                continue;
            }
            let mut windows = Vec::new();
            for window in &quota.windows {
                let expired = window.resets_at.is_some_and(|reset| reset <= now);
                let remaining = if expired {
                    "—".to_owned()
                } else {
                    format!("{:.0}%", window.remaining)
                };
                windows.push(format!("{} {remaining}", window.label));
                let reset = window
                    .resets_at
                    .map(|reset| {
                        if reset <= now {
                            "awaiting refresh".to_owned()
                        } else {
                            format!("resets in {}", countdown(reset - now))
                        }
                    })
                    .unwrap_or_else(|| "reset time unavailable".to_owned());
                details.push(format!(
                    "{name} {}: {remaining} left · {reset}",
                    window.label
                ));
                warning |= !expired && window.remaining <= 20.0;
            }
            chips.push(format!("{name} {}", windows.join(" · ")));
        }
        details.push("Uses the current local sign-in for each provider.\nRefreshes every minute; these limits apply across sessions.".to_owned());
        let text = if chips.is_empty() {
            "Account quota unavailable".to_owned()
        } else {
            format!("Quota left · {}", chips.join("   "))
        };
        self.status_bar.account.set(&text, &details.join("\n\n"));
        if warning {
            self.status_bar.account.button.add_css_class("warning");
        } else {
            self.status_bar.account.button.remove_css_class("warning");
        }
    }
}

fn session_details(name: &str, usage: &SessionUsage) -> String {
    let context = match (
        usage.context_used,
        usage.context_capacity,
        usage.context_remaining(),
    ) {
        (Some(used), Some(capacity), Some(left)) => format!(
            "Current context: {} / {} tokens\n{:.1}% used · {left:.1}% left",
            compact(used),
            compact(capacity),
            100.0 - left,
        ),
        _ => "Context capacity is not reported for this session.".to_owned(),
    };
    let context = match (
        usage.compact_window,
        usage.context_limit(),
        usage.context_fill(),
    ) {
        (Some(_), Some(limit), Some(fill)) => format!(
            "{context}\nAuto-compacts at {} tokens · {fill:.1}% of the way",
            compact(limit)
        ),
        _ => context,
    };
    let mut details = format!(
        "{name}\n\nSession tokens: {}\nInput: {} ({:.1}% of total)\nCached input: {} ({:.1}% of input)\nOutput: {} ({:.1}% of total)\n\n{context}\nToken totals are separate from account quota.",
        compact(usage.total()),
        compact(usage.input),
        percentage(usage.input, usage.total()),
        compact(usage.cached),
        percentage(usage.cached, usage.input),
        compact(usage.output),
        percentage(usage.output, usage.total()),
    );
    if usage.incomplete {
        details.push_str("\n\nAn oversized log entry was skipped; token totals may be incomplete.");
    }
    details
}

fn percentage(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * part as f64 / total as f64
    }
}

fn compact(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.1}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn countdown(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, (minutes % 1440) / 60)
    } else if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a private display"]
    fn usage_popover_opens_without_selecting_all_details() {
        gtk::init().unwrap();
        let segment = Segment::new("usage-test", "Session", "utilities-terminal-symbolic");
        let details = session_details(
            "usage",
            &SessionUsage {
                input: 9_990_944,
                cached: 9_698_688,
                output: 50_069,
                context_used: Some(51_908),
                context_capacity: Some(258_400),
                available: true,
                ..Default::default()
            },
        );
        segment.set("Session 10.0M tokens", &details);
        let window = gtk::Window::builder()
            .default_width(500)
            .default_height(300)
            .child(&segment.button)
            .build();
        window.present();
        let context = glib::MainContext::default();
        let settle = || {
            let until = Instant::now() + Duration::from_millis(100);
            while Instant::now() < until {
                while context.pending() {
                    context.iteration(false);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        settle();
        for _ in 0..2 {
            segment.button.popup();
            settle();
            assert!(segment.button.popover().unwrap().is_visible());
            assert_eq!(segment.details.selection_bounds(), None);
            if let Ok(directory) = std::env::var("AGTK_TEST_CAPTURE_DIR") {
                let popover = segment.button.popover().unwrap();
                let paintable = gtk::WidgetPaintable::new(Some(&popover));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, popover.width() as f64, popover.height() as f64);
                let node = snapshot.to_node().expect("popover rendered");
                let renderer = gtk::gsk::CairoRenderer::new();
                renderer.realize_for_display(&popover.display()).unwrap();
                let texture = renderer.render_texture(&node, None);
                renderer.unrealize();
                texture
                    .save_to_png(std::path::Path::new(&directory).join("session-usage.png"))
                    .unwrap();
            }
            // Explicit selection remains available for copying.
            segment.details.select_region(0, 7);
            assert_eq!(segment.details.selection_bounds(), Some((0, 7)));
            segment.set("Session 10.0M tokens", &details);
            assert_eq!(segment.details.selection_bounds(), Some((0, 7)));
            segment.button.popdown();
            settle();
        }
        window.close();
    }
}
