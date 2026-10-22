//! Tray icon over the StatusNotifierItem D-Bus protocol (Cinnamon, KDE, XFCE,
//! and GNOME with the AppIndicator extension).

use std::sync::mpsc::Sender;

use ksni::blocking::TrayMethods;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    SayHi,
    SetQuiet(bool),
    SetHidden(bool),
    Quit,
}

struct NibTray {
    tx: Sender<Command>,
    quiet: bool,
    hidden: bool,
    icons: Vec<ksni::Icon>,
}

impl ksni::Tray for NibTray {
    fn id(&self) -> String {
        "desknib".into()
    }

    fn title(&self) -> String {
        "Desknib".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Desknib".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Left-clicking the tray icon brings a hidden pet back.
        if self.hidden {
            self.hidden = false;
            let _ = self.tx.send(Command::SetHidden(false));
        } else {
            let _ = self.tx.send(Command::SayHi);
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "Say hi".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(Command::SayHi);
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Quiet mode".into(),
                checked: self.quiet,
                activate: Box::new(|t: &mut Self| {
                    t.quiet = !t.quiet;
                    let _ = t.tx.send(Command::SetQuiet(t.quiet));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Hide pet".into(),
                checked: self.hidden,
                activate: Box::new(|t: &mut Self| {
                    t.hidden = !t.hidden;
                    let _ = t.tx.send(Command::SetHidden(t.hidden));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.tx.send(Command::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct TrayHandle {
    handle: ksni::blocking::Handle<NibTray>,
}

impl TrayHandle {
    pub fn set_quiet(&self, quiet: bool) {
        self.handle.update(|t| t.quiet = quiet);
    }

    pub fn set_hidden(&self, hidden: bool) {
        self.handle.update(|t| t.hidden = hidden);
    }
}

/// Starts the tray on its own thread. Fails when no tray host is running;
/// the pet still works and can be quit with a double right-click.
pub fn spawn(tx: Sender<Command>, icon_source: &Pixmap, quiet: bool) -> Result<TrayHandle, String> {
    let icons = [22u32, 32, 48]
        .iter()
        .map(|&s| icon(icon_source, s))
        .collect();
    let tray = NibTray {
        tx,
        quiet,
        hidden: false,
        icons,
    };
    tray.spawn()
        .map(|handle| TrayHandle { handle })
        .map_err(|e| e.to_string())
}

/// Square ARGB32 (network byte order, straight alpha) icon cropped to the sprite.
fn icon(source: &Pixmap, size: u32) -> ksni::Icon {
    let side = source.width().max(source.height()) as f32;
    let scale = size as f32 / side;
    let mut pm = Pixmap::new(size, size).expect("icon size is non-zero");
    let dx = (side - source.width() as f32) / 2.0;
    let dy = (side - source.height() as f32) / 2.0;
    let paint = PixmapPaint {
        quality: tiny_skia::FilterQuality::Bicubic,
        ..Default::default()
    };
    pm.draw_pixmap(
        0,
        0,
        source.as_ref(),
        &paint,
        Transform::from_scale(scale, scale).pre_translate(dx, dy),
        None,
    );
    let data = pm
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.alpha(), c.red(), c.green(), c.blue()]
        })
        .collect();
    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_argb_of_requested_size() {
        let mut src = Pixmap::new(10, 20).unwrap();
        src.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let i = icon(&src, 32);
        assert_eq!((i.width, i.height, i.data.len()), (32, 32, 32 * 32 * 4));
        // Centre pixel is opaque red in ARGB order; the side margins are empty.
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(&i.data[centre..centre + 4], &[255, 255, 0, 0]);
        assert_eq!(i.data[(16 * 32) * 4], 0);
    }
}
