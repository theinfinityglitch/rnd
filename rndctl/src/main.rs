use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use zbus::Proxy;

const DBUS_DESTINATION: &str = "org.freedesktop.Notifications";
const DBUS_PATH: &str = "/org/freedesktop/Notifications";
const DBUS_INTERFACE: &str = "org.freedesktop.Notifications";

#[derive(Parser)]
#[command(name = "rndctl")]
#[command(about = "Control the rnd notification daemon from the command line", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Close a notification by ID
    Close {
        /// Notification ID to close
        id: u32,
    },
    /// Close all notifications tracked by rnd
    CloseAll,
    /// Read or clear the notification history
    History {
        #[command(subcommand)]
        subcommand: Option<HistoryCommand>,
        /// Limit the number of history entries printed
        #[arg(short, long, value_name = "N")]
        limit: Option<usize>,
    },
    /// Print the daemon server information
    Info,
    /// Print the daemon capabilities reported over D-Bus
    Capabilities,
}

#[derive(Subcommand)]
enum HistoryCommand {
    /// Clear persisted history
    Clear,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct HistoryEntry {
    id: u32,
    app_name: String,
    app_icon: String,
    summary: String,
    body: String,
    urgency: String,
    timestamp: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Close { id } => close_notification(id).await?,
        Command::CloseAll => close_all().await?,
        Command::History { subcommand, limit } => match subcommand {
            Some(HistoryCommand::Clear) => clear_history()?,
            None => print_history(limit)?,
        },
        Command::Info => print_server_info().await?,
        Command::Capabilities => print_capabilities().await?,
    }

    Ok(())
}

async fn notification_proxy() -> Result<Proxy<'static>, Box<dyn Error>> {
    let connection = zbus::Connection::session().await?;
    let proxy = Proxy::new(&connection, DBUS_DESTINATION, DBUS_PATH, DBUS_INTERFACE).await?;
    Ok(proxy)
}

async fn close_notification(id: u32) -> Result<(), Box<dyn Error>> {
    let proxy = notification_proxy().await?;
    proxy.call_method("CloseNotification", &(id)).await?;
    println!("Closed notification {}", id);
    Ok(())
}

async fn close_all() -> Result<(), Box<dyn Error>> {
    let ids = match read_history_ids() {
        Ok(ids) => ids,
        Err(_) => Vec::new(),
    };

    if ids.is_empty() {
        println!("No persisted notification history found. Nothing to close.");
        return Ok(());
    }

    let proxy = notification_proxy().await?;
    for id in ids {
        if let Err(err) = proxy.call_method("CloseNotification", &(id)).await {
            eprintln!("Warning: failed to close {}: {}", id, err);
        }
    }

    println!("Requested close for all notifications from history.");
    Ok(())
}

fn history_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rnd")
        .join("history.json")
}

fn read_history_entries() -> Result<Vec<HistoryEntry>, Box<dyn Error>> {
    let path = history_path();
    let content = fs::read_to_string(&path)?;
    let entries: Vec<HistoryEntry> = serde_json::from_str(&content)?;
    Ok(entries)
}

fn read_history_ids() -> Result<Vec<u32>, Box<dyn Error>> {
    let entries = read_history_entries()?;
    let ids: HashSet<u32> = entries.into_iter().map(|entry| entry.id).collect();
    let mut ids: Vec<u32> = ids.into_iter().collect();
    ids.sort_unstable();
    Ok(ids)
}

fn print_history(limit: Option<usize>) -> Result<(), Box<dyn Error>> {
    let entries = match read_history_entries() {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to read history: {}", err);
            return Ok(());
        }
    };

    if entries.is_empty() {
        println!("No history entries found.");
        return Ok(());
    }

    let limit = limit.unwrap_or(entries.len());
    for entry in entries.into_iter().take(limit) {
        println!(
            "[{}] {} | {} | {}",
            entry.id, entry.urgency, entry.app_name, entry.summary
        );
        if !entry.body.is_empty() {
            println!("    {}", entry.body);
        }
    }

    Ok(())
}

fn clear_history() -> Result<(), Box<dyn Error>> {
    let path = history_path();
    if !path.exists() {
        println!("History already empty.");
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, "[]")?;
    println!("History cleared.");
    Ok(())
}

async fn print_server_info() -> Result<(), Box<dyn Error>> {
    let proxy = notification_proxy().await?;
    let message = proxy.call_method("GetServerInformation", &()).await?;
    let (name, vendor, version, spec_version) = message
        .body()
        .deserialize_unchecked::<(String, String, String, String)>()?;
    println!(
        "Name: {}\nVendor: {}\nVersion: {}\nSpec Version: {}",
        name, vendor, version, spec_version
    );
    Ok(())
}

async fn print_capabilities() -> Result<(), Box<dyn Error>> {
    let proxy = notification_proxy().await?;
    let message = proxy.call_method("GetCapabilities", &()).await?;
    let capabilities = message.body().deserialize_unchecked::<Vec<String>>()?;
    if capabilities.is_empty() {
        println!("No capabilities reported.");
        return Ok(());
    }
    println!("Capabilities:");
    for cap in capabilities {
        println!("  - {}", cap);
    }
    Ok(())
}
