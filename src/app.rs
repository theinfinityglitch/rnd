use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk4::gdk_pixbuf::{Colorspace, InterpType, Pixbuf, PixbufLoader};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GBox, Button, GestureClick, Image, Label, Orientation,
    Widget,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tokio::sync::mpsc::UnboundedSender;
use zbus::zvariant::{OwnedValue, Structure};

use crate::config::{Anchor, Config};
use crate::dbus::{DaemonEvent, DaemonSignal};
use crate::history::History;
use crate::notification::{CloseReason, Notification};

// ── Active notification tracking ──────────────────────────────────────────────

struct ActiveNotification {
    notif: Notification,
    widget: Widget,
    timeout_source: Option<glib::SourceId>,
}

// ── NotificationManager ───────────────────────────────────────────────────────

pub struct NotificationManager {
    config: Arc<Config>,
    window: ApplicationWindow,
    vbox: GBox,
    active: Vec<ActiveNotification>,
    history: Arc<Mutex<History>>,
    signal_tx: UnboundedSender<DaemonSignal>,
}

impl NotificationManager {
    pub fn new(
        app: &Application,
        config: Arc<Config>,
        signal_tx: UnboundedSender<DaemonSignal>,
        history: Arc<Mutex<History>>,
    ) -> Rc<RefCell<Self>> {
        let (window, vbox) = build_popup_window(app, &config);

        Rc::new(RefCell::new(Self {
            config,
            window,
            vbox,
            active: Vec::new(),
            history,
            signal_tx,
        }))
    }

    // ── Event dispatch ────────────────────────────────────────────────────────

    pub fn handle_event(mgr: &Rc<RefCell<Self>>, event: DaemonEvent) {
        match event {
            DaemonEvent::Show(notif) => Self::show(mgr, notif),
            DaemonEvent::Close(id) => {
                mgr.borrow_mut()
                    .close(id, CloseReason::CloseNotificationCalled);
            }
            DaemonEvent::CloseAll => {
                mgr.borrow_mut().close_all();
            }
            DaemonEvent::InvokeAction { id, action_key } => {
                mgr.borrow_mut().invoke_action(id, action_key)
            }
            DaemonEvent::ClearHistory => mgr.borrow_mut().history.lock().unwrap().clear(),
            DaemonEvent::HistoryRemove { id } => {
                mgr.borrow_mut().history.lock().unwrap().remove(id)
            }
        }
    }

    // ── Show ──────────────────────────────────────────────────────────────────

    fn show(mgr: &Rc<RefCell<Self>>, notif: Notification) {
        {
            let mut m = mgr.borrow_mut();

            // Replace: remove the old card first.
            if notif.replaces_id != 0 {
                m.remove_widget(notif.replaces_id);
            }

            // Enforce max_visible: evict the oldest notification.
            let max = m.config.general.max_visible;
            while m.active.len() >= max {
                let oldest_id = m.active.last().map(|a| a.notif.id);
                if let Some(id) = oldest_id {
                    m.close(id, CloseReason::Expired);
                } else {
                    break;
                }
            }

            m.history.lock().unwrap().push(&notif);

            let timeout_ms = notif.effective_timeout_ms(&m.config.timeouts);
            let widget = build_card(&notif, &m.config, mgr);

            // Prepend so the newest notification appears at the top.
            m.vbox.prepend(&widget);

            let mut entry = ActiveNotification {
                notif,
                widget,
                timeout_source: None,
            };

            if let Some(ms) = timeout_ms {
                let id = entry.notif.id;
                let mgr_weak = Rc::downgrade(mgr);
                let src =
                    glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
                        if let Some(m) = mgr_weak.upgrade() {
                            m.borrow_mut().close(id, CloseReason::Expired);
                        }
                    });
                entry.timeout_source = Some(src);
            }

