use panfig_image as image;

mod drawing;
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
    let mut inputs = resources::Inputs::new(
        serde_json::from_slice(files)
            .map_err(|e| format!("invalid resource table: {e}"))?,
    );
    let result = pgfcodec::read_with(source(data)?, &mut inputs).and_then(|items| {
        drawing::validate(&items)?;
        render::layers(&items)
    });
    if let Some(name) = inputs.need {
        return json(&serde_json::json!({ "input": name }));
    }
    match result {
        Ok(layers) => json(&layers),
        Err(error) if error.contains("cyclic PGF input") => Err(error),
        Err(reason) => json(&serde_json::json!({ "tex": reason })),
    }
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
    pgfcodec::named_labels(source(data)?, source(prefix)?)
}

/// transfer binary image resources without expanding them into JSON number arrays.
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
