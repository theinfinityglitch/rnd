pub mod app;
pub mod config;
pub mod dbus;
pub mod history;
pub mod installer;
pub mod notification;

pub const DBUS_DESTINATION: &str = "org.freedesktop.Notifications";
pub const DBUS_PATH: &str = "/org/freedesktop/Notifications";
pub const DBUS_INTERFACE: &str = "org.freedesktop.Notifications";

pub const CONTROL_PATH: &str = "/org/rnd/Control";
pub const CONTROL_INTERFACE: &str = "org.rnd.Control";

pub const APP_ID: &str = "org.rnd.NotificationDaemon";

pub const DEFAULT_CSS: &str = include_str!("../assets/style.css");
pub const RND_SERVICE_SRC: &str = include_str!("../contrib/rnd.service");
pub const DBUS_SRC: &str = include_str!("../contrib/org.freedesktop.Notifications.service");
