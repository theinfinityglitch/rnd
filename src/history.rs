use std::collections::VecDeque;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::notification::Notification;

// ── Serialisable history entry ────────────────────────────────────────────────
// We flatten Notification into a lighter struct for persistence —
// OwnedValue (hints) doesn't implement Serialize generically.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub urgency: crate::notification::Urgency,
    pub timestamp: u64,
}

impl From<&Notification> for HistoryEntry {
    fn from(n: &Notification) -> Self {
        Self {
            id: n.id,
            app_name: n.app_name.clone(),
            app_icon: n.app_icon.clone(),
            summary: n.summary.clone(),
            body: n.body.clone(),
            urgency: n.urgency,
            timestamp: n.timestamp,
        }
    }
}

// ── History store ─────────────────────────────────────────────────────────────

pub struct History {
    entries: VecDeque<HistoryEntry>,
    max_size: usize,
    persist: bool,
    path: PathBuf,
}

impl History {
    pub fn new(max_size: usize, persist: bool) -> Self {
        let path = history_path();

        if !persist && path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                warn!("Cannot remove old history file: {}", e);
            }
        }

        let entries = if persist {
            load_from_disk(&path, max_size)
        } else {
            VecDeque::new()
        };

        Self {
            entries,
            max_size,
            persist,
            path,
        }
    }

    pub fn all_entries(&self) -> Vec<HistoryEntry> {
        self.entries.iter().cloned().collect()
    }

    /// Push a new notification; evicts oldest entries beyond max_size.
    pub fn push(&mut self, notif: &Notification) {
        // Remove existing entry with same ID (replace scenario)
        self.entries.retain(|e| e.id != notif.id);

        self.entries.push_front(HistoryEntry::from(notif));

        while self.entries.len() > self.max_size {
            self.entries.pop_back();
        }

        if self.persist {
            self.save();
        }
    }

    pub fn entries(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        if self.persist {
            self.save();
        }
    }

    // ── Persistence ──────────────────────────────────────────────────────────

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("Cannot create history dir: {}", e);
                return;
            }
        }
        let entries: Vec<&HistoryEntry> = self.entries.iter().collect();
        match serde_json::to_string_pretty(&entries) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    warn!("Cannot write history: {}", e);
                }
            }
            Err(e) => warn!("Cannot serialise history: {}", e),
        }
    }
}

fn history_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rnd")
        .join("history.json")
}

fn load_from_disk(path: &PathBuf, max_size: usize) -> VecDeque<HistoryEntry> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<Vec<HistoryEntry>>(&s) {
            Ok(mut v) => {
                v.truncate(max_size);
                info!("Loaded {} history entries from {:?}", v.len(), path);
                VecDeque::from(v)
            }
            Err(e) => {
                warn!("Cannot parse history file: {}", e);
                VecDeque::new()
            }
        },
        Err(_) => VecDeque::new(),
    }
}
