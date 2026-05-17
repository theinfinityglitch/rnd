use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_channel::Sender;
use tokio::sync::mpsc::UnboundedReceiver;
use zbus::interface;
use zbus::zvariant::{OwnedValue, Structure};

use crate::notification::Notification;

// ── Cross-thread messages ─────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DaemonEvent {
    Show(Notification),
    Close(u32),
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

        tracing::debug!("Notify id={} summary={:?}", id, notif.summary);
        self.tx.send(DaemonEvent::Show(notif)).await.ok();
        id
    }

    async fn close_notification(&self, id: u32) {
        tracing::debug!("CloseNotification id={}", id);
        self.tx.send(DaemonEvent::Close(id)).await.ok();
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
) -> zbus::Result<()> {
    let server = NotificationServer {
        tx: event_tx,
        next_id: Arc::new(AtomicU32::new(1)),
    };

    let builder = match zbus::connection::Builder::session()
        .and_then(|builder| builder.name("org.freedesktop.Notifications"))
        .and_then(|builder| builder.serve_at("/org/freedesktop/Notifications", server))
    {
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
                    tracing::debug!("Emitting NotificationClosed id={} reason={}", id, reason);
                    emit_signal(&conn_sig, "NotificationClosed", &(id, reason)).await
                }
                DaemonSignal::ActionInvoked { id, ref action_key } => {
                    tracing::debug!("Emitting ActionInvoked id={} key={}", id, action_key);
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
