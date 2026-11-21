//! prepare native curves and batch dense runs of paths without crossing text or images.
use super::drawing::*;
use kurbo::{Affine, BezPath, Circle, PathEl, Rect, Shape};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Write;

pub const NATIVE_PATHS: usize = 128;
pub const NATIVE_SEGMENTS: usize = 1024;

fn path(contours: &[Contour]) -> BezPath {
    let mut p = BezPath::new();
    let point = |[x, y]: Point| (x, y);
    for c in contours {
        match c {
            Contour::Rectangle { a, b } => {
                p.move_to(point(*a));
                p.line_to((b[0], a[1]));
                p.line_to(point(*b));
                p.line_to((a[0], b[1]));
                p.close_path();
            }
            Contour::Circle { center, radius } => {
                p.extend(Circle::new(point(*center), *radius).path_elements(0.001))
            }
            Contour::Curve { start, segments, closed } => {
                p.move_to(point(*start));
                for s in segments {
                    match s {
                        Segment::Line { to } => p.line_to(point(*to)),
                        Segment::Quadratic { control, to } => {
                            p.quad_to(point(*control), point(*to))
                        }
                        Segment::Cubic { first, second, to } => {
                            p.curve_to(point(*first), point(*second), point(*to))
                        }
                    }
                }
                if *closed {
                    p.close_path()
                }
            }
        }
    }
    Affine::FLIP_Y * p
}

fn paint_bounds(p: &BezPath, style: &Style) -> Rect {
    let mut bounds = p.bounding_box();
    if let Some(s) = &style.stroke {
        let cap = match s.cap {
            LineCap::Butt => kurbo::Cap::Butt,
            LineCap::Round => kurbo::Cap::Round,
            LineCap::Square => kurbo::Cap::Square,
        };
        let join = match s.join {
            LineJoin::Miter => kurbo::Join::Miter,
            LineJoin::Round => kurbo::Join::Round,
            LineJoin::Bevel => kurbo::Join::Bevel,
        };
        bounds = bounds.inflate(s.width / 2., s.width / 2.);
        if s.width > 0. {
            let pen = kurbo::Stroke::new(s.width)
                .with_join(join)
                .with_caps(cap)
                .with_miter_limit(s.miter);
            let outline = kurbo::stroke(
                p.elements().iter().copied(),
                &pen,
                &Default::default(),
                0.001,
            );
            if !outline.is_empty() {
                bounds = bounds.union(outline.bounding_box());
            }
        }
    }
    bounds
}

pub fn native(p: &BezPath, style: &Style) -> Value {
    let b = paint_bounds(p, style);
    let xy = |pt: kurbo::Point| [pt.x - b.x0, pt.y - b.y0];
    let components: Vec<_> = p
        .elements()
        .iter()
        .map(|el| match *el {
            PathEl::MoveTo(a) => json!(["move", xy(a)]),
            PathEl::LineTo(a) => json!(["line", xy(a)]),
            PathEl::QuadTo(a, b) => json!(["quad", xy(a), xy(b)]),
            PathEl::CurveTo(a, b, c) => json!(["cubic", xy(a), xy(b), xy(c)]),
            PathEl::ClosePath => json!(["close"]),
        })
        .collect();
    json!({"kind":"curve","at":[b.x0,-b.y0],"size":[b.width(),b.height()],"style":style,"components":components})
}

fn svg_path(svg: &mut String, p: &BezPath, style: &Style) {
    write!(svg, r#"<path d="{}" "#, p.to_svg()).unwrap();
    let fill = style
        .fill
        .map_or_else(|| "none".into(), |c| format!("#{}", super::drawing::hex(c)));
    let rule = if style.fill_rule == FillRule::EvenOdd { "evenodd" } else { "nonzero" };
    write!(
        svg,
        r#"fill="{fill}" fill-opacity="{}" fill-rule="{rule}""#,
        style.fill_opacity
    )
    .unwrap();
    if let Some(s) = &style.stroke {
        use super::drawing::{LineCap, LineJoin};
        let cap_name = match s.cap {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
            LineCap::Square => "square",
        };
        let join_name = match s.join {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
            LineJoin::Bevel => "bevel",
        };
        write!(svg, r##" stroke="#{}" stroke-width="{}" stroke-opacity="{}" stroke-linecap="{cap_name}" stroke-linejoin="{join_name}" stroke-miterlimit="10""##,super::drawing::hex(s.color),s.width,s.opacity).unwrap();
        if let Some(d) = &s.dash {
            let lengths =
                d.array.iter().map(f64::to_string).collect::<Vec<_>>().join(" ");
            write!(
                svg,
                r#" stroke-dasharray="{lengths}" stroke-dashoffset="{}""#,
                d.phase
            )
            .unwrap();
        }
    }
    svg.push_str("/>");
}

fn flush(paths: &mut Vec<(BezPath, &Style)>, layers: &mut Vec<Value>) {
    if paths.is_empty() {
        return;
    }
    if paths.len() <= NATIVE_PATHS
        && paths.iter().map(|(p, _)| p.elements().len()).sum::<usize>() <= NATIVE_SEGMENTS
    {
        layers.extend(paths.drain(..).map(|(p, s)| native(&p, s)));
    } else {
        let mut body = String::new();
        let mut bounds: Option<Rect> = None;
        for (p, s) in paths.drain(..) {
            let b = paint_bounds(&p, s);
            bounds = Some(bounds.map_or(b, |prev| prev.union(b)));
            svg_path(&mut body, &p, s);
        }
        let b = bounds.unwrap();
        let (x, y, w, h) = (b.x0, b.y0, b.width().max(0.001), b.height().max(0.001));
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}pt" height="{h}pt" viewBox="{x} {y} {w} {h}">{body}</svg>"#
        );
        layers.push(json!({"kind":"vector","at":[x,-y],"size":[w,h],"svg":svg}));
    }
}

pub fn layers(items: &[Item]) -> Result<Vec<Value>> {
    let mut layers = Vec::new();
    let mut paths = Vec::new();
    let mut anchors = HashMap::new();
    for item in items {
        match item {
            Item::Path { contours, style } => paths.push((path(contours), style)),
            Item::Anchor { name, at } => {
                anchors.insert(name.clone(), *at);
            }
            Item::Image { at, anchor, file, width, height } => {
                flush(&mut paths, &mut layers);
                layers.push(json!({"kind":"image","at":at.resolve(&anchors)?,"anchor":anchor,"file":file,"width":width,"height":height}));
            }
            Item::Text { at, anchor, runs, font, size } => {
                flush(&mut paths, &mut layers);
                layers.push(json!({"kind":"text","at":at.resolve(&anchors)?,"anchor":anchor,"runs":runs,"font":font,"size":size}));
            }
        }
    }
    flush(&mut paths, &mut layers);
    Ok(layers)
}
