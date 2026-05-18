use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_channel::Sender;
use tokio::sync::mpsc::UnboundedReceiver;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Structure};

use crate::history::History;
use crate::notification::Notification;

// ── Cross-thread messages ─────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DaemonEvent {
    Show(Notification),
    Close(u32),
    CloseAll,
    InvokeAction { id: u32, action_key: String },
    ClearHistory,
    HistoryRemove { id: Option<u32> },
}

#[derive(Debug)]
pub enum DaemonSignal {
    NotificationClosed { id: u32, reason: u32 },
    ActionInvoked { id: u32, action_key: String },
}

// ── D-Bus interface ───────────────────────────────────────────────────────────

struct NotificationServer {
    /// Sends events to the GTK main thread via an async_channel.
    tx: Sender<DaemonEvent>,
    next_id: Arc<AtomicU32>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationServer {
    async fn get_capabilities(&self) -> Vec<&str> {
        vec![
            "body",
            "body-markup",
            "actions",
            "icon-static",
            "persistence",
        ]
    }

    async fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("rnd", "rnd", env!("CARGO_PKG_VERSION"), "1.2")
    }

    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            let v = self.next_id.fetch_add(1, Ordering::Relaxed);
            if v == 0 {
                self.next_id.fetch_add(1, Ordering::Relaxed)
            } else {
                v
            }
        };

        tracing::info!(
            "notify received app_icon={:?}, hint_count={}",
            app_icon,
            hints.len()
        );
        let keys: Vec<&String> = hints.keys().collect();
        tracing::info!("notify hint keys={:?}", keys);
        tracing::info!("notify received raw hints={:?}", hints);
        for (k, v) in hints.iter() {
            tracing::info!(hint=%k, summary=%summarize_hint_value(v));
        }

        let notif = Notification::new(
            id,
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
        );

        self.tx.send(DaemonEvent::Show(notif)).await.ok();
        id
    }

    async fn close_notification(&self, id: u32) {
        self.tx.send(DaemonEvent::Close(id)).await.ok();
    }
}

struct ControlServer {
    tx: Sender<DaemonEvent>,
    history: Arc<Mutex<History>>,
}

#[interface(name = "org.rnd.Control")]
impl ControlServer {
    async fn get_history(&self) -> String {
        let history = self.history.lock().unwrap();
        let mut notifications = Vec::new();

        for entry in history.all_entries() {
            let urgency_str = match entry.urgency {
                crate::notification::Urgency::Low => "LOW",
                crate::notification::Urgency::Normal => "NORMAL",
                crate::notification::Urgency::Critical => "CRITICAL",
            };

            let timeout_us = if entry.expire_timeout > 0 {
                (entry.expire_timeout as i64) * 1000
            } else {
                entry.expire_timeout as i64
            };

            let mut notif = serde_json::Map::new();
            notif.insert(
                "body".to_string(),
                serde_json::json!({"type": "s", "data": entry.body}),
            );
            notif.insert(
                "message".to_string(),
                serde_json::json!({
                    "type": "s",
                    "data": format!("<b>{}</b>\n{}", entry.summary, entry.body)
                }),
            );
            notif.insert(
                "summary".to_string(),
                serde_json::json!({"type": "s", "data": entry.summary}),
            );
            notif.insert(
                "appname".to_string(),
                serde_json::json!({"type": "s", "data": entry.app_name}),
            );
            notif.insert(
                "category".to_string(),
                serde_json::json!({"type": "s", "data": ""}),
            );
            notif.insert(
                "default_action_name".to_string(),
                serde_json::json!({"type": "s", "data": "default"}),
            );
            notif.insert(
                "icon_path".to_string(),
                serde_json::json!({"type": "s", "data": entry.app_icon}),
            );
            notif.insert(
                "id".to_string(),
                serde_json::json!({"type": "i", "data": entry.id}),
            );
            notif.insert(
                "timestamp".to_string(),
                serde_json::json!({"type": "x", "data": (entry.timestamp as i64) * 1_000_000}),
            );
            notif.insert(
                "timeout".to_string(),
                serde_json::json!({"type": "x", "data": timeout_us}),
            );
            notif.insert(
                "progress".to_string(),
                serde_json::json!({"type": "i", "data": -1}),
            );
            notif.insert(
                "urgency".to_string(),
                serde_json::json!({"type": "s", "data": urgency_str}),
            );
            notif.insert(
                "stack_tag".to_string(),
                serde_json::json!({"type": "s", "data": ""}),
            );
            notif.insert(
                "urls".to_string(),
                serde_json::json!({"type": "s", "data": ""}),
            );

            notifications.push(serde_json::Value::Object(notif));
        }

        serde_json::to_string_pretty(&serde_json::json!({
            "type": "aa{sv}",
            "data": [notifications],
        }))
        .unwrap_or_else(|_| "{\"type\": \"aa{sv}\", \"data\": [[]]}".to_string())
    }

