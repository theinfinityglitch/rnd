use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zbus::zvariant::OwnedValue;

// ── Urgency ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    pub fn from_hints(hints: &HashMap<String, OwnedValue>) -> Self {
        hints
            .get("urgency")
            .and_then(|v| {
                let raw: Result<u8, _> = v.try_into();
                raw.ok()
            })
            .map(|b| match b {
                0 => Urgency::Low,
                2 => Urgency::Critical,
                _ => Urgency::Normal,
            })
            .unwrap_or(Urgency::Normal)
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Urgency::Low => "urgency-low",
            Urgency::Normal => "urgency-normal",
            Urgency::Critical => "urgency-critical",
        }
    }
}

// ── Notification ──────────────────────────────────────────────────────────────
//
// NOTE: We do NOT derive Clone because zbus::zvariant::OwnedValue does not
// implement Clone in all versions.  Notification is always moved, never cloned.

#[derive(Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    /// Pairs of (action_key, action_label).
    pub actions: Vec<(String, String)>,
    /// Raw hints — skipped during serialisation (not needed in history).
    #[serde(skip)]
    pub hints: HashMap<String, OwnedValue>,
    /// -1 = server default, 0 = never expire, >0 = milliseconds
    pub expire_timeout: i32,
    pub urgency: Urgency,
    /// Unix timestamp (seconds) when the notification was received.
    pub timestamp: u64,
}

impl Notification {
    pub fn new(
        id: u32,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions_flat: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> Self {
        let urgency = Urgency::from_hints(&hints);

        let mut actions = Vec::new();
        let mut iter = actions_flat.into_iter();
        while let (Some(key), Some(label)) = (iter.next(), iter.next()) {
            actions.push((key, label));
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            id,
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
            urgency,
            timestamp,
        }
    }

    pub fn effective_timeout_ms(&self, defaults: &crate::config::TimeoutConfig) -> Option<u64> {
        match self.expire_timeout {
            0 => None,
            -1 => match self.urgency {
                Urgency::Critical => None,
                Urgency::Low => Some(defaults.low as u64),
                Urgency::Normal => Some(defaults.normal as u64),
            },
            ms => Some(ms as u64),
        }
    }
}

// ── Close reasons (FDO spec §3.6) ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum CloseReason {
    Expired = 1,
    DismissedByUser = 2,
    CloseNotificationCalled = 3,
    Undefined = 4,
}
