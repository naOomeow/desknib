//! Composes the pet window: bubble on top, character at the bottom.

use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::bubble::{self, Bubble, Fonts, RenderedBubble};
use crate::character::Character;

/// Height reserved above the character's anchor for a bubble.
const BUBBLE_AREA: u32 = 4 * 17 + 2 * 9 + bubble::TAIL_HEIGHT as u32 + 8;
const MIN_WIDTH: u32 = 240;
/// Pixels with less alpha than this let clicks fall through to the desktop.
const HIT_ALPHA: u8 = 24;

pub struct Scene {
    pub width: u32,
    pub height: u32,
    sprite_x: i32,
    sprite_y: i32,
    anchor: (f32, f32),
    canvas: Pixmap,
    fonts: Fonts,
    bubble: Option<(Bubble, RenderedBubble)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
}

impl Scene {
    pub fn new(character: &Character) -> Scene {
        let width = MIN_WIDTH.max(character.frame_width);
        let anchor_y = character.bubble_anchor.1.max(0.0) as u32;
        let height = BUBBLE_AREA + character.frame_height.saturating_sub(anchor_y);
        let sprite_x = ((width - character.frame_width) / 2) as i32;
        let sprite_y = (height - character.frame_height) as i32;
        Scene {
            width,
            height,
            sprite_x,
            sprite_y,
            anchor: (
                sprite_x as f32 + character.bubble_anchor.0,
                sprite_y as f32 + character.bubble_anchor.1,
            ),
            canvas: Pixmap::new(width, height).expect("scene size is non-zero"),
            fonts: Fonts::load(),
            bubble: None,
        }
    }

    pub fn canvas(&self) -> &Pixmap {
        &self.canvas
    }

    /// Draws one frame. The bubble is only re-rasterised when its text changes.
    pub fn draw(&mut self, frame: &Pixmap, bubble: Option<&Bubble>) {
        match bubble {
            Some(b) if self.bubble.as_ref().is_some_and(|(cur, _)| cur == b) => {}
            Some(b) => {
                let max_w = self.width as f32 - 4.0;
                let offset = self.anchor.0 - self.width as f32 / 2.0;
                let rendered = bubble::render(&self.fonts, b, max_w, offset);
                self.bubble = Some((b.clone(), rendered));
            }
            None => self.bubble = None,
        }

        self.canvas.fill(tiny_skia::Color::TRANSPARENT);
        let paint = PixmapPaint::default();
        self.canvas.draw_pixmap(
            self.sprite_x,
            self.sprite_y,
            frame.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
        if let Some((_, r)) = &self.bubble {
            let x = (self.anchor.0 - r.tail_x).round() as i32;
            let x = x.clamp(0, (self.width - r.pixmap.width()) as i32);
            let y = (self.anchor.1 - r.pixmap.height() as f32).round().max(0.0) as i32;
            self.canvas
                .draw_pixmap(x, y, r.pixmap.as_ref(), &paint, Transform::identity(), None);
        }
    }
}

/// Premultiplied RGBA (tiny-skia) to premultiplied BGRA, the byte order of a
/// 32-bit ARGB X11 visual and of Windows layered windows on little-endian.
pub fn to_bgra(canvas: &Pixmap, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(canvas.data().len());
    for px in canvas.data().as_chunks::<4>().0 {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
}

/// Rectangles covering every pixel solid enough to be clicked. Rows with
/// identical runs are merged so a round sprite becomes a few dozen rects.
pub fn hit_region(canvas: &Pixmap) -> Vec<Rect> {
    let (w, h) = (canvas.width() as usize, canvas.height() as usize);
    let pixels = canvas.pixels();
    let mut rects: Vec<Rect> = Vec::new();
    let mut prev_runs: Vec<(usize, usize)> = Vec::new();
    // Index in `rects` of each run in the previous row, for extending downward.
    let mut prev_ids: Vec<usize> = Vec::new();

    for y in 0..h {
        let row = &pixels[y * w..(y + 1) * w];
        let mut runs = Vec::new();
        let mut x = 0;
        while x < w {
            if row[x].alpha() >= HIT_ALPHA {
                let start = x;
                while x < w && row[x].alpha() >= HIT_ALPHA {
                    x += 1;
                }
                runs.push((start, x));
            } else {
                x += 1;
            }
        }
        let ids = if runs == prev_runs {
            for &id in &prev_ids {
                rects[id].h += 1;
            }
            prev_ids.clone()
        } else {
            runs.iter()
                .map(|&(a, b)| {
                    rects.push(Rect {
                        x: a as i16,
                        y: y as i16,
                        w: (b - a) as u16,
                        h: 1,
                    });
                    rects.len() - 1
                })
                .collect()
        };
        prev_runs = runs;
        prev_ids = ids;
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, Paint, Rect as SkRect};

    fn filled(w: u32, h: u32, rect: (f32, f32, f32, f32)) -> Pixmap {
        let mut pm = Pixmap::new(w, h).unwrap();
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(10, 20, 30, 255));
        pm.fill_rect(
            SkRect::from_xywh(rect.0, rect.1, rect.2, rect.3).unwrap(),
            &paint,
            Transform::identity(),
            None,
        );
        pm
    }

    #[test]
    fn solid_block_becomes_one_rect() {
        let pm = filled(20, 20, (2.0, 3.0, 5.0, 4.0));
        assert_eq!(
            hit_region(&pm),
            vec![Rect {
                x: 2,
                y: 3,
                w: 5,
                h: 4
            }]
        );
    }

    #[test]
    fn empty_canvas_has_no_hit_region() {
        assert!(hit_region(&Pixmap::new(8, 8).unwrap()).is_empty());
    }

    #[test]
    fn bgra_swaps_red_and_blue() {
        let pm = filled(1, 1, (0.0, 0.0, 1.0, 1.0));
        let mut out = Vec::new();
        to_bgra(&pm, &mut out);
        assert_eq!(out, vec![30, 20, 10, 255]);
    }

    #[test]
    fn scene_fits_nib_and_bubble_stays_inside() {
        let nib = Character::nib();
        let mut scene = Scene::new(&nib);
        assert!(scene.width >= nib.frame_width && scene.height > nib.frame_height);
        let frame = &nib.animation("idle").frames[0];
        scene.draw(frame, Some(&Bubble::text("Hi! I'm Nib. ".repeat(12))));
        let region = hit_region(scene.canvas());
        // The bubble occupies the top rows, the character the bottom ones.
        assert!(region.iter().any(|r| r.y < 20));
        assert!(region.iter().any(|r| r.y as u32 > scene.height - 40));
        scene.draw(frame, None);
        assert!(hit_region(scene.canvas()).iter().all(|r| r.y > 20));
    }
}
