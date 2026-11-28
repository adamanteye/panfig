//! host the snapshot of e-TeX and PGF in the Typst plugin's linear memory.
#![cfg(target_arch = "wasm32")]
use panfig_image as image;
mod svg;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{cell::RefCell, collections::HashMap, io::Read, rc::Rc};

wasm_minimal_protocol::initiate_protocol!();

#[link(wasm_import_module = "tex")]
extern "C" {
    #[link_name = "restart"]
    fn run_tex();
    #[link_name = "history"]
    fn tex_history() -> i32;
}

const TEX_MEMORY: usize = 1100 * 65536;
const OUT: &str = "panfig.dvi";

#[derive(Default)]
struct Handle {
    name: String,
    bytes: usize,
    line: usize,
    missing: bool,
    eof: bool,
}

#[derive(Default)]
struct Files {
    data: HashMap<String, Rc<Vec<u8>>>,
    handles: Vec<Handle>,
    missed: Vec<String>,
    missing_fonts: Vec<String>,
    log: Vec<u8>,
    io_error: Option<String>,
}

/// synthesize minimal fallback metrics when e-TeX requests an unbundled TFM font.
fn fallback_tfm(name: &str) -> Vec<u8> {
    let fix = |v: f64| (v * 1_048_576.0).round() as i32;
    let (lh, bc, ec, nw, nh, nd, ni, np) =
        (2u16, 0u16, 127u16, 2u16, 2u16, 2u16, 1u16, 22u16);
    let lf = 6 + lh + (ec - bc + 1) + nw + nh + nd + ni + np;
    let mut out = Vec::with_capacity(usize::from(lf) * 4);
    for w in [lf, lh, bc, ec, nw, nh, nd, ni, 0, 0, 0, np] {
        out.extend_from_slice(&w.to_be_bytes());
    }
    // header: checksum 0, design size 10pt.
    out.extend_from_slice(&0i32.to_be_bytes());
    out.extend_from_slice(&fix(10.0).to_be_bytes());
    // char_info for codes 0..=127: width=1, height=1, depth=1, italic=0.
    for _ in bc..=ec {
        out.extend_from_slice(&[1, 0x11, 0, 0]);
    }
    for v in [0.0, 0.55, 0.0, 0.70, 0.0, 0.20, 0.0] {
        out.extend_from_slice(&fix(v).to_be_bytes());
    }
    let params: [f64; 22] = if name.contains("ex") {
        [
            0.0, 0.0, 0.0, 0.0, 0.431, 1.0, 0.0, 0.04, 0.111, 0.167, 0.200, 0.600, 0.100,
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ]
    } else {
        [
            0.0, 0.333, 0.167, 0.111, 0.431, 1.0, 0.111, 0.677, 0.394, 0.444, 0.686,
            0.345, 0.413, 0.363, 0.289, 0.150, 0.247, 0.386, 0.050, 2.390, 1.010, 0.250,
        ]
    };
    for v in params {
        out.extend_from_slice(&fix(v).to_be_bytes());
    }
    out
}

thread_local! { static FS: RefCell<Files> = RefCell::default(); }

// pointers passed across the linked modules are offsets in TeX's reserved memory.
unsafe fn mem<'a>(p: usize, n: usize) -> &'a mut [u8] {
    assert!(p > 0 && p.checked_add(n).is_some_and(|end| end <= TEX_MEMORY));
    std::slice::from_raw_parts_mut(p as *mut u8, n)
}

