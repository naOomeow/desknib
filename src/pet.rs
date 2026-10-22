//! Platform-independent pet state: animation clock, bubbles and pointer gestures.

use std::time::{Duration, Instant};

use crate::bubble::Bubble;

const CLICK_LINES: &[&str] = &[
    "Hi! I'm Nib.",
    "I'll stay out of your way. Drag me anywhere.",
    "Stretch break? Your back will thank you.",
    "Drink some water, I'll guard the desk.",
    "Notifications land here as bubbles instead of popups.",
];

/// How long the pet waits between redraws when nothing animates.
const IDLE_WAKE: Duration = Duration::from_secs(1);
const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(3);
const CRITICAL_BUBBLE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
}

#[derive(Debug)]
pub struct ActiveBubble {
    pub bubble: Bubble,
    pub until: Instant,
    /// Set when the bubble shows a desktop notification.
    pub notification: Option<u32>,
}

#[derive(Debug)]
pub struct Pet {
    pub animation: String,
    pub frame: usize,
    frame_count: usize,
    frame_interval: Duration,
    next_frame_at: Instant,
    pub bubble: Option<ActiveBubble>,
    /// Quiet mode suppresses bubbles nobody asked for.
    pub quiet: bool,
    click_count: usize,
    quit_armed_until: Option<Instant>,
    /// A notification bubble that timed out, for the caller to report.
    expired_notification: Option<u32>,
}

impl Pet {
    pub fn new(animation: &str, frame_count: usize, fps: f32, now: Instant) -> Pet {
        let frame_interval = Duration::from_nanos((1e9 / fps as f64).round() as u64);
        Pet {
            animation: animation.to_string(),
            frame: 0,
            frame_count,
            frame_interval,
            next_frame_at: now + frame_interval,
            bubble: None,
            quiet: false,
            click_count: 0,
            quit_armed_until: None,
            expired_notification: None,
        }
    }

    pub fn set_animation(&mut self, name: &str, frame_count: usize, fps: f32, now: Instant) {
        if self.animation == name {
            return;
        }
        *self = Pet {
            bubble: self.bubble.take(),
            quiet: self.quiet,
            click_count: self.click_count,
            quit_armed_until: self.quit_armed_until,
            expired_notification: self.expired_notification,
            ..Pet::new(name, frame_count, fps, now)
        };
    }

