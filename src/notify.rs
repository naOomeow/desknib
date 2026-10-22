//! Receives desktop notifications over D-Bus (org.freedesktop.Notifications).
//!
//! Preferred: own the notification service, so the desktop's own popups go
//! away and bubbles can trigger the sender's default action. When the desktop
//! keeps the service (Cinnamon does), fall back to watching the bus as a
//! monitor: every notification is still seen, but the desktop's popups must
//! be switched off by the user and clicks cannot open the sender.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use zbus::blocking::{Connection, MessageIterator, connection};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedValue;
use zbus::{MatchRule, interface};

use crate::inbox::{Notification, strip_markup};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

#[derive(Debug)]
pub enum Event {
    Show(Notification),
    /// The sender withdrew a notification.
    Close(u32),
}

/// Why a notification closed, as the spec numbers it.
#[derive(Debug, Clone, Copy)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// desknib is the notification service; desktop popups are gone.
    Service,
    /// The desktop keeps its service; desknib only listens.
    Listening,
}

/// Talks back to notification senders. Only the service mode can.
#[derive(Clone)]
pub struct Notifier {
    conn: Option<Connection>,
}

impl Notifier {
    /// For when the session bus is unreachable.
    pub fn none() -> Notifier {
        Notifier { conn: None }
    }

    pub fn closed(&self, id: u32, reason: CloseReason) {
        if let Some(conn) = &self.conn {
            let _ = conn.emit_signal(
                None::<&str>,
                PATH,
                NAME,
                "NotificationClosed",
                &(id, reason as u32),
            );
        }
    }

    pub fn invoke_default(&self, id: u32) {
        if let Some(conn) = &self.conn {
            let _ = conn.emit_signal(None::<&str>, PATH, NAME, "ActionInvoked", &(id, "default"));
        }
    }
}

struct Service {
    tx: Sender<Event>,
    next_id: AtomicU32,
}

#[interface(name = "org.freedesktop.Notifications")]
impl Service {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "body-markup", "actions"]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        _app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            // Ids start at 1; 0 means "no notification" in the spec.
            self.next_id.fetch_add(1, Ordering::Relaxed).max(1)
        };
        let _ = self.tx.send(Event::Show(to_notification(
            id, app_name, summary, body, &actions, &hints,
        )));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.tx.send(Event::Close(id));
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("desknib", "desknib", env!("CARGO_PKG_VERSION"), "1.2")
    }
}

fn to_notification(
    id: u32,
    app: &str,
    summary: &str,
    body: &str,
    actions: &[String],
    hints: &HashMap<String, OwnedValue>,
) -> Notification {
    // Actions come in (key, label) pairs.
    let has_default_action = actions.chunks(2).any(|pair| pair[0] == "default");
    let critical = hints
        .get("urgency")
        .and_then(|v| u8::try_from(v).ok())
        .is_some_and(|u| u == 2);
    Notification {
        id,
        app: strip_markup(app),
        summary: strip_markup(summary),
        body: strip_markup(body),
        has_default_action,
        critical,
    }
}

/// Starts listening on the session bus. Returns which mode it got.
pub fn start(tx: Sender<Event>) -> zbus::Result<(Mode, Notifier)> {
    let service = Service {
        tx: tx.clone(),
        next_id: AtomicU32::new(1),
    };
    let conn = connection::Builder::session()?
        .serve_at(PATH, service)?
        .build()?;
    // zbus turns a "name already owned by someone else" reply into
    // `Err(Error::NameTaken)` instead of `Ok(RequestNameReply::Exists)`, so
    // that case must be matched explicitly rather than propagated with `?`:
    // it means "fall back to listening", not "notifications are broken".
    match conn.request_name_with_flags(
        NAME,
        RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue,
    ) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            // zbus serves the interface on its own executor thread.
            return Ok((Mode::Service, Notifier { conn: Some(conn) }));
        }
        Ok(_) | Err(zbus::Error::NameTaken) => {}
        Err(e) => return Err(e),
    }
    drop(conn);

    let conn = Connection::session()?;
    let rule = MatchRule::builder()
        .msg_type(MessageType::MethodCall)
        .interface(NAME)?
        .member("Notify")?
        .build();
    zbus::blocking::fdo::MonitoringProxy::new(&conn)?.become_monitor(&[rule], 0)?;
    std::thread::Builder::new()
        .name("notify-monitor".into())
        .spawn(move || monitor(conn, tx))
        .map_err(|e| zbus::Error::Failure(e.to_string()))?;
    Ok((Mode::Listening, Notifier { conn: None }))
}

