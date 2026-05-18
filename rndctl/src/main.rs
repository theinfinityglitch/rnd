use clap::{Parser, Subcommand};
use serde_json::Value;
use std::error::Error;
use zbus::Proxy;

const DBUS_DESTINATION: &str = "org.freedesktop.Notifications";
const DBUS_PATH: &str = "/org/freedesktop/Notifications";
const DBUS_INTERFACE: &str = "org.freedesktop.Notifications";
const CONTROL_PATH: &str = "/org/rnd/Control";
const CONTROL_INTERFACE: &str = "org.rnd.Control";

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
    /// Close all active notifications
    CloseAll,
    /// Invoke an action on an active notification
    Action {
        /// Notification ID
        id: u32,
        /// Action key to invoke
        action_key: String,
    },
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
    /// Clear the in-memory history
    Clear,
    /// Remove the last notification or the specified notification by ID
    Remove {
        /// Notification ID
        id: Option<u32>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Close { id } => close_notification(id).await?,
        Command::CloseAll => close_all().await?,
        Command::Action { id, action_key } => invoke_action(id, action_key).await?,
        Command::History { subcommand, limit } => match subcommand {
            Some(HistoryCommand::Clear) => clear_history().await?,
            Some(HistoryCommand::Remove { id }) => remove_message(id).await?,
            None => print_history(limit).await?,
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

async fn control_proxy() -> Result<Proxy<'static>, Box<dyn Error>> {
    let connection = zbus::Connection::session().await?;
    let proxy = Proxy::new(
        &connection,
        DBUS_DESTINATION,
        CONTROL_PATH,
        CONTROL_INTERFACE,
    )
    .await?;
    Ok(proxy)
}

async fn close_notification(id: u32) -> Result<(), Box<dyn Error>> {
    let proxy = notification_proxy().await?;
    proxy.call_method("CloseNotification", &(id)).await?;
    println!("Closed notification {}", id);
    Ok(())
}

async fn close_all() -> Result<(), Box<dyn Error>> {
    let proxy = control_proxy().await?;
    proxy.call_method("CloseAllNotifications", &()).await?;
    println!("Requested close for all active notifications.");
    Ok(())
}

async fn invoke_action(id: u32, action_key: String) -> Result<(), Box<dyn Error>> {
    let proxy = control_proxy().await?;
    proxy
        .call_method("InvokeAction", &(id, action_key.clone()))
        .await?;
    println!("Invoked action {} on notification {}", action_key, id);
    Ok(())
}

async fn print_history(limit: Option<usize>) -> Result<(), Box<dyn Error>> {
    let proxy = control_proxy().await?;
    let result: String = proxy
        .call_method("GetHistory", &())
        .await?
        .body()
        .deserialize_unchecked()?;

    if let Some(limit) = limit {
        if let Ok(mut value) = serde_json::from_str::<Value>(&result) {
            if let Some(data) = value.get_mut("data") {
                if let Some(array) = data.as_array_mut() {
                    if let Some(notifs) = array.get_mut(0).and_then(|v| v.as_array_mut()) {
                        notif_limit_slice(notifs, limit);
                    }
                }
            }
            println!("{}", serde_json::to_string_pretty(&value)?);
            return Ok(());
        }
    }

    println!("{}", result);
    Ok(())
}

fn notif_limit_slice(notifs: &mut Vec<Value>, limit: usize) {
    if notifs.len() > limit {
        notifs.truncate(limit);
    }
}

async fn clear_history() -> Result<(), Box<dyn Error>> {
    let proxy = control_proxy().await?;
    proxy.call_method("ClearHistory", &()).await?;
    println!("History cleared.");
    Ok(())
}

async fn remove_message(id: Option<u32>) -> Result<(), Box<dyn Error>> {
    let proxy = control_proxy().await?;
    proxy.call_method("HistoryRemove", &id).await?;
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
