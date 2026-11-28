//! interpret the DVI positioning instructions and the PGF SVG driver's specials.
use dvi::Instruction as Op;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
};

type Result<T> = std::result::Result<T, String>;
const BP: f64 = 72. / 72.27;

pub enum Image {
    Encoded(String),
    Label { content: String, width: f64, height: f64 },
}

#[derive(Clone, Copy)]
enum Encoding {
    Ot1,
    Oml,
    Oms,
    Omx,
    Ascii,
}

#[derive(Clone)]
struct TextRun {
    x: f64,
    y: f64,
    font_id: u32,
    family: String,
    size: f64,
    bold: bool,
    italic: bool,
    color: String,
    text: String,
}

#[derive(Default, Clone, Copy)]
struct Pen {
    h: f64,
    v: f64,
    w: i32,
    x: i32,
    y: i32,
    z: i32,
}

struct Page {
    pen: Pen,
    stack: Vec<Pen>,
    colors: Vec<String>,
    has_picture: bool,
    body: String,
    defs: String,
    used: HashSet<String>,
    size: Option<[f64; 2]>,
    fragments: HashMap<String, Vec<(bool, String)>>,
    recording: Option<String>,
    run: Option<TextRun>,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            pen: Pen::default(),
            stack: vec![],
            colors: vec!["black".into()],
            has_picture: false,
            body: String::new(),
            defs: String::new(),
            used: HashSet::new(),
            size: None,
            fragments: HashMap::new(),
            recording: None,
            run: None,
        }
    }
}

fn color(s: &str) -> Result<String> {
    let mut args = s.split_whitespace();
    let model = args.next().unwrap_or("");
    let vals = args
        .map(str::parse)
        .collect::<std::result::Result<Vec<f64>, _>>()
        .map_err(|e| e.to_string())?;
    let rgb = match (model, vals.as_slice()) {
        ("rgb", [r, g, b]) => [*r, *g, *b],
        ("gray", [g]) => [*g; 3],
        ("cmyk", [c, m, y, k]) => {
            [1. - (c + k).min(1.), 1. - (m + k).min(1.), 1. - (y + k).min(1.)]
        }
        _ => return Err(format!("unsupported DVI color {s:?}")),
    };
    let [r, g, b] = rgb.map(|n| (n.clamp(0., 1.) * 255.).round() as u8);
    Ok(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// read character advance widths (as design-size fractions) from a TFM file.
fn tfm_widths(bytes: &[u8]) -> Option<HashMap<u32, f64>> {
    if bytes.len() < 24 {
        return None;
    }
    let u16_at = |i: usize| u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
    let lh = u16_at(2);
    let bc = u16_at(4);
    let ec = u16_at(6);
    let nw = u16_at(8);
    if bc > ec || nw == 0 {
        return None;
    }
    let ci_off = (6 + lh) * 4;
    let width_off = ci_off + (ec - bc + 1) * 4;
    if bytes.len() < width_off + nw * 4 {
        return None;
    }
    let mut widths = HashMap::with_capacity(ec - bc + 1);
    for c in bc..=ec {
        let wi = bytes[ci_off + (c - bc) * 4] as usize;
        if wi < nw {
            let off = width_off + wi * 4;
            let fix = i32::from_be_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]);
            widths.insert(c as u32, fix as f64 / 1_048_576.0);
        }
    }
    Some(widths)
}

