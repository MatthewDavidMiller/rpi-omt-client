//! The decode ceiling a board may attempt: a frame is admitted when it fits
//! inside any one shape. This is policy, not safety -- the protocol still
//! refuses anything above 1920x1080@60, which is what sizes allocations.

use std::fmt::Write;

use omt_common::Error;

pub const CEILING_MAX_SHAPES: usize = 4;
const CEILING_MAX_WIDTH: i32 = 1920;
const CEILING_MAX_HEIGHT: i32 = 1080;
const CEILING_MAX_FPS: i32 = 60;
const CEILING_MIN_DIMENSION: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub width: i32,
    pub height: i32,
    pub fps: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoCeiling {
    shapes: Vec<Shape>,
}

/// Digits only, so a leading '+', a leading zero, or surrounding space is a
/// rejection rather than something a lenient parser would quietly accept.
fn number(s: &str) -> Option<i32> {
    if s.is_empty() || s.len() > 4 || s.starts_with('0') {
        return None;
    }
    omt_common::buf::parse_u64(s.as_bytes(), 9999).and_then(|v| i32::try_from(v).ok())
}

fn shape(field: &str) -> Result<Shape, Error> {
    let parsed = field.split_once('@').and_then(|(size, fps)| {
        let (w, h) = size.split_once('x')?;
        Some(Shape { width: number(w)?, height: number(h)?, fps: number(fps)? })
    });
    let Some(s) = parsed else {
        return Err(Error::new(format!(
            "Invalid video ceiling: {field}. Expected WIDTHxHEIGHT@FPS."
        )));
    };
    if !(CEILING_MIN_DIMENSION..=CEILING_MAX_WIDTH).contains(&s.width)
        || !(CEILING_MIN_DIMENSION..=CEILING_MAX_HEIGHT).contains(&s.height)
        || !(1..=CEILING_MAX_FPS).contains(&s.fps)
    {
        return Err(Error::new(format!(
            "Video ceiling {field} is outside the supported \
             {CEILING_MAX_WIDTH}x{CEILING_MAX_HEIGHT} at {CEILING_MAX_FPS} fps maximum."
        )));
    }
    Ok(s)
}

impl VideoCeiling {
    /// Parses `WIDTHxHEIGHT@FPS[,...]`. The error is the operator-facing
    /// reason.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut shapes = Vec::new();
        for field in text.split(',') {
            if shapes.len() == CEILING_MAX_SHAPES {
                return Err(Error::new(format!(
                    "A video ceiling may list at most {CEILING_MAX_SHAPES} resolutions."
                )));
            }
            shapes.push(shape(field)?);
        }
        Ok(Self { shapes })
    }

    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    /// "1920x1080 at 30 fps, or 1280x720 at 60 fps".
    pub fn describe(&self) -> String {
        let mut out = String::new();
        for (i, s) in self.shapes.iter().enumerate() {
            if i > 0 {
                out.push_str(", or ");
            }
            let _ = write!(out, "{}x{} at {} fps", s.width, s.height, s.fps);
        }
        out
    }

    /// Whether the format fits; when not, the error names the stream and the
    /// ceiling. The rate is compared with a tolerance because 59.94 arrives
    /// as 60000/1001 and must not be refused by a 60 fps ceiling.
    pub fn admits(&self, width: i32, height: i32, rate: f64) -> Result<(), Error> {
        if self
            .shapes
            .iter()
            .any(|s| width <= s.width && height <= s.height && rate <= f64::from(s.fps) + 0.01)
        {
            return Ok(());
        }
        Err(Error::new(format!(
            "{width}x{height} at {rate:.2} fps exceeds this appliance's limit of {}.",
            self.describe()
        )))
    }
}
