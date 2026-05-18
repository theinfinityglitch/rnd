use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::AdwApplicationWindowExt;
use adw::{Application, ApplicationWindow};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Box as GBox, Button, GestureClick, Label, Orientation, Widget};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{Anchor, Config};
use crate::dbus::{DaemonEvent, DaemonSignal};
use crate::history::{History, HistoryEntry};
use crate::icon::*;
use crate::notification::{CloseReason, Notification};

struct ActiveNotification {
    notif: Notification,
    widget: Widget,
    timeout_source: Option<glib::SourceId>,
}

pub struct NotificationManager {
    config: Arc<Config>,
    window: ApplicationWindow,
    vbox: GBox,
    status: Arc<Mutex<bool>>,
    active: Vec<ActiveNotification>,
    history: Arc<Mutex<History>>,
    signal_tx: UnboundedSender<DaemonSignal>,
}

impl NotificationManager {
    pub fn new(
        app: &Application,
        config: Arc<Config>,
        signal_tx: UnboundedSender<DaemonSignal>,
        status: Arc<Mutex<bool>>,
        history: Arc<Mutex<History>>,
    ) -> Rc<RefCell<Self>> {
        let (window, vbox) = build_popup_window(app, &config);

        Rc::new(RefCell::new(Self {
            config,
            window,
            vbox,
            status,
            active: Vec::new(),
            history,
            signal_tx,
        }))
    }

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
            _ => {}
        }
    }

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

            let mut history_entry: HistoryEntry = HistoryEntry::from(&notif);

            let timeout_ms = notif.effective_timeout_ms(&m.config.timeouts);
            let widget = build_card(&notif, &mut history_entry, &m.config, mgr);
            m.history.lock().unwrap().push(&history_entry);

            let status_bool = m.status.lock().unwrap().clone();

            if !status_bool {
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
                    let src = glib::timeout_add_local_once(
                        std::time::Duration::from_millis(ms),
                        move || {
                            if let Some(m) = mgr_weak.upgrade() {
                                m.borrow_mut().close(id, CloseReason::Expired);
                            }
                        },
                    );
                    entry.timeout_source = Some(src);
                }

                m.active.insert(0, entry);
                m.sync_visibility();
            }
        }
    }

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

    pub fn invoke_action(&mut self, id: u32, action_key: String) {
        let _ = self
            .signal_tx
            .send(DaemonSignal::ActionInvoked { id, action_key });
        self.close(id, CloseReason::DismissedByUser);
    }
}

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

    window.set_content(Some(&vbox));
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

fn build_card(
    notif: &Notification,
    history_entry: &mut HistoryEntry,
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
        || may_have_icon_from_hint(&notif.hints)
    {
        card.append(&build_icon(&notif, history_entry));
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
        .css_classes(vec!["notification-close", "circular"])
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
