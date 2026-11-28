use panfig_image as image;

mod drawing;
mod labels;
mod pgf;
mod pgfcodec;
mod render;
mod resources;
mod svg;
mod texmath;

#[cfg(target_arch = "wasm32")]
wasm_minimal_protocol::initiate_protocol!();

fn source(data: &[u8]) -> drawing::Result<&str> {
    std::str::from_utf8(data)
        .map_err(|e| format!("invalid UTF-8 at byte {}", e.valid_up_to()))
}

/// read PGF into paint-ordered instructions, requesting TeX for dynamic programs.
#[cfg_attr(target_arch = "wasm32", wasm_minimal_protocol::wasm_func)]
pub fn read_pgf(data: &[u8], files: &[u8]) -> Result<Vec<u8>, String> {
    let files =
        typwire::decode(files).map_err(|e| format!("invalid resource table: {e}"))?;
    let mut inputs = resources::Inputs::new(&files);
    let result = pgfcodec::read_with(source(data)?, &mut inputs).and_then(|figure| {
        drawing::validate(&figure)?;
        render::layers(&figure)
    });
    let reply = match (inputs.need, result) {
        (Some(name), _) => PgfResult::Input { name },
        (_, Ok(figure)) => PgfResult::Figure(figure),
        (_, Err(error)) if error.contains("cyclic PGF input") => return Err(error),
        (_, Err(reason)) => PgfResult::Tex { reason },
    };
    typwire::encode(&reply).map_err(|e| e.to_string())
}

#[derive(serde::Serialize, serde::Deserialize, typwire::Typwire)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum PgfResult {
    Figure(render::Layers),
    Input { name: String },
    Tex { reason: String },
}

fn json(value: &impl serde::Serialize) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|e| e.to_string())
}

/// split an SVG viewport into native content and self-contained vector layers.
#[cfg_attr(target_arch = "wasm32", wasm_minimal_protocol::wasm_func)]
pub fn read_svg(data: &[u8], host_labels: &[u8]) -> Result<Vec<u8>, String> {
    svg::read(source(data)?, host_labels == b"tex")
}

/// identify literal PGF labels for the host typesetter.
#[cfg_attr(target_arch = "wasm32", wasm_minimal_protocol::wasm_func)]
pub fn labels(data: &[u8], prefix: &[u8]) -> Result<Vec<u8>, String> {
    labels::named_labels(source(data)?, source(prefix)?)
}

/// encode image data for embedding in an SVG data URL.
#[cfg_attr(target_arch = "wasm32", wasm_minimal_protocol::wasm_func)]
pub fn pack(data: &[u8]) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    Ok(STANDARD.encode(data).into_bytes())
}

