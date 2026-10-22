mod bubble;
mod character;
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

    let (tx, rx) = mpsc::channel();
    let idle = character.animation("idle");
    let tray = match tray::spawn(tx, &idle.frames[0], opts.quiet) {
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

    loop {
        let now = Instant::now();
        dirty |= pet.tick(now);

        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                tray::Command::SayHi => {
                    pet.on_click(now);
                    dirty = true;
                }
                tray::Command::SetQuiet(q) => pet.quiet = q,
                tray::Command::SetHidden(h) => {
                    visible = !h;
                    window.set_visible(visible)?;
                    dirty = true;
                }
                tray::Command::Quit => return Ok(()),
            }
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
                        pet.on_click(now);
                        dirty = true;
                    }
                }
                WindowEvent::Exposed => dirty = true,
                _ => {}
            }
        }
        if let Some(t) = &tray {
            // Keep the tray's checkmarks in sync with changes made elsewhere.
            t.set_quiet(pet.quiet);
            t.set_hidden(!visible);
        }
    }
}