    async fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    async fn history_remove(&self, id: Option<u32>) {
        self.history.lock().unwrap().remove(id);
    }

    async fn close_all_notifications(&self) {
        self.tx.send(DaemonEvent::CloseAll).await.ok();
    }

    async fn invoke_action(&self, id: u32, action_key: String) {
        self.tx
            .send(DaemonEvent::InvokeAction { id, action_key })
            .await
            .ok();
    }
}

fn summarize_hint_value(value: &OwnedValue) -> String {
    if let Ok(ov) = value.try_clone() {
        if let Ok(s) = String::try_from(ov) {
            return format!("string({})", s);
        }
    }

    if let Ok(ov) = value.try_clone() {
        if let Ok(structure) = Structure::try_from(ov) {
            let sig = structure.full_signature().as_str();
            let len = structure.fields().len();
            if sig == "(iiibiiay)" && len == 7 {
                if let Some(last) = structure.fields().last() {
                    if let Ok(owned_last) = OwnedValue::try_from(last) {
                        if let Ok(bytes) = Vec::<u8>::try_from(owned_last) {
                            return format!(
                                "structure(sig={}, fields={}, data_len={})",
                                sig,
                                len,
                                bytes.len()
                            );
                        }
                    }
                }
            }
            return format!("structure(sig={}, fields={})", sig, len);
        }
    }

    if let Ok(ov) = value.try_clone() {
        if let Ok(bytes) = Vec::<u8>::try_from(ov) {
            return format!("bytes(len={})", bytes.len());
        }
    }

    format!("{:?}", value)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub async fn run(
    event_tx: Sender<DaemonEvent>,
    mut signal_rx: UnboundedReceiver<DaemonSignal>,
    startup_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
    history: Arc<Mutex<History>>,
) -> zbus::Result<()> {
    let server = NotificationServer {
        tx: event_tx.clone(),
        next_id: Arc::new(AtomicU32::new(1)),
    };

    let builder = match zbus::connection::Builder::session()
        .and_then(|builder| builder.name("org.freedesktop.Notifications"))
        .and_then(|builder| builder.serve_at("/org/freedesktop/Notifications", server))
        .and_then(|builder| {
            let control = ControlServer {
                tx: event_tx.clone(),
                history: Arc::clone(&history),
            };
            builder.serve_at("/org/rnd/Control", control)
        }) {
        Ok(builder) => builder,
        Err(err) => {
            let _ = startup_tx.send(Err(err.to_string()));
            return Err(err);
        }
    };

    let conn = match builder.build().await {
        Ok(conn) => Arc::new(conn),
        Err(err) => {
            let _ = startup_tx.send(Err(err.to_string()));
            return Err(err);
        }
    };

    let _ = startup_tx.send(Ok(()));
    tracing::info!("D-Bus server registered as org.freedesktop.Notifications");

    #[cfg(unix)]
    {
        use sd_notify::NotifyState;
        sd_notify::notify(&[NotifyState::Ready]).ok();
    }

    let conn_sig = Arc::clone(&conn);
    tokio::spawn(async move {
        while let Some(signal) = signal_rx.recv().await {
            let res = match signal {
                DaemonSignal::NotificationClosed { id, reason } => {
                    emit_signal(&conn_sig, "NotificationClosed", &(id, reason)).await
                }
                DaemonSignal::ActionInvoked { id, ref action_key } => {
                    emit_signal(&conn_sig, "ActionInvoked", &(id, action_key.as_str())).await
                }
            };
            if let Err(e) = res {
                tracing::warn!("Failed to emit D-Bus signal: {}", e);
            }
        }
    });

    std::future::pending::<()>().await;
    Ok(())
}

async fn emit_signal<B>(conn: &zbus::Connection, name: &str, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let dest: Option<zbus::names::BusName<'_>> = None;
    conn.emit_signal(
        dest,
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        name,
        body,
    )
    .await
}