            m.active.insert(0, entry);
            m.sync_visibility();
        }
    }

    // ── Close ─────────────────────────────────────────────────────────────────

    pub fn close(&mut self, id: u32, reason: CloseReason) {
        self.remove_widget(id);
        let _ = self.signal_tx.send(DaemonSignal::NotificationClosed {
            id,
            reason: reason as u32,
        });
        self.sync_visibility();
    }

    pub fn close_all(&mut self) {
        let ids: Vec<u32> = self.active.iter().map(|a| a.notif.id).collect();
        for id in ids {
            self.close(id, CloseReason::CloseNotificationCalled);
        }
    }

    fn remove_widget(&mut self, id: u32) {
        if let Some(pos) = self.active.iter().position(|a| a.notif.id == id) {
            let mut entry = self.active.remove(pos);
            if let Some(src) = entry.timeout_source.take() {
                src.remove();
            }
            self.vbox.remove(&entry.widget);
        }
    }

    fn sync_visibility(&self) {
        if self.active.is_empty() {
            self.window.set_visible(false);
        } else {
            self.window.set_visible(true);
            self.window.present();
        }
    }

    // ── Action invoked ────────────────────────────────────────────────────────

    pub fn invoke_action(&mut self, id: u32, action_key: String) {
        let _ = self
            .signal_tx
            .send(DaemonSignal::ActionInvoked { id, action_key });
        self.close(id, CloseReason::DismissedByUser);
    }
}

// ── Layer-shell window ────────────────────────────────────────────────────────

fn build_popup_window(app: &Application, config: &Config) -> (ApplicationWindow, GBox) {
    let window = ApplicationWindow::builder()
        .application(app)
        .name("rnd-popup")
        .decorated(false)
        .resizable(false)
        .build();

    // Layer-shell MUST be initialised before the window is realised/shown.
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);

    apply_anchor(&window, config.general.anchor, config.general.margin);

    let vbox = GBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(config.general.gap)
        .css_classes(vec!["notification-list"])
        .build();

    window.set_child(Some(&vbox));
    window.set_visible(false);

    (window, vbox)
}

fn apply_anchor(window: &ApplicationWindow, anchor: Anchor, margin: i32) {
    // Clear all edges.
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, false);
        window.set_margin(edge, 0);
    }

    match anchor {
        Anchor::TopRight => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, margin);
            window.set_margin(Edge::Right, margin);
        }
        Anchor::TopLeft => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Top, margin);
            window.set_margin(Edge::Left, margin);
        }
        Anchor::TopCenter => {
            window.set_anchor(Edge::Top, true);
            window.set_margin(Edge::Top, margin);
        }
        Anchor::BottomRight => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Bottom, margin);
            window.set_margin(Edge::Right, margin);
        }
        Anchor::BottomLeft => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Bottom, margin);
            window.set_margin(Edge::Left, margin);
        }
        Anchor::BottomCenter => {
            window.set_anchor(Edge::Bottom, true);
            window.set_margin(Edge::Bottom, margin);
        }
    }
}

// ── Notification card ─────────────────────────────────────────────────────────