/// map a TeX font name to a host font family, style, encoding, and whether it is known.
fn map_font(name: &str) -> (&str, bool, bool, Encoding, bool) {
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    match stem {
        "cmr" | "cmcsc" => ("New Computer Modern", false, false, Encoding::Ot1, true),
        "cmbx" | "cmb" => ("New Computer Modern", true, false, Encoding::Ot1, true),
        "cmti" | "cmsl" | "cmu" => {
            ("New Computer Modern", false, true, Encoding::Ot1, true)
        }
        "cmbxti" | "cmbxsl" => ("New Computer Modern", true, true, Encoding::Ot1, true),
        "cmss" => ("New Computer Modern Sans", false, false, Encoding::Ot1, true),
        "cmssbx" | "cmssdc" => {
            ("New Computer Modern Sans", true, false, Encoding::Ot1, true)
        }
        "cmssi" => ("New Computer Modern Sans", false, true, Encoding::Ot1, true),
        "cmtt" | "cmvtt" => {
            ("New Computer Modern Mono", false, false, Encoding::Ot1, true)
        }
        "cmitt" | "cmsltt" => {
            ("New Computer Modern Mono", false, true, Encoding::Ot1, true)
        }
        "cmmi" => ("New Computer Modern Math", false, true, Encoding::Oml, true),
        "cmmib" => ("New Computer Modern Math", true, true, Encoding::Oml, true),
        "cmsy" => ("New Computer Modern Math", false, false, Encoding::Oms, true),
        "cmbsy" => ("New Computer Modern Math", true, false, Encoding::Oms, true),
        "cmex" => ("New Computer Modern Math", false, false, Encoding::Omx, true),
        _ => (name, false, false, Encoding::Ascii, false),
    }
}

fn decode_glyph(enc: Encoding, c: u32) -> String {
    match enc {
        Encoding::Ot1 => match c {
            0 => "Γ",
            1 => "Δ",
            2 => "Θ",
            3 => "Λ",
            4 => "Ξ",
            5 => "Π",
            6 => "Σ",
            7 => "Υ",
            8 => "Φ",
            9 => "Ψ",
            10 => "Ω",
            11 => "ff",
            12 => "fi",
            13 => "fl",
            14 => "ffi",
            15 => "ffl",
            16 => "ı",
            17 => "ȷ",
            18 => "`",
            19 => "´",
            20 => "ˇ",
            21 => "˘",
            22 => "¯",
            23 => "˚",
            24 => "¸",
            25 => "ß",
            26 => "æ",
            27 => "œ",
            28 => "ø",
            29 => "Æ",
            30 => "Œ",
            31 => "Ø",
            34 => "”",
            60 => "¡",
            62 => "¿",
            92 => "“",
            123 => "–",
            124 => "—",
            125 => "˝",
            126 => "˜",
            127 => "¨",
            32..=122 => return char::from_u32(c).unwrap_or('?').to_string(),
            _ => return char::from_u32(c).unwrap_or('\u{fffd}').to_string(),
        }
        .into(),
        Encoding::Oml => match c {
            0 => "Γ",
            1 => "Δ",
            2 => "Θ",
            3 => "Λ",
            4 => "Ξ",
            5 => "Π",
            6 => "Σ",
            7 => "Υ",
            8 => "Φ",
            9 => "Ψ",
            10 => "Ω",
            11 => "α",
            12 => "β",
            13 => "γ",
            14 => "δ",
            15 => "ϵ",
            16 => "ζ",
            17 => "η",
            18 => "θ",
            19 => "ι",
            20 => "κ",
            21 => "λ",
            22 => "μ",
            23 => "ν",
            24 => "ξ",
            25 => "π",
            26 => "ρ",
            27 => "σ",
            28 => "τ",
            29 => "υ",
            30 => "ϕ",
            31 => "χ",
            32 => "ψ",
            33 => "ω",
            34 => "ε",
            35 => "ϑ",
            36 => "ϖ",
            37 => "ϱ",
            38 => "ς",
            39 => "φ",
            58 => ".",
            59 => ",",
            60 => "<",
            61 => "/",
            62 => ">",
            63 => "⋆",
            64 => "∂",
            96 => "ℓ",
            123 => "ı",
            124 => "ȷ",
            125 => "℘",
            _ => return char::from_u32(c).unwrap_or('\u{fffd}').to_string(),
        }
        .into(),
        Encoding::Oms => match c {
            0 => "−",
            1 => "·",
            2 => "×",
            3 => "∗",
            4 => "÷",
            5 => "⋄",
            6 => "±",
            7 => "∓",
            8 => "⊕",
            9 => "⊖",
            10 => "⊗",
            11 => "⊘",
            12 => "⊙",
            13 => "○",
            14 => "∘",
            15 => "•",
            17 => "≡",
            18 => "⊆",
            19 => "⊇",
            20 => "≤",
            21 => "≥",
            24 => "∼",
            25 => "≈",
            26 => "⊂",
            27 => "⊃",
            28 => "≪",
            29 => "≫",
            32 => "←",
            33 => "→",
            34 => "↑",
            35 => "↓",
            36 => "↔",
            40 => "⇐",
            41 => "⇒",
            44 => "⇔",
            47 => "∝",
            48 => "′",
            49 => "∞",
            50 => "∈",
            51 => "∋",
            56 => "∀",
            57 => "∃",
            58 => "¬",
            59 => "∅",
            60 => "ℜ",
            61 => "ℑ",
            62 => "⊤",
            63 => "⊥",
            64 => "ℵ",
            91 => "∪",
            92 => "∩",
            94 => "∧",
            95 => "∨",
            98 => "⌊",
            99 => "⌋",
            100 => "⌈",
            101 => "⌉",
            102 => "{",
            103 => "}",
            104 => "⟨",
            105 => "⟩",
            106 => "|",
            107 => "‖",
            110 => "∖",
            114 => "∇",
            121 => "†",
            122 => "‡",
            _ => return char::from_u32(c).unwrap_or('\u{fffd}').to_string(),
        }
        .into(),
        Encoding::Omx => match c {
            0 | 16 | 18 | 32 | 48 => "(",
            1 | 17 | 19 | 33 | 49 => ")",
            2 | 20 | 34 | 50 => "[",
            3 | 21 | 35 | 51 => "]",
            8 | 26 | 40 | 56 => "{",
            9 | 27 | 41 | 57 => "}",
            10 | 28 | 42 | 58 => "⟨",
            11 | 29 | 43 | 59 => "⟩",
            12 | 60 => "|",
            13 | 61 => "‖",
            72 | 73 => "∮",
            80 | 88 => "∑",
            81 | 89 => "∏",
            82 | 90 => "∫",
            83 | 91 => "⋃",
            84 | 92 => "⋂",
            112 | 113 | 114 | 115 | 116 => "√",
            _ => return char::from_u32(c).unwrap_or('\u{fffd}').to_string(),
        }
        .into(),
        Encoding::Ascii => char::from_u32(c).unwrap_or('\u{fffd}').to_string(),
    }
}