    /// Advances the clock. Returns true when something visible changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.frame_count > 1 && now >= self.next_frame_at {
            // Skip frames we slept through instead of replaying them.
            let behind =
                now.duration_since(self.next_frame_at).as_nanos() / self.frame_interval.as_nanos();
            let steps = behind as usize + 1;
            self.frame = (self.frame + steps) % self.frame_count;
            self.next_frame_at += self.frame_interval * steps as u32;
            changed = true;
        }
        if self.bubble.as_ref().is_some_and(|b| now >= b.until) {
            if let Some(id) = self.bubble.take().and_then(|b| b.notification) {
                self.expired_notification = Some(id);
            }
            changed = true;
        }
        if self.quit_armed_until.is_some_and(|t| now >= t) {
            self.quit_armed_until = None;
        }
        changed
    }

    /// When the main loop should wake next.
    pub fn next_deadline(&self, now: Instant) -> Instant {
        let mut next = if self.frame_count > 1 {
            self.next_frame_at
        } else {
            now + IDLE_WAKE
        };
        if let Some(b) = &self.bubble {
            next = next.min(b.until);
        }
        if let Some(t) = self.quit_armed_until {
            next = next.min(t);
        }
        next
    }

    /// Shows a bubble. Unprompted bubbles are dropped in quiet mode.
    /// Returns true when the bubble is shown.
    pub fn say(&mut self, bubble: Bubble, prompted: bool, now: Instant) -> bool {
        if self.quiet && !prompted {
            return false;
        }
        let until = now + Duration::from_millis(bubble.duration_ms());
        self.bubble = Some(ActiveBubble {
            bubble,
            until,
            notification: None,
        });
        true
    }

    /// Shows a notification. It replaces any chatter on screen; critical
    /// ones stay up until clicked, within reason.
    pub fn show_notification(&mut self, bubble: Bubble, id: u32, critical: bool, now: Instant) {
        let ms = if critical {
            CRITICAL_BUBBLE_MS
        } else {
            bubble.duration_ms() + 1000
        };
        let until = now + Duration::from_millis(ms);
        self.bubble = Some(ActiveBubble {
            bubble,
            until,
            notification: Some(id),
        });
    }

    /// The notification currently on screen, if any.
    pub fn notification_on_screen(&self) -> Option<u32> {
        self.bubble.as_ref().and_then(|b| b.notification)
    }

    /// Removes a notification bubble, leaving chatter alone.
    pub fn clear_notification(&mut self) {
        if self.notification_on_screen().is_some() {
            self.bubble = None;
        }
    }

    /// The notification whose bubble timed out since the last call.
    pub fn take_expired_notification(&mut self) -> Option<u32> {
        self.expired_notification.take()
    }

    pub fn on_click(&mut self, now: Instant) {
        let line = CLICK_LINES[self.click_count % CLICK_LINES.len()];
        self.click_count += 1;
        self.say(Bubble::text(line), true, now);
    }

    /// First right-click asks for confirmation, a second one within a few
    /// seconds quits. Works even when the desktop has no tray.
    pub fn on_secondary_click(&mut self, now: Instant) -> Action {
        if self.quit_armed_until.is_some_and(|t| now < t) {
            return Action::Quit;
        }
        self.quit_armed_until = Some(now + QUIT_CONFIRM_WINDOW);
        self.say(Bubble::text("Right-click again to say bye."), true, now);
        Action::None
    }
}

/// Tells a click from a drag. Coordinates are in screen pixels.
#[derive(Debug, Default)]
pub struct Gesture {
    press: Option<Press>,
}

#[derive(Debug, Clone, Copy)]
struct Press {
    pointer: (i32, i32),
    window: (i32, i32),
    dragging: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GestureEvent {
    None,
    MoveWindow(i32, i32),
    Click,
}

const DRAG_THRESHOLD: i32 = 4;

impl Gesture {
    pub fn press(&mut self, pointer: (i32, i32), window: (i32, i32)) {
        self.press = Some(Press {
            pointer,
            window,
            dragging: false,
        });
    }

    pub fn motion(&mut self, pointer: (i32, i32)) -> GestureEvent {
        let Some(p) = self.press.as_mut() else {
            return GestureEvent::None;
        };
        let (dx, dy) = (pointer.0 - p.pointer.0, pointer.1 - p.pointer.1);
        if !p.dragging && dx.abs().max(dy.abs()) < DRAG_THRESHOLD {
            return GestureEvent::None;
        }
        p.dragging = true;
        GestureEvent::MoveWindow(p.window.0 + dx, p.window.1 + dy)
    }

    pub fn release(&mut self) -> GestureEvent {
        match self.press.take() {
            Some(p) if !p.dragging => GestureEvent::Click,
            _ => GestureEvent::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn frames_advance_on_schedule_and_wrap() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 3, 10.0, t0);
        assert!(!pet.tick(t0 + ms(50)));
        assert!(pet.tick(t0 + ms(100)));
        assert_eq!(pet.frame, 1);
        pet.tick(t0 + ms(200));
        pet.tick(t0 + ms(300));
        assert_eq!(pet.frame, 0);
    }

