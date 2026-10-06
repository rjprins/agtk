use super::*;
use glib::variant::ToVariant;
use std::time::{Duration, Instant};

fn pump(milliseconds: u64) {
    let deadline = Instant::now() + Duration::from_millis(milliseconds);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Run with scripts/test-modals.sh on a private 1000x700 Mutter display and bus.
#[test]
#[ignore = "requires a private Mutter display and AGTK_TEST_BUS session bus"]
fn real_pointer_input_dismisses_only_the_top_modal_without_clicking_through() {
    let display = std::env::var("AGTK_TEST_DISPLAY").expect("private display required");
    let test_bus = std::env::var("AGTK_TEST_BUS").expect("private session bus required");
    assert!(display.contains("agtk-modal-test-"));
    assert_eq!(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(), test_bus);
    // SAFETY: run this GTK test alone with --test-threads=1.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", display);
        std::env::set_var("GDK_BACKEND", "wayland");
    }
    adw::init().unwrap();
    let app = adw::Application::builder()
        .application_id("nl.rutger.Agtk.ModalTest")
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let bus = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
    let destination = "org.gnome.Mutter.RemoteDesktop";
    let session = bus
        .call_sync(
            Some(destination),
            "/org/gnome/Mutter/RemoteDesktop",
            destination,
            "CreateSession",
            None,
            None,
            gio::DBusCallFlags::NONE,
            5000,
            None::<&gio::Cancellable>,
        )
        .unwrap();
    let session_path = session.child_value(0).str().unwrap().to_owned();
    let call = |method: &str, parameters: Option<glib::Variant>| {
        bus.call_sync(
            Some(destination),
            &session_path,
            "org.gnome.Mutter.RemoteDesktop.Session",
            method,
            parameters.as_ref(),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            None::<&gio::Cancellable>,
        )
        .unwrap();
    };
    call("Start", None);
    // Create the virtual keyboard before presenting windows so they get focus.
    call(
        "NotifyKeyboardKeysym",
        Some((0xffe1_u32, true).to_variant()),
    );
    pump(100);
    call(
        "NotifyKeyboardKeysym",
        Some((0xffe1_u32, false).to_variant()),
    );
    pump(100);
    let click = |x: f64, y: f64| {
        call(
            "NotifyPointerMotionRelative",
            Some((-10000.0_f64, -10000.0_f64).to_variant()),
        );
        pump(50);
        call("NotifyPointerMotionRelative", Some((x, y).to_variant()));
        pump(50);
        call("NotifyPointerButton", Some((272_i32, true).to_variant()));
        pump(50);
        call("NotifyPointerButton", Some((272_i32, false).to_variant()));
        pump(150);
    };
    let background_clicks = Rc::new(Cell::new(0));
    let background = gtk::Button::with_label("Workspace");
    let count = background_clicks.clone();
    background.connect_clicked(move |_| count.set(count.get() + 1));
    let parent = adw::ApplicationWindow::builder()
        .application(&app)
        .content(&background)
        .build();
    parent.fullscreen();
    let background_events = Rc::new(Cell::new(0));
    let events = gtk::EventControllerLegacy::new();
    events.set_propagation_phase(gtk::PropagationPhase::Capture);
    let count = background_events.clone();
    events.connect_event(move |_, event| {
        if matches!(
            event.event_type(),
            gtk::gdk::EventType::ButtonPress | gtk::gdk::EventType::ButtonRelease
        ) {
            count.set(count.get() + 1);
        }
        glib::Propagation::Proceed
    });
    parent.add_controller(events);
    parent.present();
    pump(300);

    let inside_clicks = Rc::new(Cell::new(0));
    let inside = gtk::Button::with_label("Inside");
    let count = inside_clicks.clone();
    inside.connect_clicked(move |_| count.set(count.get() + 1));
    let modal = Modal::new(&parent, "Modal", 360, 240, &inside);
    modal.present();
    pump(300);
    click(500.0, 350.0);
    assert_eq!(inside_clicks.get(), 1, "inside click must reach its button");
    assert!(modal.is_visible(), "inside click must keep the modal open");

    // Popovers use a separate GDK surface but still belong to this dialog.
    let pointer = gtk::prelude::WidgetExt::display(&parent)
        .default_seat()
        .unwrap()
        .pointer()
        .unwrap();
    let (x, y, _) = modal
        .window
        .surface()
        .unwrap()
        .device_position(&pointer)
        .unwrap();
    let origin = (500.0 - x, 350.0 - y);
    let popup_clicks = Rc::new(Cell::new(0));
    let popup_button = gtk::Button::with_label("Popover action");
    let count = popup_clicks.clone();
    popup_button.connect_clicked(move |_| count.set(count.get() + 1));
    let popover = gtk::Popover::builder().child(&popup_button).build();
    popover.set_parent(&inside);
    popover.popup();
    pump(200);
    let popup = popover
        .surface()
        .unwrap()
        .downcast::<gtk::gdk::Popup>()
        .unwrap();
    click(
        origin.0 + f64::from(popup.position_x()) + f64::from(popup.width()) / 2.0,
        origin.1 + f64::from(popup.position_y()) + f64::from(popup.height()) / 2.0,
    );
    assert_eq!(popup_clicks.get(), 1, "popover click must reach its button");
    assert!(modal.is_visible(), "popover click must keep the modal open");
    popover.popdown();
    pump(200);
    popover.unparent();

    click(50.0, 350.0);
    assert!(!modal.is_visible(), "outside click must close the modal");
    assert_eq!(
        background_events.get(),
        0,
        "neither half of the click may reach the workspace"
    );
    assert_eq!(
        background_clicks.get(),
        0,
        "dismissal must not click through"
    );
    click(50.0, 350.0);
    assert_eq!(background_clicks.get(), 1, "workspace must be usable again");

    modal.present();
    pump(300);
    let top = Modal::new(
        &parent,
        "Top modal",
        300,
        200,
        &gtk::Label::new(Some("Top")),
    );
    top.present();
    pump(300);
    click(50.0, 350.0);
    assert!(!top.is_visible(), "outside click must close the top modal");
    assert!(modal.is_visible(), "one click must only close one modal");
    assert_eq!(background_clicks.get(), 1);
    click(50.0, 350.0);
    assert!(!modal.is_visible(), "reopened modal must still dismiss");
    assert_eq!(background_clicks.get(), 1);

    modal.present();
    pump(300);
    click(500.0, 350.0);
    call(
        "NotifyKeyboardKeysym",
        Some((0xff1b_u32, true).to_variant()),
    );
    call(
        "NotifyKeyboardKeysym",
        Some((0xff1b_u32, false).to_variant()),
    );
    pump(150);
    assert!(!modal.is_visible(), "Escape must still close the modal");

    call("Stop", None);
    top.window.destroy();
    modal.window.destroy();
    parent.destroy();
}