type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

fn monitor(conn: Connection, tx: Sender<Event>) {
    // Without the service, real ids are unknown; number them ourselves, far
    // above anything a desktop hands out so replaces_id values cannot clash.
    let mut next_id: u32 = 1 << 30;
    for msg in MessageIterator::from(&conn) {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        if msg.message_type() != MessageType::MethodCall
            || header.member().is_none_or(|m| m.as_str() != "Notify")
        {
            continue;
        }
        let Ok((app, replaces, _icon, summary, body, actions, hints, _timeout)) =
            msg.body().deserialize::<NotifyArgs>()
        else {
            continue;
        };
        let id = if replaces != 0 {
            replaces
        } else {
            next_id = next_id.wrapping_add(1).max(1 << 30);
            next_id
        };
        let n = to_notification(id, &app, &summary, &body, &actions, &hints);
        // Clicks cannot reach the sender in this mode.
        let n = Notification {
            has_default_action: false,
            ..n
        };
        if tx.send(Event::Show(n)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use zbus::zvariant::Value;

    // Needs a real session bus: run under `dbus-run-session --` (CI does;
    // locally, `dbus-run-session -- cargo test`). Skips quietly otherwise
    // rather than failing a developer's plain `cargo test`.
    fn has_session_bus() -> bool {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
    }

    /// Both halves of `start()`'s fallback, in one test so they cannot run
    /// concurrently and fight over who owns the name on the shared bus.
    ///
    /// The regression this guards: zbus turns a "name already taken" reply
    /// into `Err(Error::NameTaken)` rather than `Ok(RequestNameReply::Exists)`,
    /// and `start()` must treat that as "fall back to listening", not as
    /// failure. This is the bug behind notifications silently not working
    /// on desktops (Cinnamon among them) that run their own notification
    /// daemon by default.
    #[test]
    fn start_picks_service_or_listening_depending_on_who_owns_the_name() {
        if !has_session_bus() {
            eprintln!("skipping: no session bus");
            return;
        }

        let (tx, _rx) = mpsc::channel();
        let (mode, _notifier) = start(tx).unwrap();
        assert_eq!(mode, Mode::Service, "nothing else owns the name yet");

        // Stand in for Cinnamon's own notification daemon. With DoNotQueue
        // set, zbus itself turns the losing reply into this same
        // Err(NameTaken) rather than Ok(RequestNameReply::Exists) - exactly
        // the surprise start() has to handle, so this doubles as a sanity
        // check on that behaviour.
        let holder = connection::Builder::session().unwrap().build().unwrap();
        let err = holder
            .request_name_with_flags(
                NAME,
                RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue,
            )
            .unwrap_err();
        assert!(
            matches!(err, zbus::Error::NameTaken),
            "the first start() above should still hold it: {err}"
        );

        let (tx, _rx) = mpsc::channel();
        let (mode, _notifier) =
            start(tx).expect("start() must not fail just because the name is taken");
        assert_eq!(mode, Mode::Listening);
    }

    fn hints(urgency: Option<u8>) -> HashMap<String, OwnedValue> {
        let mut h = HashMap::new();
        if let Some(u) = urgency {
            h.insert(
                "urgency".to_string(),
                OwnedValue::try_from(Value::U8(u)).unwrap(),
            );
        }
        h
    }

    #[test]
    fn default_action_and_urgency_are_read() {
        let actions = vec![
            "reply".to_string(),
            "Reply".to_string(),
            "default".to_string(),
            "Open".to_string(),
        ];
        let n = to_notification(
            7,
            "Mail",
            "<b>New</b>",
            "from &amp; to",
            &actions,
            &hints(Some(2)),
        );
        assert!(n.has_default_action && n.critical);
        assert_eq!(
            (n.id, n.summary.as_str(), n.body.as_str()),
            (7, "New", "from & to")
        );
    }

    #[test]
    fn action_labels_are_not_mistaken_for_keys() {
        let actions = vec!["open".to_string(), "default".to_string()];
        let n = to_notification(1, "a", "b", "c", &actions, &hints(Some(1)));
        assert!(!n.has_default_action && !n.critical);
    }

    #[test]
    fn missing_hints_mean_normal_urgency() {
        let n = to_notification(1, "a", "b", "c", &[], &hints(None));
        assert!(!n.critical);
    }
}
