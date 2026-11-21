//! lower a parsed PGF figure into paint-ordered SVG, native text and image layers.
use super::drawing::*;
use kurbo::{Affine, BezPath, Rect, Shape};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;

fn paint_bounds(p: &BezPath, style: &Style, transform: Affine) -> Rect {
    let bounds_of = |path: &BezPath| {
        if transform == Affine::IDENTITY {
            path.bounding_box()
        } else {
            (transform * path).bounding_box()
        }
    };
    let mut bounds = bounds_of(p);
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
        if transform == Affine::IDENTITY {
            bounds = bounds.inflate(s.width / 2., s.width / 2.);
        }
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
                bounds = bounds.union(bounds_of(&outline));
            }
        }
    }
    bounds
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
        let cap = match s.cap {
            LineCap::Butt => "butt",
            LineCap::Round => "round",
            LineCap::Square => "square",
        };
        let join = match s.join {
            LineJoin::Miter => "miter",
            LineJoin::Round => "round",
            LineJoin::Bevel => "bevel",
        };
        write!(
            svg,
            r##" stroke="#{}" stroke-width="{}" stroke-opacity="{}" stroke-linecap="{cap}" stroke-linejoin="{join}" stroke-miterlimit="{}""##,
            super::drawing::hex(s.color),
            s.width,
            s.opacity,
            s.miter,
        )
        .unwrap();
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

fn frame(rect: Rect) -> [f64; 4] {
    [rect.x0, -rect.y1, rect.width(), rect.height()]
}

fn clip_bounds(clips: &[Rc<Clip>], viewport: Option<Rect>) -> Option<Rect> {
    clips.iter().fold(viewport, |bounds, clip| {
        let b = clip.rectangle.unwrap_or_else(|| clip.path.bounding_box());
        Some(bounds.map_or(b, |r| r.intersect(b)))
    })
}

fn wrap_svg(rect: [f64; 4], defs: &[Rc<Clip>], body: &str, xlink: bool) -> String {
    let [x, y, w, h] = rect;
    let ns = if xlink { r#" xmlns:xlink="http://www.w3.org/1999/xlink""# } else { "" };
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg"{ns} width="{w}pt" height="{h}pt" viewBox="{x} {y} {w} {h}">"#
    );
    if !defs.is_empty() {
        svg.push_str("<defs>");
        for (i, clip) in defs.iter().enumerate() {
            let path = Affine::FLIP_Y * &clip.path;
            let rule = if clip.rule == FillRule::EvenOdd { "evenodd" } else { "nonzero" };
            write!(
                svg,
                r#"<clipPath id="c{i}"><path d="{}" clip-rule="{rule}"/></clipPath>"#,
                path.to_svg()
            )
            .unwrap();
        }
        svg.push_str("</defs>");
    }
    svg.push_str(body);
    svg.push_str("</svg>");
    svg
}

// coalesce consecutive geometry paths across clip changes into one SVG layer.
#[derive(Default)]
struct Batch<'a> {
    paths: Vec<(&'a BezPath, &'a Style, Affine, &'a [Rc<Clip>])>,
}

