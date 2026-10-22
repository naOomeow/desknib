//! Which notification is on screen, what waits, and what was held back.

use std::collections::VecDeque;

use crate::bubble::Bubble;

/// A desktop notification, already stripped of markup.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    /// The sender offered a "default" action (opening the chat, the mail…).
    pub has_default_action: bool,
    pub critical: bool,
}

impl Notification {
    pub fn bubble(&self, earlier_dropped: usize) -> Bubble {
        let mut title = if self.app.trim().is_empty() {
            None
        } else {
            Some(self.app.trim().to_string())
        };
        let summary = self.summary.trim();
        let body = self.body.trim();
        let text = match (summary.is_empty(), body.is_empty()) {
            (false, false) => format!("{summary}: {body}"),
            (false, true) => summary.to_string(),
            (true, false) => body.to_string(),
            (true, true) => String::new(),
        };
        if earlier_dropped > 0 {
            let more = format!("+{earlier_dropped} earlier");
            title = Some(match title {
                Some(t) => format!("{t} · {more}"),
                None => more,
            });
        }
        Bubble { title, body: text }
    }
}

/// Most notifications kept waiting; older ones beyond this are dropped.
const MAX_QUEUED: usize = 5;

#[derive(Debug, Default)]
pub struct Inbox {
    pub current: Option<Notification>,
    queue: VecDeque<Notification>,
    /// Notifications dropped from a full queue since the last one shown.
    dropped: usize,
    /// Notifications held back while quiet or hidden.
    missed: Vec<Notification>,
}

/// What the caller must do after a change to the inbox.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// The bubble on screen must change to the current notification.
    pub redisplay: bool,
    /// Notifications that will never be shown, so senders hear they closed.
    pub discarded: Vec<u32>,
}

impl Inbox {
    /// Accepts a notification. `holding` is true while quiet or hidden;
    /// critical notifications still come through then.
    pub fn push(&mut self, n: Notification, holding: bool) -> Outcome {
        let mut out = Outcome::default();
        // A sender updating a notification it already sent.
        if self.current.as_ref().is_some_and(|c| c.id == n.id) {
            self.current = Some(n);
            out.redisplay = true;
            return out;
        }
        if let Some(q) = self.queue.iter_mut().find(|q| q.id == n.id) {
            *q = n;
            return out;
        }
        if holding && !n.critical {
            self.missed.retain(|m| m.id != n.id);
            self.missed.push(n);
            return out;
        }
        if self.current.is_none() {
            self.current = Some(n);
            out.redisplay = true;
            return out;
        }
        if n.critical {
            self.queue.push_front(n);
        } else {
            self.queue.push_back(n);
        }
        while self.queue.len() > MAX_QUEUED {
            let pos = self.queue.iter().position(|q| !q.critical).unwrap_or(0);
            if let Some(old) = self.queue.remove(pos) {
                self.dropped += 1;
                out.discarded.push(old.id);
            }
        }
        out
    }

    /// The current notification is finished (expired, clicked or closed);
    /// moves on to the next one. Returns the finished one.
    pub fn advance(&mut self) -> Option<Notification> {
        let done = self.current.take();
        self.current = self.queue.pop_front();
        done
    }

    /// The sender withdrew a notification. Returns true when it was on screen.
    pub fn close(&mut self, id: u32) -> bool {
        self.queue.retain(|q| q.id != id);
        self.missed.retain(|m| m.id != id);
        if self.current.as_ref().is_some_and(|c| c.id == id) {
            self.advance();
            return true;
        }
        false
    }

    /// Bubble for the current notification, mentioning drops once.
    pub fn current_bubble(&mut self) -> Option<Bubble> {
        let b = self.current.as_ref()?.bubble(self.dropped);
        self.dropped = 0;
        Some(b)
    }

    /// Ends a quiet spell: a summary of what was held back, if anything.
    /// The held notifications are discarded.
    pub fn take_missed(&mut self) -> Option<(Bubble, Vec<u32>)> {
        let last = self.missed.last()?.clone();
        let ids = self.missed.drain(..).map(|m| m.id).collect::<Vec<_>>();
        let count = ids.len();
        let mut bubble = last.bubble(0);
        if count > 1 {
            bubble.title = Some(format!(
                "{} notifications while you were away · latest from {}",
                count,
                if last.app.is_empty() {
                    "an app"
                } else {
                    &last.app
                }
            ));
        }
        Some((bubble, ids))
    }
}

