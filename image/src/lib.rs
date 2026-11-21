//! read the physical dimensions of external graphics for both vector renderers.

/// identify an image format supported by both drawing backends.
pub fn mime(name: &str) -> Option<&'static str> {
    match name.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// return intrinsic width and height in physical points.
pub fn image_dimensions(bytes: &[u8], mime: &str) -> Result<(f64, f64), String> {
    let (w, h) = dimensions(bytes, mime)?;
    if !(w.is_finite() && h.is_finite() && w > 0. && h > 0.) {
        return Err(format!("invalid intrinsic dimensions for {mime}: {w} × {h}"));
    }
    Ok((w, h))
}

fn dimensions(bytes: &[u8], mime: &str) -> Result<(f64, f64), String> {
    if mime == "image/svg+xml" {
        let src = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let tree = roxmltree::Document::parse(src).map_err(|e| e.to_string())?;
        let svg = tree.root_element();
        let len = |s: &str| -> Option<f64> {
            for (unit, k) in [
                ("pt", 1.),
                ("px", 0.75),
                ("in", 72.),
                ("cm", 72. / 2.54),
                ("mm", 72. / 25.4),
                ("", 0.75),
            ] {
                if let Some(n) = s.strip_suffix(unit).and_then(|n| n.parse::<f64>().ok())
                {
                    return Some(n * k);
                }
            }
            None
        };
        if let (Some(w), Some(h)) =
            (svg.attribute("width").and_then(len), svg.attribute("height").and_then(len))
        {
            return Ok((w, h));
        }
        let bounds: Vec<f64> = svg
            .attribute("viewBox")
            .unwrap_or("")
            .split(|c: char| c.is_ascii_whitespace() || c == ',')
            .filter_map(|s| s.parse().ok())
            .collect();
        if let [_, _, w, h] = bounds[..] {
            return Ok((w * 0.75, h * 0.75));
        }
        return Err("SVG image needs a width and height or viewBox".into());
    }
    let s = imagesize::blob_size(bytes).map_err(|e| format!("invalid {mime}: {e}"))?;
    let mut dpi = (72., 72.);
    if mime == "image/png" {
        let mut chunk = bytes.get(8..).unwrap_or_default();
        while chunk.len() >= 12 {
            let n = u32::from_be_bytes(chunk[..4].try_into().unwrap()) as usize;
            let Some(next) = n.checked_add(12).and_then(|end| chunk.get(end..)) else {
                break;
            };
            if &chunk[4..8] == b"pHYs" && n == 9 && chunk[16] == 1 {
                let density = |off| {
                    u32::from_be_bytes(chunk[off..off + 4].try_into().unwrap()) as f64
                        * 0.0254
                };
                if density(8) > 0. && density(12) > 0. {
                    dpi = (density(8), density(12))
                }
                break;
            }
            chunk = next;
        }
    }
    Ok((s.width as f64 * 72. / dpi.0, s.height as f64 * 72. / dpi.1))
}