fn build_card(
    notif: &Notification,
    config: &Arc<Config>,
    mgr: &Rc<RefCell<NotificationManager>>,
) -> Widget {
    let card = GBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .css_classes(vec!["notification", notif.urgency.css_class()])
        .width_request(config.appearance.min_width)
        .build();

    // App icon / image hints
    if !notif.app_icon.is_empty()
        || notif.hints.contains_key("image-path")
        || icon_from_hints(&notif.hints, config).is_some()
    {
        card.append(&build_icon(&notif.app_icon, &notif.hints, &config));
    }

    // Text column
    let text_col = GBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .css_classes(vec!["notification-text"])
        .build();

    if config.appearance.show_app_name
        && !notif.app_name.is_empty()
        && notif.app_name != "notify-send"
    {
        let app_label = Label::builder()
            .label(&notif.app_name)
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .css_classes(vec!["notification-app-name"])
            .build();
        text_col.append(&app_label);
    }

    if !notif.summary.is_empty() {
        let summary = Label::builder()
            .label(&notif.summary)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .css_classes(vec!["notification-summary"])
            .build();
        summary.set_max_width_chars(48);
        text_col.append(&summary);
    }

    if !notif.body.is_empty() {
        let body = Label::builder()
            .label(&notif.body)
            .xalign(0.0)
            .use_markup(true)
            .wrap(true)
            .wrap_mode(gtk4::pango::WrapMode::WordChar)
            .css_classes(vec!["notification-body"])
            .build();
        body.set_max_width_chars(48);
        text_col.append(&body);
    }

    // Action buttons
    if config.appearance.show_actions && !notif.actions.is_empty() {
        let action_row = GBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(4)
            .css_classes(vec!["notification-actions"])
            .build();

        for (key, label) in &notif.actions {
            if key == "default" {
                continue; // default action is triggered by clicking the card
            }
            let btn = Button::builder()
                .label(label)
                .css_classes(vec!["notification-action"])
                .build();

            let mgr_weak = Rc::downgrade(mgr);
            let id = notif.id;
            let key = key.clone();
            btn.connect_clicked(move |_| {
                if let Some(m) = mgr_weak.upgrade() {
                    m.borrow_mut().invoke_action(id, key.clone());
                }
            });
            action_row.append(&btn);
        }
        text_col.append(&action_row);
    }

    card.append(&text_col);

    // Close button
    let close_btn = Button::builder()
        .icon_name("window-close-symbolic")
        .css_classes(vec!["notification-close"])
        .valign(gtk4::Align::Start)
        .build();

    {
        let mgr_weak = Rc::downgrade(mgr);
        let id = notif.id;
        close_btn.connect_clicked(move |_| {
            if let Some(m) = mgr_weak.upgrade() {
                m.borrow_mut().close(id, CloseReason::DismissedByUser);
            }
        });
    }
    card.append(&close_btn);

    // Click-to-dismiss / default action on the card body
    let gesture = GestureClick::new();
    {
        let mgr_weak = Rc::downgrade(mgr);
        let has_default = notif.actions.iter().any(|(k, _)| k == "default");
        let id = notif.id;
        gesture.connect_released(move |_, _, _, _| {
            if let Some(m) = mgr_weak.upgrade() {
                if has_default {
                    m.borrow_mut().invoke_action(id, "default".to_string());
                } else {
                    m.borrow_mut().close(id, CloseReason::DismissedByUser);
                }
            }
        });
    }
    card.add_controller(gesture);

    card.upcast()
}

// ── Icon helper ───────────────────────────────────────────────────────────────

fn build_icon(app_icon: &str, hints: &HashMap<String, OwnedValue>, config: &Config) -> Widget {
    // helper to wrap an Image in a container that enforces rounding/clipping
    fn wrap_image(img: Image) -> Widget {
        use gtk4::Align;
        // prevent Image from expanding; keep it centered within the container
        img.set_halign(Align::Center);
        img.set_valign(Align::Center);
        img.set_width_request(64);
        img.set_height_request(64);

        let container = GBox::builder()
            .orientation(Orientation::Horizontal)
            .spacing(0)
            .css_classes(vec!["notification-icon-container"])
            .build();
        container.append(&img);
        container.upcast::<Widget>()
    }

    if let Some(img) = icon_from_hints(hints, config) {
        return wrap_image(img);
    }

    if let Some(img) = icon_from_path(app_icon) {
        return wrap_image(img);
    }

    const ICON_SIZE: i32 = 64;
    let img = Image::builder()
        .icon_name(app_icon)
        .pixel_size(ICON_SIZE)
        .css_classes(vec!["notification-icon"])
        .build();
    wrap_image(img)
}

fn icon_from_hints(hints: &HashMap<String, OwnedValue>, config: &Config) -> Option<Image> {
    if let Some(value) = hints.get("image-path") {
        if let Some(img) = icon_from_hint_value("image-path", value, config) {
            return Some(img);
        }
    }

    if let Some(value) = hints.get("image-data") {
        if let Some(img) = icon_from_hint_value("image-data", value, config) {
            return Some(img);
        }
    }

    for (key, value) in hints {
        if !hint_key_might_be_icon(key) {
            continue;
        }
        if let Some(img) = icon_from_hint_value(key, value, config) {
            return Some(img);
        }
    }

    None
}