fn open(len: usize, p: usize, writing: bool) -> i32 {
    let name = std::str::from_utf8(unsafe { mem(p, len) })
        .unwrap_or("")
        .trim()
        .trim_matches(['{', '}'])
        .replace('"', "");
    let name = name.as_str();
    let name = name
        .strip_prefix("TeXfonts:")
        .or_else(|| name.strip_prefix("TeXinputs:"))
        .unwrap_or(name);
    let name = if writing && name == "TTY:" { "stdout" } else { name };
    FS.with_borrow_mut(|f| {
        let name = std::path::Path::new(name)
            .components()
            .collect::<std::path::PathBuf>()
            .to_string_lossy()
            .into_owned();
        if writing {
            f.data.insert(name.clone(), Rc::default());
        } else if !f.data.contains_key(&name) {
            if let Some(font) = name.strip_suffix(".tfm") {
                if !f.missing_fonts.iter().any(|s| s == font) {
                    f.missing_fonts.push(font.to_owned());
                }
                f.data.insert(name.clone(), Rc::new(fallback_tfm(font)));
            }
        }
        let missing = !f.data.contains_key(&name);
        if missing && !f.missed.iter().any(|s| s == &name) {
            f.missed.push(name.clone());
        }
        let id = f.handles.len();
        f.handles.push(Handle { name, missing, ..Default::default() });
        id as i32
    })
}
#[no_mangle]
pub extern "C" fn reset(l: usize, p: usize) -> i32 {
    open(l, p, false)
}
#[no_mangle]
pub extern "C" fn rewrite(l: usize, p: usize) -> i32 {
    open(l, p, true)
}
#[no_mangle]
pub extern "C" fn close(_fd: usize) {}
#[no_mangle]
pub extern "C" fn erstat(fd: usize) -> i32 {
    FS.with_borrow(|f| i32::from(f.handles[fd].missing))
}
#[no_mangle]
pub extern "C" fn eof(fd: usize) -> i32 {
    FS.with_borrow(|f| i32::from(f.handles[fd].eof))
}
#[no_mangle]
pub extern "C" fn eoln(fd: usize) -> i32 {
    FS.with_borrow(|f| {
        let h = &f.handles[fd];
        i32::from(
            h.eof
                || f.data
                    .get(&h.name)
                    .and_then(|b| b.get(h.line))
                    .is_none_or(|b| matches!(b, b'\r' | b'\n')),
        )
    })
}
#[no_mangle]
pub extern "C" fn get(fd: usize, p: usize, n: usize) {
    FS.with_borrow_mut(|f| {
        let h = &mut f.handles[fd];
        let b = f.data.get(&h.name).map_or(&[][..], |b| b.as_slice());
        let count = n.min(b.len().saturating_sub(h.bytes));
        let dest = unsafe { mem(p, n) };
        dest.fill(0);
        if count > 0 {
            dest[..count].copy_from_slice(&b[h.bytes..h.bytes + count]);
            h.bytes += count;
        } else {
            h.eof = true;
        }
    })
}
#[no_mangle]
pub extern "C" fn inputln(
    fd: usize,
    bypass: i32,
    buf: usize,
    first: usize,
    last: usize,
    high: usize,
    size: usize,
) -> i32 {
    FS.with_borrow_mut(|f| {
        let h = &mut f.handles[fd];
        let bytes = f.data.get(&h.name).map_or(&[][..], |b| b.as_slice());
        let start =
            u32::from_le_bytes(unsafe { mem(first, 4) }.try_into().unwrap()) as usize;
        if bypass != 0 {
            if bytes.get(h.line) == Some(&b'\r') {
                h.line += 1;
            }
            if bytes.get(h.line) == Some(&b'\n') {
                h.line += 1;
            }
        }
        if h.line >= bytes.len() {
            h.eof = true;
            return 0;
        }
        let mut end = h.line;
        while end < bytes.len() && !matches!(bytes[end], b'\r' | b'\n') {
            end += 1;
        }
        let mut line = &bytes[h.line..end];
        h.line = end;
        while line.last() == Some(&b' ') {
            line = &line[..line.len() - 1];
        }
        if start + line.len() >= size {
            f.io_error = Some(format!(
                "TeX input line exceeds the {size}-byte buffer in {}",
                h.name
            ));
            h.eof = true;
            return 0;
        }
        unsafe { mem(buf + start, line.len()) }.copy_from_slice(line);
        let new = (start + line.len()) as u32;
        unsafe { mem(last, 4) }.copy_from_slice(&new.to_le_bytes());
        let mx = u32::from_le_bytes(unsafe { mem(high, 4) }.try_into().unwrap());
        unsafe { mem(high, 4) }.copy_from_slice(&new.max(mx).to_le_bytes());
        1
    })
}
fn append(fd: i32, bytes: &[u8]) {
    FS.with_borrow_mut(|f| {
        if fd < 0 || f.handles.get(fd as usize).is_none_or(|h| h.name == "stdout") {
            f.log.extend(bytes);
        } else {
            let name = &f.handles[fd as usize].name;
            Rc::make_mut(f.data.get_mut(name).expect("open output file"))
                .extend_from_slice(bytes);
        }
    })
}
#[no_mangle]
pub extern "C" fn put(fd: i32, p: usize, n: usize) {
    append(fd, unsafe { mem(p, n) })
}
#[no_mangle]
pub extern "C" fn printInteger(fd: i32, v: i32) {
    append(fd, v.to_string().as_bytes())
}
#[no_mangle]
pub extern "C" fn printChar(fd: i32, v: u8) {
    append(fd, &[v])
}
#[no_mangle]
pub extern "C" fn printString(fd: i32, p: usize) {
    let n = unsafe { mem(p, 1) }[0] as usize;
    append(fd, unsafe { mem(p + 1, n) })
}
#[no_mangle]
pub extern "C" fn printNewline(fd: i32) {
    append(fd, b"\n")
}
#[no_mangle]
pub extern "C" fn getCurrentMinutes() -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn getCurrentDay() -> i32 {
    1
}
#[no_mangle]
pub extern "C" fn getCurrentMonth() -> i32 {
    1
}
#[no_mangle]
pub extern "C" fn getCurrentYear() -> i32 {
    2000
}
#[no_mangle]
pub extern "C" fn tex_final_end() {}

