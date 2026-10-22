//! X11 window: 32-bit ARGB, undecorated, always on top, click-through outside
//! the visible pixels. Works on X11 desktops and under XWayland.

use std::borrow::Cow;
use std::error::Error;
use std::os::fd::AsRawFd;
use std::time::Instant;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::image::{BitsPerPixel, Image, ImageOrder, ScanlinePad};
use x11rb::protocol::Event;
use x11rb::protocol::screensaver;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::scene::{self, Rect};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub enum WindowEvent {
    /// Pointer button pressed or released, with pointer position on screen.
    Press {
        button: u8,
        root: (i32, i32),
    },
    Release {
        button: u8,
    },
    Motion {
        root: (i32, i32),
    },
    Exposed,
}

pub struct PetWindow {
    conn: RustConnection,
    window: Window,
    root: Window,
    gc: Gcontext,
    width: u16,
    height: u16,
    pub position: (i32, i32),
    last_region: Vec<Rect>,
    has_shape: bool,
    has_screensaver: bool,
    pixels: Vec<u8>,
    mapped: bool,
}

struct Atoms {
    net_wm_state: Atom,
    above: Atom,
    skip_taskbar: Atom,
    skip_pager: Atom,
    sticky: Atom,
    window_type: Atom,
    utility: Atom,
    motif_hints: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
    workarea: Atom,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Atoms> {
        let names: [&[u8]; 11] = [
            b"_NET_WM_STATE",
            b"_NET_WM_STATE_ABOVE",
            b"_NET_WM_STATE_SKIP_TASKBAR",
            b"_NET_WM_STATE_SKIP_PAGER",
            b"_NET_WM_STATE_STICKY",
            b"_NET_WM_WINDOW_TYPE",
            b"_NET_WM_WINDOW_TYPE_UTILITY",
            b"_MOTIF_WM_HINTS",
            b"_NET_WM_NAME",
            b"UTF8_STRING",
            b"_NET_WORKAREA",
        ];
        // Send every request before waiting on any reply.
        let cookies = names
            .iter()
            .map(|n| conn.intern_atom(false, n))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut atoms = cookies
            .into_iter()
            .map(|c| Ok(c.reply()?.atom))
            .collect::<Result<Vec<Atom>>>()?
            .into_iter();
        let mut next = || atoms.next().expect("one atom per name");
        Ok(Atoms {
            net_wm_state: next(),
            above: next(),
            skip_taskbar: next(),
            skip_pager: next(),
            sticky: next(),
            window_type: next(),
            utility: next(),
            motif_hints: next(),
            net_wm_name: next(),
            utf8_string: next(),
            workarea: next(),
        })
    }
}

impl PetWindow {
    pub fn open(width: u32, height: u32) -> Result<PetWindow> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = conn.setup().roots[screen_num].clone();
        let atoms = Atoms::intern(&conn)?;

        let visual = screen
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .ok_or("the X server has no 32-bit TrueColor visual, so transparency is unavailable")?
            .visual_id;