impl Batch<'_> {
    fn flush(&mut self, layers: &mut Vec<Value>, viewport: Option<Rect>) {
        if self.paths.is_empty() {
            return;
        }
        let mut defs: Vec<Rc<Clip>> = Vec::new();
        let mut active: Vec<usize> = Vec::new();
        let mut body = String::new();
        let mut bounds: Option<Rect> = None;
        let mut emitted = false;

        for (path, style, t, clips) in self.paths.drain(..) {
            let clipped = clip_bounds(clips, viewport);
            if clipped.is_some_and(|r| r.is_zero_area()) {
                continue;
            }
            if viewport.is_none() {
                let b = clipped.unwrap_or_else(|| paint_bounds(path, style, t));
                bounds = Some(bounds.map_or(b, |r| r.union(b)));
            }
            let target: Vec<usize> = clips
                .iter()
                .map(|clip| {
                    defs.iter()
                        .position(|c| Rc::ptr_eq(c, clip) || c.as_ref() == clip.as_ref())
                        .unwrap_or_else(|| {
                            defs.push(clip.clone());
                            defs.len() - 1
                        })
                })
                .collect();
            let common = active.iter().zip(&target).take_while(|(a, b)| a == b).count();
            for _ in active.drain(common..) {
                body.push_str("</g>");
            }
            for &id in &target[common..] {
                write!(body, r#"<g clip-path="url(#c{id})">"#).unwrap();
                active.push(id);
            }
            // keep the pen in local coordinates under non-uniform scales and shears.
            let p = Affine::FLIP_Y * path;
            if t == Affine::IDENTITY {
                svg_path(&mut body, &p, style);
            } else {
                let [a, b, c, d, e, f] =
                    (Affine::FLIP_Y * t * Affine::FLIP_Y).as_coeffs();
                write!(body, r#"<g transform="matrix({a} {b} {c} {d} {e} {f})">"#)
                    .unwrap();
                svg_path(&mut body, &p, style);
                body.push_str("</g>");
            }
            emitted = true;
        }
        if !emitted {
            return;
        }
        for _ in active.drain(..) {
            body.push_str("</g>");
        }
        let mut rect = viewport.unwrap_or_else(|| bounds.unwrap());
        if viewport.is_none() {
            rect.x1 = rect.x1.max(rect.x0 + 0.001);
            rect.y0 = rect.y0.min(rect.y1 - 0.001);
        }
        let rect = frame(rect);
        let svg = wrap_svg(rect, &defs, &body, false);
        layers.push(json!({"kind": "svg", "rect": rect, "svg": svg}));
    }
}

pub fn layers(figure: &Figure) -> Result<Value> {
    let mut layers = Vec::new();
    let mut batch = Batch::default();
    let mut anchors = HashMap::new();
    for placed in &figure.items {
        let t = placed.transform;
        let xy = |[x, y]: Point| {
            let p = t * kurbo::Point::new(x, y);
            [p.x, p.y]
        };
        match &placed.item {
            Item::Path { path, style } => {
                batch.paths.push((path, style, t, &placed.clips));
            }
            Item::Anchor { name, at } => {
                // system-layer transforms affect painting, after PGF resolves named coordinates.
                anchors.insert(name.clone(), *at);
            }
            Item::Bounds(rect) => {
                layers.push(json!({"kind": "bounds", "rect": frame(*rect)}));
            }
            item => {
                batch.flush(&mut layers, figure.viewport);
                let (at, mut layer) = match item {
                    Item::Image { at, anchor, file, width, height } => (
                        at,
                        json!({"kind": "image", "anchor": anchor, "file": file, "width": width, "height": height}),
                    ),
                    Item::Text { at, anchor, runs, font, size, opacity } => (
                        at,
                        json!({"kind": "text", "anchor": anchor, "runs": runs, "font": font, "size": size, "opacity": opacity}),
                    ),
                    _ => unreachable!(),
                };
                let [x, y] = xy(at.resolve(&anchors)?);
                layer["at"] = json!([x, -y]);
                let orientation = t * Affine::rotate(placed.angle.to_radians());
                let [a, b, c, d, _, _] = orientation.as_coeffs();
                if let Some(rect) = clip_bounds(&placed.clips, None) {
                    if rect.is_zero_area() {
                        continue;
                    }
                    let rect = frame(rect);
                    if placed.clips.iter().all(|clip| clip.rectangle.is_some()) {
                        layer["clip"] = json!(rect);
                    } else if matches!(item, Item::Image { .. }) {
                        let mut body = String::new();
                        for i in 0..placed.clips.len() {
                            write!(body, r#"<g clip-path="url(#c{i})">"#).unwrap();
                        }
                        write!(
                            body,
                            r#"<g transform="matrix({a} {} {} {d} {x} {})"><!--image--></g>"#,
                            -b, -c, -y
                        )
                        .unwrap();
                        for _ in &placed.clips {
                            body.push_str("</g>");
                        }
                        layer["slice"] = json!({"rect": rect, "svg": wrap_svg(rect, &placed.clips, &body, true)});
                        layers.push(layer);
                        continue;
                    } else {
                        return Err("nonrectangular text clip".into());
                    }
                }
                // decompose the screen-space matrix into rotation, horizontal shear and scale.
                let sx = a.hypot(b);
                let sy = orientation.determinant() / sx;
                if sx == 0. || sy == 0. || !sy.is_finite() {
                    return Err("singular text or image transform".into());
                }
                let shear = -(a * c + b * d) / (sx * sy);
                if shear.abs() > 1e-12 {
                    layer["shear"] = json!(shear);
                }
                layer["scale"] = json!([sx, sy]);
                layer["angle"] = json!(-b.atan2(a).to_degrees());
                layers.push(layer);
            }
        }
    }
    batch.flush(&mut layers, figure.viewport);
    let mut out = json!({"layers": layers});
    if let Some(rect) = figure.viewport {
        out["viewport"] = json!(frame(rect));
    }
    Ok(out)
}