fn hint_key_might_be_icon(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("image") || key.contains("icon") || key.contains("app_icon")
}

fn icon_from_hint_value(key: &str, value: &OwnedValue, config: &Config) -> Option<Image> {
    if let Ok(owned_value) = value.try_clone() {
        if let Ok(path) = String::try_from(owned_value) {
            if let Some(img) = icon_from_path(&path) {
                return Some(img);
            }
            if !path.is_empty() {
                let img = Image::builder()
                    .icon_name(&path)
                    .pixel_size(64)
                    .css_classes(vec!["notification-icon"])
                    .build();
                return Some(img);
            }
        }
    }

    if let Ok(owned_value) = value.try_clone() {
        if let Ok(bytes) = Vec::<u8>::try_from(owned_value) {
            if let Some(img) = image_from_raw_bytes(&bytes, config) {
                return Some(img);
            }
        }
    }

    if let Ok(owned_value) = value.try_clone() {
        if let Ok(structure) = Structure::try_from(owned_value) {
            // tracing::debug!(
            //     hint=%key,
            //     signature=%structure.full_signature().as_str(),
            //     fields=%structure.fields().len(),
            //     "image-data structure received"
            // );
            if let Some(img) = image_from_image_data_structure(structure, config) {
                return Some(img);
            }
            tracing::debug!(hint=%key, "image-data structure parse failed");
        }
    }

    tracing::debug!(hint=%key, "hint value could not be converted to an image");
    None
}

fn image_from_image_data_structure(structure: Structure, _config: &Config) -> Option<Image> {
    if let Ok((width, height, rowstride, has_alpha, bits, channels, data)) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(structure)
    {
        // tracing::debug!(
        //     width = width,
        //     height = height,
        //     rowstride = rowstride,
        //     has_alpha = has_alpha,
        //     bits = bits,
        //     channels = channels,
        //     byte_count = data.len(),
        //     "decoded image-data structure"
        // );

        if width <= 0 || height <= 0 || bits != 8 {
            tracing::debug!("unsupported image-data dimensions or bits");
            return None;
        }

        let channels = channels.max(1).min(4);
        let min_stride = width * channels;
        let rowstride = if rowstride <= 0 || rowstride < min_stride {
            min_stride
        } else {
            rowstride
        };

        let bytes = glib::Bytes::from(&data[..]);
        let pixbuf = Pixbuf::from_bytes(
            &bytes,
            Colorspace::Rgb,
            has_alpha,
            bits,
            width,
            height,
            rowstride,
        );

        if pixbuf.width() == 0 || pixbuf.height() == 0 {
            tracing::debug!("image-data pixbuf created with zero size");
            return None;
        }

        let max_icon_size = 64i32;
        let scale_factor = (width.max(height) as f64) / (max_icon_size as f64);
        let scale_factor = scale_factor.max(1.0); // Only scale down if larger
        let desired_w = (width as f64 / scale_factor).round() as i32;
        let desired_h = (height as f64 / scale_factor).round() as i32;
        let img_pix =
            if let Some(pb) = pixbuf.scale_simple(desired_w, desired_h, InterpType::Bilinear) {
                pb
            } else {
                pixbuf
            };

        tracing::debug!(
            final_width = img_pix.width(),
            final_height = img_pix.height(),
            channels = img_pix.n_channels(),
            "image-data pixbuf ready"
        );

        let img = Image::from_pixbuf(Some(&img_pix));
        img.set_pixel_size(img_pix.width());
        img.add_css_class("notification-icon");
        return Some(img);
    }

    tracing::debug!("image-data structure did not match expected tuple");
    None
}