    #[test]
    fn oversleeping_skips_frames_instead_of_replaying() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 8, 10.0, t0);
        assert!(pet.tick(t0 + ms(450)));
        assert_eq!(pet.frame, 4);
        assert!(pet.next_deadline(t0 + ms(450)) > t0 + ms(450));
    }

    #[test]
    fn single_frame_animation_wakes_rarely() {
        let t0 = Instant::now();
        let pet = Pet::new("idle", 1, 10.0, t0);
        assert_eq!(pet.next_deadline(t0), t0 + IDLE_WAKE);
    }

    #[test]
    fn bubble_expires_and_wakes_the_loop() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        assert!(pet.say(Bubble::text("hi"), false, t0));
        assert_eq!(pet.next_deadline(t0), t0 + ms(3000).min(IDLE_WAKE));
        assert!(pet.tick(t0 + ms(3000)));
        assert!(pet.bubble.is_none());
    }

    #[test]
    fn quiet_mode_drops_only_unprompted_bubbles() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.quiet = true;
        assert!(!pet.say(Bubble::text("unasked"), false, t0));
        assert!(pet.bubble.is_none());
        pet.on_click(t0);
        assert!(pet.bubble.is_some());
    }

    #[test]
    fn clicks_cycle_through_lines() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.on_click(t0);
        let first = pet.bubble.as_ref().unwrap().bubble.body.clone();
        pet.on_click(t0);
        assert_ne!(pet.bubble.as_ref().unwrap().bubble.body, first);
    }

    #[test]
    fn double_right_click_quits_only_within_window() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        assert_eq!(pet.on_secondary_click(t0), Action::None);
        assert_eq!(pet.on_secondary_click(t0 + ms(1000)), Action::Quit);

        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.on_secondary_click(t0);
        pet.tick(t0 + ms(3500));
        assert_eq!(pet.on_secondary_click(t0 + ms(3500)), Action::None);
    }

    #[test]
    fn switching_animation_keeps_bubble_and_restarts_frames() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 4, 10.0, t0);
        pet.tick(t0 + ms(200));
        pet.say(Bubble::text("hi"), true, t0);
        pet.set_animation("dance", 8, 10.0, t0 + ms(200));
        assert_eq!((pet.animation.as_str(), pet.frame), ("dance", 0));
        assert!(pet.bubble.is_some());
    }

    #[test]
    fn notification_bubble_reports_its_expiry_once() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.show_notification(Bubble::text("hi"), 42, false, t0);
        assert_eq!(pet.notification_on_screen(), Some(42));
        assert!(pet.tick(t0 + ms(4000)));
        assert_eq!(pet.take_expired_notification(), Some(42));
        assert_eq!(pet.take_expired_notification(), None);
    }

    #[test]
    fn chatter_expiry_is_not_a_notification() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.say(Bubble::text("hi"), true, t0);
        pet.tick(t0 + ms(4000));
        assert_eq!(pet.take_expired_notification(), None);
    }

    #[test]
    fn critical_notifications_stay_longer() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.show_notification(Bubble::text("Battery low"), 1, true, t0);
        pet.tick(t0 + ms(20_000));
        assert_eq!(pet.notification_on_screen(), Some(1));
    }

    #[test]
    fn clearing_leaves_chatter_alone() {
        let t0 = Instant::now();
        let mut pet = Pet::new("idle", 1, 1.0, t0);
        pet.say(Bubble::text("hi"), true, t0);
        pet.clear_notification();
        assert!(pet.bubble.is_some());
        pet.show_notification(Bubble::text("n"), 5, false, t0);
        pet.clear_notification();
        assert!(pet.bubble.is_none());
    }

    #[test]
    fn small_movement_is_a_click() {
        let mut g = Gesture::default();
        g.press((100, 100), (10, 10));
        assert_eq!(g.motion((102, 101)), GestureEvent::None);
        assert_eq!(g.release(), GestureEvent::Click);
    }

    #[test]
    fn large_movement_drags_the_window() {
        let mut g = Gesture::default();
        g.press((100, 100), (10, 10));
        assert_eq!(g.motion((120, 90)), GestureEvent::MoveWindow(30, 0));
        // Once dragging, even small moves follow the pointer.
        assert_eq!(g.motion((121, 90)), GestureEvent::MoveWindow(31, 0));
        assert_eq!(g.release(), GestureEvent::None);
    }

    #[test]
    fn motion_without_press_does_nothing() {
        let mut g = Gesture::default();
        assert_eq!(g.motion((5, 5)), GestureEvent::None);
        assert_eq!(g.release(), GestureEvent::None);
    }
}
