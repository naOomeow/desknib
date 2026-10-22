mod bubble;
mod character;
mod inbox;
mod notify;
mod pet;
mod scene;
mod tray;
mod x11;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Instant;

use bubble::Bubble;
use character::Character;
use inbox::Inbox;
use notify::{CloseReason, Notifier};
use pet::{Action, Gesture, GestureEvent, Pet};
use scene::Scene;
use x11::{PetWindow, WindowEvent};

const USAGE: &str = "\
desknib: a quiet desktop pet

Usage: desknib [options]

Options:
  --character <dir>  load a character pack folder (contains character.json)
  --quiet            start in quiet mode (no unprompted bubbles)
  -V, --version      print the version
  -h, --help         print this help

Left-click the pet to say hi, drag to move it, right-click twice to quit.";

struct Options {
    character: Option<PathBuf>,
    quiet: bool,
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut opts = Options {
        character: None,
        quiet: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--character" => {
                let dir = args.next().ok_or("--character needs a folder")?;
                opts.character = Some(PathBuf::from(dir));
            }
            "--quiet" => opts.quiet = true,
            "-V" | "--version" => {
                println!("desknib {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            other => return Err(format!("unknown option {other:?}\n\n{USAGE}")),
        }
    }
    Ok(Some(opts))
}

fn main() -> ExitCode {
    let opts = match parse_args() {
        Ok(Some(opts)) => opts,
        Ok(None) => return ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };
    match run(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("desknib: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(opts: Options) -> x11::Result<()> {
    let character = match &opts.character {
        Some(dir) => {
            let c = Character::load_dir(dir)?;
            eprintln!(
                "desknib: loaded {} by {} ({})",
                c.name,
                c.author.as_deref().unwrap_or("unknown author"),
                c.license.as_deref().unwrap_or("no license given")
            );
            c
        }
        None => Character::nib(),
    };
    let mut scene = Scene::new(&character);
    let mut window = PetWindow::open(scene.width, scene.height)?;

    let (note_tx, note_rx) = mpsc::channel();
    let (mode, notifier) = match notify::start(note_tx) {
        Ok((mode, notifier)) => (Some(mode), notifier),
        Err(e) => {
            eprintln!("desknib: notifications unavailable ({e})");
            (None, Notifier::none())
        }
    };
    if mode == Some(notify::Mode::Listening) {
        eprintln!(
            "desknib: the desktop keeps its own notification service; turn its popups off to avoid seeing both"
        );
    }
    let mut alerts = Alerts {
        inbox: Inbox::default(),
        notifier,
    };

    let (tx, rx) = mpsc::channel();
    let idle = character.animation("idle");
    let tray = match tray::spawn(tx, &idle.frames[0], opts.quiet, mode) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("desknib: no tray icon ({e}); right-click the pet twice to quit");
            None
        }
    };

    let now = Instant::now();
    let mut pet = Pet::new("idle", idle.frames.len(), idle.fps, now);
    pet.quiet = opts.quiet;
    pet.say(
        Bubble::text(format!("Hi, I'm {}! Drag me anywhere.", character.name)),
        false,
        now,
    );
    let mut gesture = Gesture::default();
    let mut dirty = true;
    let mut visible = true;
    let mut synced = (pet.quiet, !visible);

    loop {
        let now = Instant::now();
        dirty |= pet.tick(now);
        if let Some(id) = pet.take_expired_notification() {
            alerts.finish(id, CloseReason::Expired, &mut pet, now);
            dirty = true;
        }

        while let Ok(event) = note_rx.try_recv() {
            let holding = pet.quiet || !visible;
            dirty |= alerts.receive(event, holding, &mut pet, now);
        }

        while let Ok(cmd) = rx.try_recv() {
            let was_holding = pet.quiet || !visible;
            match cmd {
                tray::Command::SayHi => pet.on_click(now),
                tray::Command::SetQuiet(q) => pet.quiet = q,
                tray::Command::SetHidden(h) => {
                    visible = !h;
                    window.set_visible(visible)?;
                }
                tray::Command::Quit => return Ok(()),
            }
            if was_holding && !(pet.quiet || !visible) {
                alerts.catch_up(&mut pet, now);
            }
            dirty = true;
        }

        if dirty && visible {
            let anim = character.animation(&pet.animation);
            let frame = &anim.frames[pet.frame % anim.frames.len()];
            scene.draw(frame, pet.bubble.as_ref().map(|b| &b.bubble));
            window.present(scene.canvas())?;
            dirty = false;
        }

        // While hidden, only the tray can wake us, so poll it about once a second.
        let deadline = if visible {
            pet.next_deadline(now)
        } else {
            now + std::time::Duration::from_secs(1)
        };
        for event in window.wait_events(deadline)? {
            let now = Instant::now();
            match event {
                WindowEvent::Press { button: 1, root } => gesture.press(root, window.position),
                WindowEvent::Press { button: 3, .. } => {
                    if pet.on_secondary_click(now) == Action::Quit {
                        return Ok(());
                    }
                    dirty = true;
                }
                WindowEvent::Motion { root } => {
                    if let GestureEvent::MoveWindow(x, y) = gesture.motion(root) {
                        window.move_to(x, y)?;
                    }
                }
                WindowEvent::Release { button: 1 } => {
                    if gesture.release() == GestureEvent::Click {
                        match pet.notification_on_screen() {
                            Some(id) => alerts.clicked(id, &mut pet, now),
                            None => pet.on_click(now),
                        }
                        dirty = true;
                    }
                }
                WindowEvent::Exposed => dirty = true,
                _ => {}
            }
        }
        if let Some(t) = &tray {
            // Keep the tray's checkmarks in sync with changes made elsewhere.
            if synced != (pet.quiet, !visible) {
                synced = (pet.quiet, !visible);
                t.set_quiet(pet.quiet);
                t.set_hidden(!visible);
            }
        }
    }
}

