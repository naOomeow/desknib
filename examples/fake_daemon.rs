//! Throwaway helper for testing: holds org.freedesktop.Notifications and
//! actually answers Notify/Introspect, so the main binary must fall back to
//! monitor mode, like a real desktop that already runs its own notification
//! daemon (e.g. Cinnamon) - and so ordinary tools (gdbus, notify-send) can
//! send it a real notification to prove desknib's monitor picks it up.
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use zbus::blocking::connection;
use zbus::fdo::RequestNameFlags;
use zbus::interface;
use zbus::zvariant::OwnedValue;

struct Daemon;

#[interface(name = "org.freedesktop.Notifications")]
impl Daemon {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        _replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        _body: &str,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        eprintln!("fake daemon received Notify from {app_name:?}: {summary:?}");
        1
    }

    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body"]
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("fake-daemon", "fake-daemon", "0", "1.2")
    }
}

fn main() {
    let conn = connection::Builder::session()
        .unwrap()
        .serve_at("/org/freedesktop/Notifications", Daemon)
        .unwrap()
        .build()
        .unwrap();
    let reply = conn
        .request_name_with_flags(
            "org.freedesktop.Notifications",
            RequestNameFlags::ReplaceExisting.into(),
        )
        .unwrap();
    eprintln!("fake daemon owns the name: {:?}", reply);
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