#[derive(Default)]
struct Runtime {
    pages: Vec<(usize, Vec<u8>)>,
    files: HashMap<String, Rc<Vec<u8>>>,
}

thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::default(); }

const FORMAT: &[u8] = include_bytes!(env!("PANFIG_FORMAT"));
const TEXMF: &[u8] = include_bytes!(env!("PANFIG_TEXMF"));

/// initialize an immutable Typst plugin transition, shared by all compilations.
#[wasm_minimal_protocol::wasm_func]
pub fn initialize() -> Result<Vec<u8>, String> {
    let mut runtime = Runtime::default();
    let mut fmt = flate2::read::GzDecoder::new(FORMAT);
    let mut page = [0u8; 65536];
    for offset in (0..TEX_MEMORY).step_by(page.len()) {
        fmt.read_exact(&mut page).map_err(|e| e.to_string())?;
        if offset == 0 && page[0] != 0 {
            return Err("invalid TeX format".into());
        }
        if let Some(end) = page.iter().rposition(|&v| v != 0) {
            let start = usize::from(offset == 0);
            if end >= start {
                runtime.pages.push((offset + start, page[start..=end].to_vec()));
            }
        }
    }
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(TEXMF));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut e = entry.map_err(|e| e.to_string())?;
        if e.header().entry_type().is_file() {
            let name =
                e.path().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
            let mut data = Vec::new();
            e.read_to_end(&mut data).map_err(|e| e.to_string())?;
            runtime.files.insert(name, Rc::new(data));
        }
    }
    RUNTIME.set(runtime);
    Ok(vec![])
}