impl Page {
    fn flush_text(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        let weight = if run.bold { r#" font-weight="bold""# } else { "" };
        let style = if run.italic { r#" font-style="italic""# } else { "" };
        write!(
            self.body,
            r#"<text x="{}" y="{}" font-family="{}" font-size="{}"{weight}{style} fill="{}" xml:space="preserve">{}</text>"#,
            run.x,
            run.y,
            escape_xml(&run.family),
            run.size,
            run.color,
            escape_xml(&run.text),
        )
        .unwrap();
    }

    fn special(
        &mut self,
        cmd: &str,
        k: f64,
        images: &HashMap<String, Image>,
    ) -> Result<()> {
        self.flush_text();
        let (x, y) = (self.pen.h * k, self.pen.v * k);
        if let Some(raw) = cmd
            .strip_prefix("dvisvgm:raw ")
            .or_else(|| cmd.strip_prefix("dvisvgm:rawdef "))
        {
            let def = cmd.starts_with("dvisvgm:rawdef ");
            if let Some(key) = &self.recording {
                self.fragments
                    .entry(key.clone())
                    .or_default()
                    .push((def, raw.to_string()));
                return Ok(());
            }
            self.has_picture |= raw.contains("<svg beginpicture>");
            let raw = raw
                .replace("<svg beginpicture>", "")
                .replace("</svg endpicture>", "")
                .replace("{?x}", &x.to_string())
                .replace("{?y}", &y.to_string())
                .replace("{?nl}", "\n")
                .replace("{?color}", self.colors.last().unwrap());
            if def {
                self.defs.push_str(&raw)
            } else {
                self.body.push_str(&raw)
            }
        } else if let Some(key) = cmd.strip_prefix("dvisvgm:rawset ") {
            self.recording = Some(key.trim().into())
        } else if cmd.trim() == "dvisvgm:endrawset" {
            self.recording = None
        } else if let Some(key) = cmd.strip_prefix("dvisvgm:rawput ") {
            let key = key.trim();
            let first = self.used.insert(format!("set:{key}"));
            for (def, raw) in self
                .fragments
                .get(key)
                .ok_or_else(|| format!("undefined SVG fragment {key}"))?
                .clone()
            {
                if !def || first {
                    self.special(
                        &format!("dvisvgm:{} {raw}", if def { "rawdef" } else { "raw" }),
                        k,
                        images,
                    )?
                }
            }
        } else if let Some(s) = cmd.strip_prefix("panfig:image ") {
            let mut a = s.splitn(3, ' ');
            let mut dim = || {
                a.next()
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|n| n * k)
                    .ok_or("invalid image dimensions")
            };
            let (w, h) = (dim()?, dim()?);
            let name = a.next().unwrap_or("").trim().trim_matches('"');
            let image = images
                .get(name)
                .ok_or_else(|| format!("missing embedded image {name:?}"))?;
            match image {
                Image::Label{content,width,height} => write!(self.body,r#"<g color="{}" transform="translate({x} {}) scale({} {})">{content}</g>"#,self.colors.last().unwrap(),y-h,w/width,h/height).unwrap(),
                Image::Encoded(url) => write!(self.body,r#"<image x="{x}" y="{}" width="{w}" height="{h}" preserveAspectRatio="none" xlink:href="{url}"/>"#,y-h).unwrap(),
            }
        } else if let Some(s) = cmd.strip_prefix("papersize=") {
            let len = |s: &str| {
                s.trim()
                    .strip_suffix("pt")
                    .and_then(|n| n.parse().ok())
                    .ok_or("invalid DVI paper size")
            };
            let (a, b) = s.split_once(',').ok_or("invalid DVI paper size")?;
            self.size = Some([len(a)?, len(b)?]);
        } else if let Some(c) = cmd.strip_prefix("color push ") {
            self.colors.push(color(c)?)
        } else if cmd.trim() == "color pop" {
            if self.colors.len() > 1 {
                self.colors.pop();
            }
        } else if !(cmd.starts_with("ps::%%HiResBoundingBox:")
            || cmd.starts_with("dvisvgm:bbox "))
        {
            return Err(format!("unsupported DVI special {cmd:?}"));
        }
        Ok(())
    }
}

/// render DVI instructions and PGF SVG specials without bundled font outlines.
pub fn render(
    mut dvi: &[u8],
    files: &HashMap<String, std::rc::Rc<Vec<u8>>>,
    images: &HashMap<String, Image>,
    missing_fonts: &[String],
) -> Result<super::CompileResult> {
    let mut warnings: Vec<String> = missing_fonts.to_vec();
    let mut widths_cache: HashMap<String, Option<HashMap<u32, f64>>> = HashMap::new();
    let mut pg = Page::default();
    let mut fonts = HashMap::new();
    let mut current = None;
    let mut k = 1. / 65536.;
    let mut output = None;
    let mut complete = false;
    while !dvi.is_empty() {
        let (rest, op) = Op::parse(dvi).map_err(|_| "invalid DVI instruction")?;
        dvi = rest;
        match op {
            Op::Pre { numerator, denominator, magnification, .. } => {
                k = numerator as f64 / denominator as f64 * magnification as f64 / 1000.
                    * 72.27
                    / 254000.
            }
            Op::Bop(..) => {
                pg = Page::default();
                current = None;
            }
            Op::Eop if pg.has_picture => {
                pg.flush_text();
                if output.is_some() {
                    return Err(
                        "expected one PGF figure, got multiple drawing pages".into()
                    );
                }
                let [w, h] =
                    pg.size.ok_or("missing PGF page size")?.map(|n| n.max(0.001));
                let [width, height] = [w * BP, h * BP];
                let svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{width}pt" height="{height}pt" viewBox="-72.27 -72.27 {w} {h}"><defs>{}</defs>{}</svg>"#,
                    pg.defs, pg.body
                );
                roxmltree::Document::parse(&svg)
                    .map_err(|e| format!("invalid SVG from PGF: {e}"))?;
                output = Some(super::CompileResult::Figure {
                    svg,
                    warnings: std::mem::take(&mut warnings),
                });
            }
            Op::PostPost { .. } => complete = true,
            Op::Push => {
                pg.flush_text();
                pg.stack.push(pg.pen);
            }
            Op::Pop => {
                pg.flush_text();
                pg.pen = pg.stack.pop().ok_or("DVI position stack underflow")?;
            }
            Op::Right(d) => {
                pg.flush_text();
                pg.pen.h += d as f64;
            }
            Op::Down(d) => {
                pg.flush_text();
                pg.pen.v += d as f64;
            }
            Op::W(d) => {
                pg.flush_text();
                pg.pen.w = d.unwrap_or(pg.pen.w);
                pg.pen.h += pg.pen.w as f64;
            }
            Op::X(d) => {
                pg.flush_text();
                pg.pen.x = d.unwrap_or(pg.pen.x);
                pg.pen.h += pg.pen.x as f64;
            }
            Op::Y(d) => {
                pg.flush_text();
                pg.pen.y = d.unwrap_or(pg.pen.y);
                pg.pen.v += pg.pen.y as f64;
            }
            Op::Z(d) => {
                pg.flush_text();
                pg.pen.z = d.unwrap_or(pg.pen.z);
                pg.pen.v += pg.pen.z as f64;
            }
            Op::FontDef(f) => {
                fonts.insert(
                    f.number,
                    (
                        String::from_utf8(f.filename).map_err(|e| e.to_string())?,
                        f.scale_factor as f64,
                    ),
                );
            }
            Op::Font(f) => {
                pg.flush_text();
                current = Some(f);
            }
            Op::Set(c) | Op::Put(c) => {
                let font_id = current.ok_or("undefined DVI font")?;
                let (name, scale) = fonts.get(&font_id).ok_or("undefined DVI font")?;
                let (family, bold, italic, enc, known) = map_font(name);
                if !known && !warnings.iter().any(|w| w == name) {
                    warnings.push(name.clone());
                }
                let glyph = decode_glyph(enc, c);
                let color = pg.colors.last().unwrap().clone();
                let x = pg.pen.h * k;
                let y = pg.pen.v * k;
                let font_size = scale * k;
                let can_append = matches!(op, Op::Set(_))
                    && pg.run.as_ref().is_some_and(|r| {
                        r.font_id == font_id && r.color == color && (r.y - y).abs() < 1e-6
                    });
                if can_append {
                    pg.run.as_mut().unwrap().text.push_str(&glyph);
                } else {
                    pg.flush_text();
                    let run = TextRun {
                        x,
                        y,
                        font_id,
                        family: family.to_owned(),
                        size: font_size,
                        bold,
                        italic,
                        color,
                        text: glyph,
                    };
                    pg.run = Some(run);
                    if matches!(op, Op::Put(_)) {
                        pg.flush_text();
                    }
                }
                if matches!(op, Op::Set(_)) {
                    let widths = widths_cache.entry(name.clone()).or_insert_with(|| {
                        files.get(&format!("{name}.tfm")).and_then(|b| tfm_widths(b))
                    });
                    let adv =
                        widths.as_ref().and_then(|m| m.get(&c).copied()).unwrap_or(0.55);
                    pg.pen.h += (adv * scale).round();
                }
            }
            Op::SetRule(h, w) | Op::PutRule(h, w) => {
                pg.flush_text();
                if h > 0 && w > 0 {
                    write!(
                        pg.body,
                        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="none"/>"#,
                        pg.pen.h * k,
                        (pg.pen.v - h as f64) * k,
                        w as f64 * k,
                        h as f64 * k,
                        pg.colors.last().unwrap()
                    )
                    .unwrap()
                }
                if matches!(op, Op::SetRule(..)) {
                    pg.pen.h += w as f64
                }
            }
            Op::Xxx(bytes) => pg.special(
                std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
                k,
                images,
            )?,
            _ => (),
        }
    }
    if !complete {
        return Err("incomplete TeX DVI output (missing postamble)".into());
    }
    output.ok_or("TeX produced no PGF picture".into())
}
