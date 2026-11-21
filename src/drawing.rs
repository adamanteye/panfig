use kurbo::{Affine, BezPath, Rect};
use serde::Serialize;
use std::collections::HashMap;
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Default)]
pub struct Figure {
    pub items: Vec<Placed>,
    pub viewport: Option<Rect>,
}

pub struct Placed {
    pub item: Item,
    pub transform: Affine,
    pub angle: f64,
    pub clips: Vec<Rc<Clip>>,
}

pub struct Clip {
    pub path: BezPath,
    pub rule: FillRule,
    pub rectangle: Option<Rect>,
}

impl PartialEq for Clip {
    fn eq(&self, other: &Self) -> bool {
        self.rule == other.rule && self.path.elements() == other.path.elements()
    }
}

/// absolute Cartesian coordinates in printer's points (1/72 inch), y up.
pub type Point = [f64; 2];
pub type Color = [u8; 3];
pub const CM: f64 = 72.0 / 2.54;
pub const TEX_PT: f64 = 72.0 / 72.27;
pub const BLACK: Color = [0; 3];

#[derive(Debug, PartialEq)]
pub enum Item {
    Path {
        path: Rc<BezPath>,
        style: Style,
    },
    Bounds(Rect),
    Anchor {
        name: String,
        at: Point,
    },
    Text {
        at: Position,
        anchor: String,
        runs: Vec<Run>,
        opacity: f64,
        font: Option<String>,
        size: Option<f64>,
    },
    Image {
        at: Position,
        anchor: String,
        file: String,
        width: Option<f64>,
        height: Option<f64>,
    },
}

/// resolve label positions in PGF drawing order.
#[derive(Debug, PartialEq)]
pub enum Position {
    Absolute(Point),
    Relative { to: String, offset: Point },
}

impl Position {
    pub fn resolve(&self, anchors: &HashMap<String, Point>) -> Result<Point> {
        match self {
            Self::Absolute(p) => Ok(*p),
            Self::Relative { to, offset } => {
                let p = anchors
                    .get(to.as_str())
                    .ok_or_else(|| format!("unknown anchor {to:?}"))?;
                Ok([p[0] + offset[0], p[1] + offset[1]])
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub stroke: Option<Stroke>,
    pub fill: Option<Color>,
    pub fill_opacity: f64,
    pub fill_rule: FillRule,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub color: Color,
    pub width: f64,
    pub opacity: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter: f64,
    pub dash: Option<Dash>,
}
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Dash {
    pub array: Vec<f64>,
    pub phase: f64,
}
impl Stroke {
    pub fn black(width: f64) -> Self {
        Self {
            width,
            color: BLACK,
            opacity: 1.,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 10.,
            dash: None,
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Run {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<Color>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub math: Option<String>,
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

pub fn push_run(runs: &mut Vec<Run>, text: &str, bold: bool, italic: bool) {
    if text.is_empty() {
        return;
    }
    if let Some(prev) = runs.last_mut() {
        if prev.fill.is_none()
            && prev.math.is_none()
            && (prev.bold, prev.italic) == (bold, italic)
        {
            prev.text.push_str(text);
            return;
        }
    }
    runs.push(Run {
        fill: None,
        math: None,
        text: text.into(),
        bold,
        italic,
    });
}

pub fn push_math(runs: &mut Vec<Run>, math: String) {
    if math.is_empty() {
        return;
    }
    runs.push(Run {
        fill: None,
        math: Some(math),
        text: String::new(),
        bold: false,
        italic: false,
    });
}

pub const ANCHORS: &[&str] = &[
    "center",
    "west",
    "east",
    "north",
    "south",
    "north-west",
    "north-east",
    "south-west",
    "south-east",
    "base",
    "base-west",
    "base-east",
];

pub fn hex(rgb: Color) -> String {
    format!("{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

pub fn parse_hex(value: &str) -> Result<Color> {
    if value.len() == 6 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        let n = u32::from_str_radix(value, 16).unwrap();
        return Ok([(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    Err(format!("expected six RGB hex digits, got {value:?}"))
}

pub fn validate(figure: &Figure) -> Result<()> {
    let point = |p: &Point| p.iter().all(|n| n.is_finite());
    let mut anchors = HashMap::new();
    for (i, placed) in figure.items.iter().enumerate() {
        let ok = match &placed.item {
            Item::Bounds(rect) => rect.is_finite(),
            Item::Path { path, style } => {
                !path.is_empty()
                    && path.is_finite()
                    && (0.0..=1.).contains(&style.fill_opacity)
                    && style.stroke.as_ref().is_none_or(|s| {
                        s.width.is_finite()
                            && s.width >= 0.
                            && (0.0..=1.).contains(&s.opacity)
                            && s.dash.as_ref().is_none_or(|d| {
                                d.phase.is_finite()
                                    && d.array.iter().all(|v| v.is_finite() && *v >= 0.)
                                    && d.array.iter().any(|v| *v > 0.)
                            })
                    })
            }
            Item::Anchor { name, at } => {
                if anchors.insert(name.clone(), *at).is_some() {
                    return Err(format!("duplicate anchor {name:?}"));
                }
                !name.is_empty()
                    && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                    && point(at)
            }
            Item::Image { at, anchor, file, width, height, .. } => {
                point(&at.resolve(&anchors)?)
                    && ANCHORS.contains(&anchor.as_str())
                    && !file.is_empty()
                    && !file.chars().any(|c| c.is_control() || "{}%\\".contains(c))
                    && width.iter().chain(height).all(|n| n.is_finite() && *n > 0.)
            }
            Item::Text { at, anchor, runs, font, size, .. } => {
                point(&at.resolve(&anchors)?)
                    && ANCHORS.contains(&anchor.as_str())
                    && size.is_none_or(|n| n.is_finite() && n > 0.)
                    && font.as_ref().is_none_or(|s| {
                        !s.is_empty()
                            && !s.contains(['{', '}', '%', '\\'])
                            && !s.chars().any(char::is_control)
                    })
                    && runs.iter().all(|r| {
                        !r.text.chars().any(char::is_control)
                            && r.math
                                .as_ref()
                                .is_none_or(|m| !m.chars().any(char::is_control))
                    })
            }
        };
        if !ok || !placed.transform.is_finite() || !placed.angle.is_finite() {
            return Err(format!(
                "invalid geometry, style or label in PGF item {}",
                i + 1
            ));
        }
    }
    Ok(())
}