/// read intrinsic width and height from the same image metadata used by the TeX driver.
#[cfg_attr(target_arch = "wasm32", wasm_minimal_protocol::wasm_func)]
pub fn image_size(data: &[u8], file: &[u8]) -> Result<Vec<u8>, String> {
    let name = source(file)?;
    let mime =
        image::mime(name).ok_or_else(|| format!("unsupported image type: {name}"))?;
    let (width, height) = image::image_dimensions(data, mime)?;
    serde_json::to_vec(&(width, height)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::pgf::Parser;
    use serde_json::{json, Value};

    fn read_layers(source: &[u8]) -> Value {
        let files =
            typwire::encode(&std::collections::HashMap::<String, String>::new()).unwrap();
        let response = super::read_pgf(source, &files).unwrap();
        match typwire::decode(&response).unwrap() {
            super::PgfResult::Figure(figure) => serde_json::to_value(figure).unwrap(),
            _ => panic!("expected native PGF layers"),
        }
    }

    fn read_events(source: &str) -> Result<Vec<super::pgf::Event<'_>>, String> {
        let mut parser = Parser::new(source);
        let mut events = Vec::new();
        while let Some(event) = parser.next_event()? {
            events.push(event);
        }
        Ok(events)
    }

    fn events(source: &str) -> Value {
        serde_json::to_value(read_events(source).unwrap()).unwrap()
    }

    #[test]
    fn clips_marker_replays_in_paint_order() {
        let source = br"\begin{pgfpicture}
            \pgfsys@defobject{marker}{\pgfpointorigin}{\pgfqpoint{4bp}{4bp}}{
                \pgfpathrectangle{\pgfpointorigin}{\pgfqpoint{4bp}{4bp}}\pgfusepath{fill}}
            \begin{pgfscope}
                \pgfpathcircle{\pgfpointorigin}{3bp}\pgfusepath{clip}
                \pgfsys@transformshift{1bp}{2bp}
                \pgfpathrectangle{\pgfpointorigin}{\pgfqpoint{2bp}{6bp}}\pgfusepath{clip}
                \pgfsetfillcolor{red}\pgfsys@useobject{marker}{}
            \end{pgfscope}
            \pgftext{separator}
            \pgfsetfillcolor{blue}\pgfsys@useobject{marker}{}
            \end{pgfpicture}";
        let doc = read_layers(source);
        let layers = doc["layers"].as_array().unwrap();
        let kinds: Vec<_> = layers.iter().map(|l| l["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["bounds", "svg", "text", "svg"]);
        assert_eq!(layers[1]["rect"], json!([1., -3., 2., 1.]));
        let svg = roxmltree::Document::parse(layers[1]["svg"].as_str().unwrap()).unwrap();
        let marker = svg
            .descendants()
            .find(|n| n.attribute("fill") == Some("#ff0000"))
            .unwrap();
        assert_eq!(
            marker.parent_element().unwrap().attribute("transform"),
            Some("matrix(1 0 0 1 1 -2)")
        );
        assert_eq!(
            marker.ancestors().filter(|n| n.has_attribute("clip-path")).count(),
            2
        );
        assert_eq!(layers[3]["rect"], json!([0., -4., 4., 4.]));
        let svg2 =
            roxmltree::Document::parse(layers[3]["svg"].as_str().unwrap()).unwrap();
        assert!(svg2.descendants().any(|n| n.attribute("fill") == Some("#0000ff")));
    }

    #[test]
    fn coalesces_identical_clips_across_scopes() {
        let source = br"\begin{pgfpicture}
            \pgfpathrectangle{\pgfpointorigin}{\pgfqpoint{20bp}{20bp}}\pgfusepath{use as bounding box,clip}
            \begin{pgfscope}
                \pgfpathrectangle{\pgfqpoint{2bp}{2bp}}{\pgfqpoint{10bp}{10bp}}\pgfusepath{clip}
                \pgfsetmiterlimit{3}\pgfpathmoveto{\pgfpointorigin}\pgfpathlineto{\pgfqpoint{10bp}{10bp}}\pgfusepath{stroke}
            \end{pgfscope}
            \begin{pgfscope}
                \pgfpathrectangle{\pgfqpoint{2bp}{2bp}}{\pgfqpoint{10bp}{10bp}}\pgfusepath{clip}
                \definecolor{g}{gray}{0.5}\pgfsetstrokecolor{g}
                \pgfpathmoveto{\pgfqpoint{0bp}{10bp}}\pgfpathlineto{\pgfqpoint{10bp}{0bp}}\pgfusepath{stroke}
            \end{pgfscope}
            \end{pgfpicture}";
        let doc = read_layers(source);
        assert_eq!(doc["viewport"], json!([0., -20., 20., 20.]));
        let layers = doc["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0]["kind"], "svg");
        let svg = roxmltree::Document::parse(layers[0]["svg"].as_str().unwrap()).unwrap();
        let clips = svg.descendants().filter(|n| n.has_tag_name("clipPath")).count();
        assert_eq!(clips, 1);
        assert!(svg
            .descendants()
            .any(|n| n.attribute("stroke-miterlimit") == Some("3")));
        assert!(svg.descendants().any(|n| n.attribute("stroke") == Some("#808080")));
    }

    #[test]
    fn normalizes_math_bars_and_label_modifiers() {
        let bare = super::texmath::to_typst(r"|\psi|^2 + \|\phi\|").unwrap();
        assert!(bare.contains(r#"class("normal", bar.v)"#));
        assert!(bare.contains(r#"class("normal", bar.v.double)"#));
        let stretched = super::texmath::to_typst(r"\left|\frac{a}{b}\right|").unwrap();
        assert!(stretched.starts_with("lr(|"));
        assert!(!stretched.contains(r#"class("normal""#));
        let sized = super::texmath::to_typst(r"\big| x \big|").unwrap();
        assert!(!sized.contains(r#"class("normal""#));

        let source = br"\begin{pgfpicture}
            \pgfsetfillcolor{red}\pgfsetfillopacity{0.5}
            \pgftext{\fontsize{10}{12}\bfseries\itshape\selectfont Hi}
            \end{pgfpicture}";
        let doc = read_layers(source);
        let text = &doc["layers"][0];
        assert_eq!(text["kind"], "text");
        assert_eq!(text["opacity"], json!(0.5));
        assert_eq!(text["runs"][0]["bold"], json!(true));
        assert_eq!(text["runs"][0]["italic"], json!(true));
        assert!(text["runs"][0].get("fill").is_none());
    }

    #[test]
    fn preserves_contents_and_order_of_delimited_arguments() {
        let source = r"\pgftext% trailing comment
            [at={a]b}]{峰值 \{A\} \% % a commented-out }
B} \pgfsys@useobject{marker}{}";
        assert_eq!(
            events(source),
            json!([
                {"kind": "command", "name": "pgftext", "args": [
                    {"kind": "optional", "value": "at={a]b}"},
                    {"kind": "required", "value": "峰值 \\{A\\} \\% % a commented-out }\nB"}
                ]},
                {"kind": "command", "name": "pgfsys@useobject", "args": [
                    {"kind": "required", "value": "marker"}, {"kind": "required", "value": ""}
                ]}
            ])
        );
    }

    #[test]
    fn emits_standalone_groups_text_and_control_symbols() {
        assert_eq!(
            events(r"{A B \%}"),
            json!([
                {"kind": "group-start"}, {"kind": "text", "value": "A B"},
                {"kind": "command", "name": "%", "args": []}, {"kind": "group-end"}
            ])
        );
        assert_eq!(events(" % comment\r\n % at EOF"), json!([]));
    }

    #[test]
    fn keeps_drawing_commands_inside_brace_scopes_in_the_stream() {
        let stream = events(
            r"\begin{pgfpicture}{\pgfpathclose{\pgfusepath{stroke}}}\end{pgfpicture}",
        );
        let kinds: Vec<_> = stream
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event.get("name").unwrap_or(&event["kind"]).as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "begin",
                "group-start",
                "pgfpathclose",
                "group-start",
                "pgfusepath",
                "group-end",
                "group-end",
                "end",
            ]
        );
    }

    #[test]
    fn reports_positions_for_malformed_figures() {
        for (source, expected) in [
            (
                "\\begin{pgfpicture}\n\\end{pgfscope}",
                "line 2, column 1: expected \\end{pgfpicture}, found \\end{pgfscope}",
            ),
            (r"\begin{pgfpicture}", r"line 1, column 1: missing \end{pgfpicture}"),
            ("图 \\foo{a", "line 1, column 7: unclosed { argument"),
            (r"\foo[x", "line 1, column 5: unclosed [ argument"),
            ("}", "line 1, column 1: unexpected }"),
            ("% comment\r}", "line 2, column 1: unexpected }"),
            ("\\", "line 1, column 1: dangling backslash"),
        ] {
            assert_eq!(read_events(source).err().unwrap(), expected);
        }
        assert_eq!(super::source(&[b'a', 0xff]).unwrap_err(), "invalid UTF-8 at byte 1");
    }
}