/// execute a fresh TeX job against the initialized, read-only bundle.
#[wasm_minimal_protocol::wasm_func]
pub fn compile(source: &[u8], files: &[u8]) -> Result<Vec<u8>, String> {
    let mut fs = Files::default();
    RUNTIME.with_borrow(|rt| {
        if rt.pages.is_empty() {
            return Err("initialize the TeX runtime first");
        }
        unsafe { mem(1, TEX_MEMORY - 1) }.fill(0);
        for (offset, data) in &rt.pages {
            unsafe { mem(*offset, data.len()) }.copy_from_slice(data);
        }
        fs.data.clone_from(&rt.files);
        Ok(())
    })?;
    let mut images = HashMap::new();
    let inputs: HashMap<String, String> =
        serde_json::from_slice(files).map_err(|e| e.to_string())?;
    for (name, data) in inputs {
        let bytes = STANDARD.decode(&data).map_err(|e| e.to_string())?;
        if let Some(mime) = image::mime(&name) {
            let (w, h) = image::image_dimensions(&bytes, mime)?;
            let key = images.len().to_string();
            fs.data.insert(
                format!("{name}.panfig-size"),
                Rc::new(format!(r"\expandafter\def\csname Gin@urx\endcsname{{{w}}}\expandafter\def\csname Gin@ury\endcsname{{{h}}}\expandafter\def\csname panfigimagefile\endcsname{{{key}}}%").into_bytes()),
            );
            fs.data.insert(name.clone(), Rc::new(b"% image\n".to_vec()));
            let image = if mime == "image/svg+xml" && name.starts_with("panfig-label-") {
                let src = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                let doc = roxmltree::Document::parse(src).map_err(|e| e.to_string())?;
                let content = doc
                    .root_element()
                    .children()
                    .filter(|n| n.is_element())
                    .map(|n| &src[n.range()])
                    .collect();
                svg::Image::Label { width: w, height: h, content }
            } else {
                svg::Image::Encoded(format!("data:{mime};base64,{data}"))
            };
            images.insert(key, image);
        } else {
            fs.data.insert(name, Rc::new(bytes));
        }
    }
    fs.data.insert(
        "panfig.tex".into(),
        Rc::new(
            [b"\n".as_slice(), include_bytes!("../picture.tex"), b"\n", source].concat(),
        ),
    );
    fs.data
        .insert("TTY:".into(), Rc::new(b" panfig.tex\n\\end\n".to_vec()));
    FS.set(fs);
    unsafe { run_tex() };
    FS.with_borrow(|f| {
        if let Some(error) = &f.io_error {
            return Err(error.clone());
        }
        let log = f.data.get("panfig.log").map_or(f.log.as_slice(), |b| b.as_slice());
        let log = String::from_utf8_lossy(log);
        if unsafe { tex_history() } >= 2 {
            let i = log
                .lines()
                .position(|line| {
                    line.trim_start().starts_with('!')
                        || line.contains(": Undefined control sequence.")
                })
                .unwrap_or_else(|| log.lines().count().saturating_sub(7));
            let message = log.lines().skip(i).take(7).collect::<Vec<_>>().join("\n");
            // TeX wraps a diagnostic mid-word at max_print_line, including file names.
            let first: String = log
                .lines()
                .skip(i)
                .take_while(|line| {
                    !line.is_empty() && !line.starts_with("l.") && !line.starts_with("<")
                })
                .collect();
            let wanted = first
                .split_once("File `")
                .and_then(|(_, s)| s.split_once("' not found").map(|(s, _)| s))
                .or_else(|| {
                    first
                        .split_once("I can't find file `")
                        .and_then(|(_, s)| s.split_once('\'').map(|(s, _)| s))
                })
                .or_else(|| {
                    first
                        .split_once("Missing panfig image: ")
                        .map(|(_, s)| s.trim_end_matches('.'))
                });
            let mut missing = Vec::new();
            if let Some(name) = wanted {
                let name = name.strip_suffix(".panfig-size").unwrap_or(name);
                if f.missed.iter().any(|p| p == name)
                    && std::path::Path::new(name).extension().is_some()
                {
                    missing.push(name.to_owned());
                } else {
                    missing.extend(
                        f.missed
                            .iter()
                            .filter(|p| {
                                *p == name
                                    || p.strip_prefix(name).is_some_and(|ext| {
                                        matches!(
                                            ext,
                                            ".tex" | ".png" | ".jpg" | ".jpeg" | ".svg"
                                        )
                                    })
                            })
                            .cloned(),
                    );
                }
            }
            return serde_json::to_vec(
                &serde_json::json!({"error":message,"missing":missing}),
            )
            .map_err(|e| e.to_string());
        }
        let dvi = f.data.get(OUT).ok_or_else(|| {
            format!(
                "TeX produced no DVI: {}",
                log.chars()
                    .rev()
                    .take(350)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>()
            )
        })?;
        svg::render(dvi, &f.data, &images, &f.missing_fonts)
    })
}