/// Desktop notifications on their way to bubbles.
struct Alerts {
    inbox: Inbox,
    notifier: Notifier,
}

impl Alerts {
    /// Returns true when the bubble on screen changed.
    fn receive(
        &mut self,
        event: notify::Event,
        holding: bool,
        pet: &mut Pet,
        now: Instant,
    ) -> bool {
        match event {
            notify::Event::Show(n) => {
                let outcome = self.inbox.push(n, holding);
                for id in outcome.discarded {
                    self.notifier.closed(id, CloseReason::Expired);
                }
                if outcome.redisplay {
                    self.show_current(pet, now);
                }
                outcome.redisplay
            }
            notify::Event::Close(id) => {
                let on_screen = self.inbox.close(id);
                self.notifier.closed(id, CloseReason::Closed);
                if on_screen {
                    self.show_current(pet, now);
                }
                on_screen
            }
        }
    }

    /// A click on a notification bubble opens it when the sender allows,
    /// and dismisses it either way.
    fn clicked(&mut self, id: u32, pet: &mut Pet, now: Instant) {
        if self
            .inbox
            .current
            .as_ref()
            .is_some_and(|n| n.id == id && n.has_default_action)
        {
            self.notifier.invoke_default(id);
        }
        self.finish(id, CloseReason::Dismissed, pet, now);
    }

    fn finish(&mut self, id: u32, reason: CloseReason, pet: &mut Pet, now: Instant) {
        if self.inbox.current.as_ref().is_some_and(|n| n.id == id) {
            self.inbox.advance();
        }
        self.notifier.closed(id, reason);
        self.show_current(pet, now);
    }

    fn show_current(&mut self, pet: &mut Pet, now: Instant) {
        let critical = self.inbox.current.as_ref().is_some_and(|n| n.critical);
        match (
            self.inbox.current.as_ref().map(|n| n.id),
            self.inbox.current_bubble(),
        ) {
            (Some(id), Some(bubble)) => pet.show_notification(bubble, id, critical, now),
            _ => pet.clear_notification(),
        }
    }

    /// After quiet mode or hiding ends, sum up what was held back.
    fn catch_up(&mut self, pet: &mut Pet, now: Instant) {
        if let Some((bubble, ids)) = self.inbox.take_missed() {
            for id in ids {
                self.notifier.closed(id, CloseReason::Expired);
            }
            if self.inbox.current.is_none() {
                pet.say(bubble, true, now);
            }
        }
    }
}
