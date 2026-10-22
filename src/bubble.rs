//! Chat bubbles: word wrapping, layout and drawing.

use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _, point};
use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

const REGULAR: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
const BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");

pub const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 17.0;
const PADDING_X: f32 = 12.0;
const PADDING_Y: f32 = 9.0;
const RADIUS: f32 = 14.0;
pub const TAIL_HEIGHT: f32 = 10.0;
const MAX_LINES: usize = 4;
const BORDER: f32 = 2.0;

const FILL: (u8, u8, u8) = (255, 255, 255);
const OUTLINE: (u8, u8, u8) = (59, 58, 92);
const TEXT: (u8, u8, u8) = (43, 42, 76);
const TITLE: (u8, u8, u8) = (58, 120, 220);

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgba8(r, g, b, 255)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bubble {
    /// Optional first line in the accent colour, e.g. the app that sent a notification.
    pub title: Option<String>,
    pub body: String,
}

impl Bubble {
    pub fn text(body: impl Into<String>) -> Bubble {
        Bubble {
            title: None,
            body: body.into(),
        }
    }

    /// How long the bubble stays up: longer text gets more reading time.
    pub fn duration_ms(&self) -> u64 {
        let chars =
            self.body.chars().count() + self.title.as_ref().map_or(0, |t| t.chars().count());
        (2500 + 55 * chars as u64).clamp(3000, 9000)
    }
}

pub struct Fonts {
    regular: FontRef<'static>,
    bold: FontRef<'static>,
}

type Font = FontRef<'static>;

impl Fonts {
    /// Glyphs are parsed lazily, so only characters actually shown cost memory.
    pub fn load() -> Fonts {
        Fonts {
            regular: FontRef::try_from_slice(REGULAR).expect("bundled regular font"),
            bold: FontRef::try_from_slice(BOLD).expect("bundled bold font"),
        }
    }

    fn width(font: &Font, text: &str) -> f32 {
        let f = font.as_scaled(px_scale(font));
        text.chars().map(|c| f.h_advance(f.glyph_id(c))).sum()
    }
}

/// ab_glyph scales by line height; convert so FONT_SIZE is the em size.
fn px_scale(font: &Font) -> PxScale {
    let em = font.units_per_em().unwrap_or(2048.0);
    PxScale::from(FONT_SIZE * font.height_unscaled() / em)
}

/// Greedy word wrap. Long words are broken by character; overflow past
/// `max_lines` ends the last line with an ellipsis.
pub fn wrap(
    text: &str,
    max_width: f32,
    max_lines: usize,
    measure: impl Fn(&str) -> f32,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut truncated = false;

    let words: Vec<&str> = text.split_whitespace().collect();
    'words: for word in words {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= max_width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                truncated = true;
                break 'words;
            }
        }
        // Word on its own line; split it if it still does not fit.
        for c in word.chars() {
            let mut next = current.clone();
            next.push(c);
            if measure(&next) > max_width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                if lines.len() == max_lines {
                    truncated = true;
                    break 'words;
                }
                current.push(c);
            } else {
                current = next;
            }
        }
    }
    if !current.is_empty() {
        if lines.len() == max_lines {
            truncated = true;
        } else {
            lines.push(current);
        }
    }
    if truncated && let Some(last) = lines.last_mut() {
        while !last.is_empty() && measure(&format!("{last}…")) > max_width {
            last.pop();
        }
        let trimmed = last.trim_end().to_string();
        *last = format!("{trimmed}…");
    }
    lines
}

/// A bubble drawn once into its own pixmap, ready to composite every frame.
pub struct RenderedBubble {
    pub pixmap: Pixmap,
    /// X offset of the tail tip inside the pixmap.
    pub tail_x: f32,
}