        let (width, height) = (width as u16, height as u16);
        let position = initial_position(&conn, &screen, &atoms, width, height);

        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual)?;
        let window = conn.generate_id()?;
        conn.create_window(
            32,
            window,
            screen.root,
            position.0 as i16,
            position.1 as i16,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .colormap(colormap)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::POINTER_MOTION
                        | EventMask::STRUCTURE_NOTIFY,
                ),
        )?;

        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"Desknib",
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            atoms.net_wm_name,
            atoms.utf8_string,
            b"Desknib",
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"desknib\0Desknib\0",
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms.net_wm_state,
            AtomEnum::ATOM,
            &[
                atoms.above,
                atoms.skip_taskbar,
                atoms.skip_pager,
                atoms.sticky,
            ],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms.window_type,
            AtomEnum::ATOM,
            &[atoms.utility],
        )?;
        // Motif hints: flags = decorations, decorations = none.
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms.motif_hints,
            atoms.motif_hints,
            &[2, 0, 0, 0, 0],
        )?;
        // WM_HINTS with InputHint = false: never steal keyboard focus.
        conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_HINTS,
            AtomEnum::WM_HINTS,
            &[1, 0, 0, 0, 0, 0, 0, 0, 0],
        )?;
        // WM_NORMAL_HINTS with a user-specified position so the WM keeps our placement.
        let mut size_hints = [0u32; 18];
        size_hints[0] = 1 | 4 | 16 | 32; // USPosition | PPosition | PMinSize | PMaxSize
        size_hints[1] = position.0 as u32;
        size_hints[2] = position.1 as u32;
        size_hints[5] = width as u32;
        size_hints[6] = height as u32;
        size_hints[7] = width as u32;
        size_hints[8] = height as u32;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NORMAL_HINTS,
            AtomEnum::WM_SIZE_HINTS,
            &size_hints,
        )?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        let has_shape = conn
            .extension_information(shape::X11_EXTENSION_NAME)?
            .is_some();
        let has_screensaver = conn
            .extension_information(screensaver::X11_EXTENSION_NAME)?
            .is_some();

        let mut win = PetWindow {
            conn,
            window,
            root: screen.root,
            gc,
            width,
            height,
            position,
            last_region: Vec::new(),
            has_shape,
            has_screensaver,
            pixels: Vec::new(),
            mapped: false,
        };
        // Start with an empty input region so the window never blocks clicks
        // before the first frame is drawn.
        win.set_input_region(&[])?;
        win.set_visible(true)?;
        Ok(win)
    }

    pub fn set_visible(&mut self, visible: bool) -> Result<()> {
        if visible == self.mapped {
            return Ok(());
        }
        if visible {
            self.conn.map_window(self.window)?;
            // Some WMs move a window on map; restate where it belongs.
            self.move_to(self.position.0, self.position.1)?;
        } else {
            self.conn.unmap_window(self.window)?;
        }
        self.mapped = visible;
        self.conn.flush()?;
        Ok(())
    }

    pub fn move_to(&mut self, x: i32, y: i32) -> Result<()> {
        self.position = (x, y);
        self.conn
            .configure_window(self.window, &ConfigureWindowAux::new().x(x).y(y))?;
        Ok(())
    }

    /// How long since the last keyboard or mouse input anywhere on the
    /// desktop, via the MIT-SCREEN-SAVER extension. `None` when that
    /// extension is not available (rare; some minimal X servers lack it).
    pub fn idle_time(&self) -> Result<Option<std::time::Duration>> {
        if !self.has_screensaver {
            return Ok(None);
        }
        let info = screensaver::query_info(&self.conn, self.root)?.reply()?;
        Ok(Some(std::time::Duration::from_millis(
            info.ms_since_user_input.into(),
        )))
    }

    /// Uploads the composed scene and updates the clickable area.
    pub fn present(&mut self, canvas: &tiny_skia::Pixmap) -> Result<()> {
        scene::to_bgra(canvas, &mut self.pixels);
        let image = Image::new(
            self.width,
            self.height,
            ScanlinePad::Pad32,
            32,
            BitsPerPixel::B32,
            ImageOrder::LsbFirst,
            Cow::Borrowed(&self.pixels),
        )?;
        image.put(&self.conn, self.window, self.gc, 0, 0)?;
        let region = scene::hit_region(canvas);
        if region != self.last_region {
            self.set_input_region(&region)?;
            self.last_region = region;
        }
        self.conn.flush()?;
        Ok(())
    }

    fn set_input_region(&self, region: &[Rect]) -> Result<()> {
        if !self.has_shape {
            return Ok(());
        }
        let rects: Vec<Rectangle> = region
            .iter()
            .map(|r| Rectangle {
                x: r.x,
                y: r.y,
                width: r.w,
                height: r.h,
            })
            .collect();
        self.conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            ClipOrdering::UNSORTED,
            self.window,
            0,
            0,
            &rects,
        )?;
        Ok(())
    }

    /// Returns queued events, waiting until `deadline` if there are none.
    pub fn wait_events(&mut self, deadline: Instant) -> Result<Vec<WindowEvent>> {
        let mut out = Vec::new();
        self.drain(&mut out)?;
        if out.is_empty() {
            self.conn.flush()?;
            let timeout = deadline.saturating_duration_since(Instant::now());
            let mut fds = libc::pollfd {
                fd: self.conn.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
            // SAFETY: one valid pollfd for the duration of the call.
            let rc = unsafe { libc::poll(&mut fds, 1, ms) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::Interrupted {
                    return Err(err.into());
                }
            }
            self.drain(&mut out)?;
        }
        Ok(out)
    }

    fn drain(&mut self, out: &mut Vec<WindowEvent>) -> Result<()> {
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::ButtonPress(e) => out.push(WindowEvent::Press {
                    button: e.detail,
                    root: (e.root_x as i32, e.root_y as i32),
                }),
                Event::ButtonRelease(e) => out.push(WindowEvent::Release { button: e.detail }),
                Event::MotionNotify(e) => {
                    let root = (e.root_x as i32, e.root_y as i32);
                    // Collapse bursts of motion into the latest position.
                    if let Some(WindowEvent::Motion { root: last }) = out.last_mut() {
                        *last = root;
                    } else {
                        out.push(WindowEvent::Motion { root });
                    }
                }
                Event::Expose(e) if e.count == 0 => out.push(WindowEvent::Exposed),
                Event::ConfigureNotify(e) if e.window == self.window => {
                    // Track where the WM actually placed us (only synthetic
                    // events carry root coordinates for reparented windows).
                    if e.response_type & 0x80 != 0 {
                        self.position = (e.x as i32, e.y as i32);
                    }
                }
                Event::MapNotify(_) => out.push(WindowEvent::Exposed),
                _ => {}
            }
        }
        Ok(())
    }
}

/// Bottom-right corner of the work area (screen minus panels).
fn initial_position(
    conn: &RustConnection,
    screen: &Screen,
    atoms: &Atoms,
    w: u16,
    h: u16,
) -> (i32, i32) {
    let (mut ax, mut ay, mut aw, mut ah) = (
        0,
        0,
        screen.width_in_pixels as i32,
        screen.height_in_pixels as i32,
    );
    if let Ok(cookie) =
        conn.get_property(false, screen.root, atoms.workarea, AtomEnum::CARDINAL, 0, 4)
        && let Ok(reply) = cookie.reply()
        && let Some(v) = reply.value32()
    {
        let v: Vec<u32> = v.collect();
        if v.len() == 4 && v[2] > 0 && v[3] > 0 {
            (ax, ay, aw, ah) = (v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32);
        }
    }
    (ax + aw - w as i32 - 24, ay + ah - h as i32 - 8)
}
