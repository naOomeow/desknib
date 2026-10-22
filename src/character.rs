//! Character packs: a `character.json` manifest plus one PNG strip per animation.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// Every pack must provide this animation; missing ones fall back to it.
pub const FALLBACK_ANIMATION: &str = "idle";

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Point in frame pixels (before scaling) that the bubble tail aims at.
    #[serde(default)]
    pub bubble_anchor: Option<[f32; 2]>,
    pub animations: BTreeMap<String, AnimationSpec>,
}

fn default_scale() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct AnimationSpec {
    pub file: String,
    pub frames: u32,
    pub fps: f32,
}

#[derive(Debug)]
pub struct Animation {
    pub frames: Vec<Pixmap>,
    pub fps: f32,
}

#[derive(Debug)]
pub struct Character {
    pub name: String,
    pub author: Option<String>,
    pub license: Option<String>,
    pub frame_width: u32,
    pub frame_height: u32,
    pub bubble_anchor: (f32, f32),
    animations: BTreeMap<String, Animation>,
}

#[derive(Debug)]
pub enum LoadError {
    Io(String, std::io::Error),
    Manifest(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(file, e) => write!(f, "cannot read {file}: {e}"),
            LoadError::Manifest(e) => write!(f, "bad character.json: {e}"),
            LoadError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for LoadError {}

const NIB_MANIFEST: &[u8] = include_bytes!("../characters/nib/character.json");
const NIB_FILES: &[(&str, &[u8])] = &[
    ("idle.png", include_bytes!("../characters/nib/idle.png")),
    ("sit.png", include_bytes!("../characters/nib/sit.png")),
    ("sleep.png", include_bytes!("../characters/nib/sleep.png")),
    ("dance.png", include_bytes!("../characters/nib/dance.png")),
];

impl Character {
    /// The built-in default character, compiled into the binary.
    pub fn nib() -> Character {
        Character::parse(NIB_MANIFEST, |name| {
            NIB_FILES
                .iter()
                .find(|(file, _)| *file == name)
                .map(|(_, bytes)| bytes.to_vec())
                .ok_or_else(|| LoadError::Io(name.to_string(), std::io::ErrorKind::NotFound.into()))
        })
        .expect("built-in Nib character is valid")
    }

    pub fn load_dir(dir: &Path) -> Result<Character, LoadError> {
        let manifest_path = dir.join("character.json");
        let manifest = std::fs::read(&manifest_path)
            .map_err(|e| LoadError::Io(manifest_path.display().to_string(), e))?;
        Character::parse(&manifest, |name| {
            // Keep packs from reaching outside their own folder.
            if name.contains('/') || name.contains('\\') || name.contains("..") {
                return Err(LoadError::Invalid(format!(
                    "file name {name:?} must be a plain file name"
                )));
            }
            let path = dir.join(name);
            std::fs::read(&path).map_err(|e| LoadError::Io(path.display().to_string(), e))
        })
    }

    pub fn parse(
        manifest: &[u8],
        read_file: impl Fn(&str) -> Result<Vec<u8>, LoadError>,
    ) -> Result<Character, LoadError> {
        let manifest: Manifest = serde_json::from_slice(manifest).map_err(LoadError::Manifest)?;
        if !manifest.animations.contains_key(FALLBACK_ANIMATION) {
            return Err(LoadError::Invalid(format!(
                "character {:?} has no {FALLBACK_ANIMATION:?} animation",
                manifest.name
            )));
        }
        if !(0.1..=4.0).contains(&manifest.scale) {
            return Err(LoadError::Invalid(format!(
                "scale {} is outside 0.1 to 4.0",
                manifest.scale
            )));
        }

        let mut animations = BTreeMap::new();
        let mut frame_size = None;
        for (anim_name, spec) in &manifest.animations {
            if spec.frames == 0 || !(spec.fps > 0.0 && spec.fps <= 60.0) {
                return Err(LoadError::Invalid(format!(
                    "animation {anim_name:?} needs at least 1 frame and an fps between 0 and 60"
                )));
            }
            let png = read_file(&spec.file)?;
            let strip = Pixmap::decode_png(&png)
                .map_err(|e| LoadError::Invalid(format!("{}: {e}", spec.file)))?;
            if strip.width() % spec.frames != 0 {
                return Err(LoadError::Invalid(format!(
                    "{} is {} px wide, which does not split into {} equal frames",
                    spec.file,
                    strip.width(),
                    spec.frames
                )));
            }
            let size = (strip.width() / spec.frames, strip.height());
            match frame_size {
                None => frame_size = Some(size),
                Some(expected) if expected != size => {
                    return Err(LoadError::Invalid(format!(
                        "{} has {}x{} frames but other animations use {}x{}",
                        spec.file, size.0, size.1, expected.0, expected.1
                    )));
                }
                Some(_) => {}
            }
            let frames = split_strip(&strip, spec.frames, manifest.scale)?;
            animations.insert(
                anim_name.clone(),
                Animation {
                    frames,
                    fps: spec.fps,
                },
            );
        }

        let (w, h) = frame_size.expect("idle animation exists");
        let scale = manifest.scale;
        let anchor = manifest.bubble_anchor.unwrap_or([w as f32 / 2.0, 0.0]);
        Ok(Character {
            name: manifest.name,
            author: manifest.author,
            license: manifest.license,
            frame_width: scaled(w, scale),
            frame_height: scaled(h, scale),
            bubble_anchor: (anchor[0] * scale, anchor[1] * scale),
            animations,
        })
    }

    /// The named animation, or idle when the pack does not have it.
    pub fn animation(&self, name: &str) -> &Animation {
        self.animations
            .get(name)
            .or_else(|| self.animations.get(FALLBACK_ANIMATION))
            .expect("idle animation exists")
    }

    #[allow(dead_code)] // behaviour states arrive in Milestone 2
    pub fn has_animation(&self, name: &str) -> bool {
        self.animations.contains_key(name)
    }
}

fn scaled(px: u32, scale: f32) -> u32 {
    ((px as f32 * scale).round() as u32).max(1)
}

fn split_strip(strip: &Pixmap, count: u32, scale: f32) -> Result<Vec<Pixmap>, LoadError> {
    let fw = strip.width() / count;
    let fh = strip.height();
    let (out_w, out_h) = (scaled(fw, scale), scaled(fh, scale));
    let quality = if (scale - 1.0).abs() < f32::EPSILON {
        FilterQuality::Nearest
    } else {
        FilterQuality::Bicubic
    };
    (0..count)
        .map(|i| {
            let mut frame = Pixmap::new(out_w, out_h)
                .ok_or_else(|| LoadError::Invalid("frame size is zero".into()))?;
            let paint = PixmapPaint {
                quality,
                ..PixmapPaint::default()
            };
            // Shift the strip left so frame i lands at the origin, then scale.
            let transform =
                Transform::from_scale(scale, scale).pre_translate(-((i * fw) as f32), 0.0);
            frame.draw_pixmap(0, 0, strip.as_ref(), &paint, transform, None);
            Ok(frame)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_png(width: u32, height: u32) -> Vec<u8> {
        let mut pm = Pixmap::new(width, height).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        pm.encode_png().unwrap()
    }

    #[test]
    fn built_in_nib_loads_with_all_animations() {
        let nib = Character::nib();
        assert_eq!(nib.name, "Nib");
        assert_eq!((nib.frame_width, nib.frame_height), (160, 184));
        for anim in ["idle", "sit", "sleep", "dance"] {
            assert!(nib.has_animation(anim), "missing {anim}");
        }
        assert_eq!(nib.animation("idle").frames.len(), 8);
    }

    #[test]
    fn missing_animation_falls_back_to_idle() {
        let manifest =
            br#"{"name":"Box","animations":{"idle":{"file":"idle.png","frames":2,"fps":4}}}"#;
        let png = strip_png(20, 10);
        let c = Character::parse(manifest, |_| Ok(png.clone())).unwrap();
        assert_eq!(c.animation("dance").frames.len(), 2);
        assert_eq!((c.frame_width, c.frame_height), (10, 10));
        assert_eq!(c.bubble_anchor, (5.0, 0.0));
    }

    #[test]
    fn scale_applies_to_frames_and_anchor() {
        let manifest = br#"{"name":"Box","scale":2.0,"bubble_anchor":[3,4],
            "animations":{"idle":{"file":"idle.png","frames":1,"fps":4}}}"#;
        let png = strip_png(10, 10);
        let c = Character::parse(manifest, |_| Ok(png.clone())).unwrap();
        assert_eq!((c.frame_width, c.frame_height), (20, 20));
        assert_eq!(c.animation("idle").frames[0].width(), 20);
        assert_eq!(c.bubble_anchor, (6.0, 8.0));
    }

    #[test]
    fn rejects_pack_without_idle() {
        let manifest =
            br#"{"name":"Box","animations":{"dance":{"file":"d.png","frames":1,"fps":4}}}"#;
        let err = Character::parse(manifest, |_| Ok(strip_png(10, 10))).unwrap_err();
        assert!(err.to_string().contains("idle"));
    }

    #[test]
    fn rejects_strip_that_does_not_split_evenly() {
        let manifest =
            br#"{"name":"Box","animations":{"idle":{"file":"i.png","frames":3,"fps":4}}}"#;
        let err = Character::parse(manifest, |_| Ok(strip_png(10, 10))).unwrap_err();
        assert!(err.to_string().contains("equal frames"));
    }

    #[test]
    fn rejects_mismatched_frame_sizes() {
        let manifest = br#"{"name":"Box","animations":{
            "idle":{"file":"a.png","frames":1,"fps":4},
            "sit":{"file":"b.png","frames":1,"fps":4}}}"#;
        let err = Character::parse(manifest, |name| {
            Ok(if name == "a.png" {
                strip_png(10, 10)
            } else {
                strip_png(12, 10)
            })
        })
        .unwrap_err();
        assert!(err.to_string().contains("other animations"));
    }

    #[test]
    fn load_dir_refuses_paths_outside_the_pack() {
        let dir = std::env::temp_dir().join(format!("desknib-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("character.json"),
            br#"{"name":"Evil","animations":{"idle":{"file":"../x.png","frames":1,"fps":4}}}"#,
        )
        .unwrap();
        let err = Character::load_dir(&dir).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(err.to_string().contains("plain file name"));
    }
}