/// Notification bodies may carry a small HTML subset; bubbles show plain text.
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => {
                let mut tag = String::new();
                for t in chars.by_ref() {
                    if t == '>' {
                        break;
                    }
                    tag.push(t);
                }
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("");
                if name.eq_ignore_ascii_case("br") || name.eq_ignore_ascii_case("p") {
                    out.push(' ');
                }
            }
            '&' => {
                let mut entity = String::new();
                while let Some(&n) = chars.peek() {
                    if n == ';' || entity.len() > 8 {
                        break;
                    }
                    entity.push(n);
                    chars.next();
                }
                let decoded = match entity.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16)
                        .ok()
                        .and_then(char::from_u32),
                    e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
                    _ => None,
                };
                match (decoded, chars.peek()) {
                    (Some(d), Some(';')) => {
                        chars.next();
                        out.push(d);
                    }
                    _ => {
                        out.push('&');
                        out.push_str(&entity);
                    }
                }
            }
            '\n' | '\r' | '\t' => out.push(' '),
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: u32, app: &str) -> Notification {
        Notification {
            id,
            app: app.into(),
            summary: format!("summary {id}"),
            body: String::new(),
            has_default_action: false,
            critical: false,
        }
    }

    fn critical(id: u32) -> Notification {
        Notification {
            critical: true,
            ..n(id, "Battery")
        }
    }

    #[test]
    fn first_notification_shows_at_once_and_others_wait() {
        let mut inbox = Inbox::default();
        assert!(inbox.push(n(1, "a"), false).redisplay);
        assert!(!inbox.push(n(2, "b"), false).redisplay);
        assert_eq!(inbox.advance().unwrap().id, 1);
        assert_eq!(inbox.current.as_ref().unwrap().id, 2);
        inbox.advance();
        assert!(inbox.current.is_none());
    }

    #[test]
    fn replacing_the_current_notification_redisplays_it() {
        let mut inbox = Inbox::default();
        inbox.push(n(1, "a"), false);
        let updated = Notification {
            summary: "new".into(),
            ..n(1, "a")
        };
        assert!(inbox.push(updated, false).redisplay);
        assert_eq!(inbox.current.as_ref().unwrap().summary, "new");
    }

    #[test]
    fn replacing_a_queued_notification_keeps_its_place() {
        let mut inbox = Inbox::default();
        inbox.push(n(1, "a"), false);
        inbox.push(n(2, "b"), false);
        inbox.push(n(3, "c"), false);
        inbox.push(
            Notification {
                summary: "edited".into(),
                ..n(2, "b")
            },
            false,
        );
        inbox.advance();
        assert_eq!(inbox.current.as_ref().unwrap().summary, "edited");
    }

    #[test]
    fn critical_notifications_jump_the_queue() {
        let mut inbox = Inbox::default();
        inbox.push(n(1, "a"), false);
        inbox.push(n(2, "b"), false);
        inbox.push(critical(3), false);
        inbox.advance();
        assert_eq!(inbox.current.as_ref().unwrap().id, 3);
    }

    #[test]
    fn full_queue_drops_oldest_and_mentions_it_once() {
        let mut inbox = Inbox::default();
        inbox.push(n(1, "a"), false);
        let mut discarded = Vec::new();
        for id in 2..=8 {
            discarded.extend(inbox.push(n(id, "chat"), false).discarded);
        }
        assert_eq!(discarded, vec![2, 3]);
        inbox.advance();
        assert_eq!(inbox.current.as_ref().unwrap().id, 4);
        assert_eq!(
            inbox.current_bubble().unwrap().title.unwrap(),
            "chat · +2 earlier"
        );
        assert_eq!(inbox.current_bubble().unwrap().title.unwrap(), "chat");
    }

    #[test]
    fn holding_keeps_notifications_back_except_critical() {
        let mut inbox = Inbox::default();
        assert!(!inbox.push(n(1, "Mail"), true).redisplay);
        assert!(inbox.current.is_none());
        assert!(inbox.push(critical(2), true).redisplay);
        inbox.advance();
        inbox.push(n(3, "Discord"), true);
        let (bubble, ids) = inbox.take_missed().unwrap();
        assert_eq!(ids, vec![1, 3]);
        assert!(
            bubble
                .title
                .unwrap()
                .starts_with("2 notifications while you were away")
        );
        assert!(inbox.take_missed().is_none());
    }

    #[test]
    fn closing_removes_from_screen_queue_and_missed() {
        let mut inbox = Inbox::default();
        inbox.push(n(1, "a"), false);
        inbox.push(n(2, "b"), false);
        assert!(!inbox.close(2));
        assert!(inbox.close(1));
        assert!(inbox.current.is_none());
        inbox.push(n(3, "c"), true);
        inbox.close(3);
        assert!(inbox.take_missed().is_none());
    }

    #[test]
    fn bubble_text_combines_summary_and_body() {
        let mut x = n(1, "Discord");
        x.summary = "sam".into();
        x.body = "you up for tonight?".into();
        let b = x.bubble(0);
        assert_eq!(b.title.as_deref(), Some("Discord"));
        assert_eq!(b.body, "sam: you up for tonight?");
        x.app = " ".into();
        x.body.clear();
        assert_eq!(
            x.bubble(0),
            Bubble {
                title: None,
                body: "sam".into()
            }
        );
    }

    #[test]
    fn markup_is_stripped_and_entities_decoded() {
        assert_eq!(strip_markup("<b>Hi</b> &amp; <i>bye</i>"), "Hi & bye");
        assert_eq!(strip_markup("line one<br/>line\ntwo"), "line one line two");
        assert_eq!(strip_markup("a &lt;tag&gt; &#65;&#x42;"), "a <tag> AB");
        assert_eq!(strip_markup("<a href=\"x\">link</a>"), "link");
        assert_eq!(strip_markup("R&D rocks"), "R&D rocks");
    }
}
