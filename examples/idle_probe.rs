//! Throwaway helper for testing: prints whether MIT-SCREEN-SAVER is
//! available and the reported idle time every second, to confirm idle
//! detection actually works against a given X server (Xvfb sometimes lacks
//! extensions a real desktop has).
use std::thread;
use std::time::Duration;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::screensaver;

fn main() {
    let (conn, screen_num) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen_num].root;
    let has_ext = conn
        .extension_information(screensaver::X11_EXTENSION_NAME)
        .unwrap()
        .is_some();
    println!("MIT-SCREEN-SAVER available: {has_ext}");
    if !has_ext {
        return;
    }
    loop {
        let info = screensaver::query_info(&conn, root)
            .unwrap()
            .reply()
            .unwrap();
        println!("idle: {} ms", info.ms_since_user_input);
        thread::sleep(Duration::from_secs(1));
    }
}