/// Draws the bubble no wider than `max_width`, with the tail tip at `tail_x`
/// relative to the bubble's centre being clamped inside the rounded corners.
pub fn render(fonts: &Fonts, bubble: &Bubble, max_width: f32, tail_offset: f32) -> RenderedBubble {
    let text_max = max_width - 2.0 * PADDING_X;
    let mut lines: Vec<(bool, String)> = Vec::new();
    if let Some(title) = &bubble.title {
        for l in wrap(title, text_max, 1, |s| Fonts::width(&fonts.bold, s)) {
            lines.push((true, l));
        }
    }
    let body_lines = MAX_LINES - lines.len();
    for l in wrap(&bubble.body, text_max, body_lines, |s| {
        Fonts::width(&fonts.regular, s)
    }) {
        lines.push((false, l));
    }
    if lines.is_empty() {
        lines.push((false, String::new()));
    }

    let text_w = lines
        .iter()
        .map(|(bold, l)| Fonts::width(if *bold { &fonts.bold } else { &fonts.regular }, l))
        .fold(0.0f32, f32::max);
    let w = (text_w + 2.0 * PADDING_X).clamp(60.0, max_width).ceil();
    let body_h = (lines.len() as f32 * LINE_HEIGHT + 2.0 * PADDING_Y).ceil();
    let h = body_h + TAIL_HEIGHT + BORDER;
    let mut pixmap = Pixmap::new(w as u32 + 2, h as u32 + 2).expect("bubble size is non-zero");

    let tail_x = (w / 2.0 + tail_offset).clamp(RADIUS + 8.0, w - RADIUS - 8.0) + 1.0;
    let path = bubble_path(1.0, 1.0, w, body_h, tail_x);
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(rgb(FILL));
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    paint.set_color(rgb(OUTLINE));
    let stroke = Stroke {
        width: BORDER,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);

    let mut y = 1.0 + PADDING_Y;
    for (bold, line) in &lines {
        let (font, color) = if *bold {
            (&fonts.bold, TITLE)
        } else {
            (&fonts.regular, TEXT)
        };
        draw_text(&mut pixmap, font, line, 1.0 + PADDING_X, y, rgb(color));
        y += LINE_HEIGHT;
    }
    RenderedBubble { pixmap, tail_x }
}

fn bubble_path(x: f32, y: f32, w: f32, h: f32, tail_x: f32) -> tiny_skia::Path {
    let r = RADIUS;
    let k = 0.5523 * r; // cubic approximation of a quarter circle
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(tail_x + 7.0, y + h);
    pb.line_to(tail_x, y + h + TAIL_HEIGHT);
    pb.line_to(tail_x - 7.0, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("bubble path is valid")
}

/// Rasterises `text` with its top-left at (x, y) and source-over blends it.
fn draw_text(pixmap: &mut Pixmap, font: &Font, text: &str, x: f32, y: f32, color: Color) {
    let f = font.as_scaled(px_scale(font));
    let baseline = y + f.ascent() + (LINE_HEIGHT - f.height()) / 2.0;
    let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
    let c = color.to_color_u8();
    let pixels = pixmap.pixels_mut();
    let mut pen = x;
    for ch in text.chars() {
        let id = f.glyph_id(ch);
        let glyph = id.with_scale_and_position(f.scale(), point(pen, baseline));
        pen += f.h_advance(id);
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            let a = (coverage.clamp(0.0, 1.0) * 255.0).round() as u32;
            if a == 0 || px < 0 || py < 0 || px >= pw || py >= ph {
                return;
            }
            let dst = &mut pixels[(py * pw + px) as usize];
            let inv = 255 - a;
            let blend = |s: u8, d: u8| ((s as u32 * a + d as u32 * inv + 127) / 255) as u8;
            if let Some(p) = PremultipliedColorU8::from_rgba(
                blend(c.red(), dst.red()),
                blend(c.green(), dst.green()),
                blend(c.blue(), dst.blue()),
                blend(255, dst.alpha()),
            ) {
                *dst = p;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One unit per character keeps wrap tests independent of the font.
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32
    }

    #[test]
    fn wraps_on_word_boundaries() {
        assert_eq!(
            wrap("the quick brown fox", 10.0, 4, chars),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn splits_words_longer_than_a_line() {
        assert_eq!(
            wrap("abcdefghijkl", 5.0, 4, chars),
            vec!["abcde", "fghij", "kl"]
        );
    }

    #[test]
    fn truncates_with_ellipsis_after_max_lines() {
        let lines = wrap("one two three four five six", 9.0, 2, chars);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert!(chars(&lines[1]) <= 9.0);
    }

    #[test]
    fn empty_text_gives_no_lines() {
        assert!(wrap("   ", 10.0, 4, chars).is_empty());
    }

    #[test]
    fn duration_grows_with_length_within_bounds() {
        assert_eq!(Bubble::text("hi").duration_ms(), 3000);
        assert_eq!(Bubble::text("x".repeat(500)).duration_ms(), 9000);
        let mid = Bubble::text("x".repeat(80)).duration_ms();
        assert!(mid > 3000 && mid < 9000);
    }

    #[test]
    fn rendered_bubble_respects_max_width_and_has_ink() {
        let fonts = Fonts::load();
        let b = Bubble {
            title: Some("Discord".into()),
            body: "sam: you up for tonight? ".repeat(10),
        };
        let r = render(&fonts, &b, 220.0, 0.0);
        assert!(r.pixmap.width() <= 222);
        let opaque = r
            .pixmap
            .pixels()
            .iter()
            .filter(|p| p.alpha() == 255)
            .count();
        assert!(opaque > 1000);
        // Text pixels are darker than the white fill.
        assert!(
            r.pixmap
                .pixels()
                .iter()
                .any(|p| p.alpha() == 255 && p.red() < 120)
        );
    }
}
