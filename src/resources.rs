//! resolve nested project inputs through the Typst caller's resource loader.
use super::drawing::Result;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Default)]
pub struct Inputs {
    pub files: HashMap<String, Rc<str>>,
    pub need: Option<String>,
    active: HashSet<String>,
}

impl Inputs {
    pub fn new(files: HashMap<String, Rc<str>>) -> Self {
        Self { files, ..Self::default() }
    }

    pub fn include<T>(
        &mut self,
        path: &str,
        parse: impl FnOnce(&str, &mut Self) -> Result<T>,
    ) -> Result<T> {
        let Some(source) = self.files.get(path).cloned() else {
            self.need = Some(path.to_owned());
            return Err(format!("missing PGF input {path:?}"));
        };
        if !self.active.insert(path.to_owned()) {
            return Err(format!("cyclic PGF input {path:?}"));
        }
        if self.active.len() > 128 {
            return Err("PGF input nesting exceeds 128 files".into());
        }
        let result = parse(&source, self);
        self.active.remove(path);
        result
    }
}

/// paths reported to the callback are relative to the main figure's directory.
pub fn resolve(name: &str) -> Result<String> {
    let absolute = name.starts_with('/');
    let mut components = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => (),
            ".." if components.last().is_some_and(|v| *v != "..") => {
                components.pop();
            }
            ".." if absolute => (),
            s => components.push(s),
        }
    }
    let mut path = components.join("/");
    if path.is_empty() || name.chars().any(char::is_control) {
        return Err("invalid resource path".into());
    }
    if absolute {
        path.insert(0, '/');
    }
    Ok(path)
}