fn image_from_raw_bytes(bytes: &[u8], _config: &Config) -> Option<Image> {
    let loader = PixbufLoader::new();
    if loader.write_bytes(&glib::Bytes::from(bytes)).is_ok() && loader.close().is_ok() {
        if let Some(pb) = loader.pixbuf() {
            let w = pb.width() as i32;
            let h = pb.height() as i32;
            let max_icon_size = 64i32;
            let scale_factor = (w.max(h) as f64) / (max_icon_size as f64);
            let scale_factor = scale_factor.max(1.0); // Only scale down if larger
            let desired_w = (w as f64 / scale_factor).round() as i32;
            let desired_h = (h as f64 / scale_factor).round() as i32;
            let final_pb =
                if let Some(s) = pb.scale_simple(desired_w, desired_h, InterpType::Bilinear) {
                    s
                } else {
                    pb
                };
            let img = Image::from_pixbuf(Some(&final_pb));
            img.set_pixel_size(final_pb.width());
            img.add_css_class("notification-icon");
            return Some(img);
        }
    }

    None
}

fn icon_from_path(name_or_path: &str) -> Option<Image> {
    const ICON_FALLBACK: i32 = 64;

    if name_or_path.is_empty() {
        return None;
    }

    let maybe_path = if let Some(stripped) = name_or_path.strip_prefix("file://") {
        stripped.strip_prefix("localhost/").unwrap_or(stripped)
    } else {
        name_or_path
    };

    // Local file path: load full image and scale proportionally but cap to ICON_FALLBACK
    if std::path::Path::new(maybe_path).exists() {
        if let Ok(pb_full) = Pixbuf::from_file(maybe_path) {
            let (w, h) = (pb_full.width() as i32, pb_full.height() as i32);
            let max_icon_size = ICON_FALLBACK;
            let scale_factor = (w.max(h) as f64) / (max_icon_size as f64);
            let scale_factor = scale_factor.max(1.0); // only scale down
            let desired_w = (w as f64 / scale_factor).round() as i32;
            let desired_h = (h as f64 / scale_factor).round() as i32;
            if let Some(pb) = pb_full.scale_simple(desired_w, desired_h, InterpType::Bilinear) {
                tracing::debug!(
                    final_width = pb.width(),
                    final_height = pb.height(),
                    "icon_from_path pixbuf ready (scaled)"
                );
                let img = Image::from_pixbuf(Some(&pb));
                img.set_pixel_size(max_icon_size);
                img.add_css_class("notification-icon");
                return Some(img);
            }
            tracing::debug!(
                final_width = pb_full.width(),
                final_height = pb_full.height(),
                "icon_from_path pixbuf ready (full)"
            );
            let img = Image::from_pixbuf(Some(&pb_full));
            img.set_pixel_size(ICON_FALLBACK);
            img.add_css_class("notification-icon");
            return Some(img);
        }
    }

    // Remote HTTP(S) URL: fetch asynchronously and update image when ready
    if name_or_path.starts_with("http://") || name_or_path.starts_with("https://") {
        // placeholder image while fetching
        let img = Image::builder()
            .icon_name("image-x-generic")
            .pixel_size(ICON_FALLBACK)
            .css_classes(vec!["notification-icon"])
            .build();

        let url = name_or_path.to_string();
        let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
        let img_clone = img.clone();
        glib::idle_add_local(move || {
            use std::sync::mpsc::TryRecvError;
            match receiver.try_recv() {
                Ok(vec) => {
                    let loader = PixbufLoader::new();
                    let _ = loader.write_bytes(&glib::Bytes::from(&vec[..]));
                    let _ = loader.close();
                    if let Some(pb) = loader.pixbuf() {
                        let final_pb = if let Some(s) =
                            pb.scale_simple(ICON_FALLBACK, ICON_FALLBACK, InterpType::Bilinear)
                        {
                            s
                        } else {
                            pb
                        };
                        img_clone.set_from_pixbuf(Some(&final_pb));
                        img_clone.set_pixel_size(ICON_FALLBACK);
                        img_clone.add_css_class("notification-icon");
                    }
                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });

        std::thread::spawn(move || {
            if let Ok(resp) = reqwest::blocking::get(&url) {
                if let Ok(bytes) = resp.bytes() {
                    let vec = bytes.to_vec();
                    let _ = sender.send(vec);
                }
            }
        });

        return Some(img);
    }

    let img = Image::builder()
        .icon_name(name_or_path)
        .pixel_size(ICON_FALLBACK)
        .css_classes(vec!["notification-icon"])
        .build();
    Some(img)
}
