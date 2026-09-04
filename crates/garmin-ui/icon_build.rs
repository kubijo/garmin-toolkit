use std::{collections::BTreeSet, env, error, fmt, fs, path::PathBuf};

use usvg::{BlendMode, Indent, Node, Opacity, Paint, Tree, WriteOptions};

const WHITE_STYLE: &str = "path { fill: #fff !important; stroke: none !important; }";

#[cfg_attr(
    test,
    expect(
        dead_code,
        reason = "integration tests exercise validation without writing Cargo build output"
    )
)]
pub fn write_catalog(icons: &[(&str, &str)]) -> Result<(), Error> {
    let output = PathBuf::from(env::var("OUT_DIR").map_err(|error| Error::new(error.to_string()))?)
        .join("icons");
    fs::create_dir_all(&output).map_err(Error::from)?;

    for (name, svg) in validate_catalog(icons)? {
        fs::write(output.join(format!("{name}.svg")), svg).map_err(Error::from)?;
    }
    Ok(())
}

pub fn validate_catalog<'a>(
    icons: &'a [(&'a str, &'a str)],
) -> Result<Vec<(&'a str, String)>, Error> {
    let mut names = BTreeSet::new();
    let mut validated = Vec::with_capacity(icons.len());
    for &(name, svg) in icons {
        validate_name(name)?;
        if !names.insert(name) {
            return Err(Error::new(format!("duplicate icon name `{name}`")));
        }
        validated.push((name, normalize(svg)?));
    }
    Ok(validated)
}

pub fn normalize(svg: &str) -> Result<String, Error> {
    validate_source(svg)?;
    let tree = Tree::from_str(svg, &usvg::Options::default()).map_err(Error::from_display)?;
    validate_tree(&tree)?;

    let options = usvg::Options {
        style_sheet: Some(WHITE_STYLE.to_owned()),
        ..usvg::Options::default()
    };
    let tree = Tree::from_str(svg, &options).map_err(Error::from_display)?;
    let writer = WriteOptions {
        indent: Indent::None,
        ..WriteOptions::default()
    };
    Ok(tree.to_string(&writer))
}

fn validate_name(name: &str) -> Result<(), Error> {
    let valid = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_uppercase())
        && name.as_bytes().first().is_some_and(u8::is_ascii_uppercase);
    if valid {
        Ok(())
    } else {
        Err(Error::new(format!(
            "icon name `{name}` must be uppercase snake case"
        )))
    }
}

fn validate_source(svg: &str) -> Result<(), Error> {
    let document = usvg::roxmltree::Document::parse(svg).map_err(Error::from_display)?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err(Error::new("icon root must be `svg`"));
    }
    if root.attribute("viewBox").is_none() {
        return Err(Error::new("icon must declare a viewBox"));
    }

    for node in document.descendants() {
        if node.is_text() && node.text().is_some_and(|text| !text.trim().is_empty()) {
            return Err(Error::new("icon cannot contain text"));
        }
        if !node.is_element() {
            continue;
        }
        let element = node.tag_name().name();
        let allowed = match element {
            "svg" => &["fill", "height", "viewBox", "width"][..],
            "path" => &["d", "fill", "fill-rule", "transform"][..],
            _ => return Err(Error::new(format!("unsupported SVG element `{element}`"))),
        };
        for attribute in node.attributes() {
            if !allowed.contains(&attribute.name()) {
                return Err(Error::new(format!(
                    "unsupported `{element}` attribute `{}`",
                    attribute.name()
                )));
            }
        }
    }
    Ok(())
}

fn validate_tree(tree: &Tree) -> Result<(), Error> {
    let mut state = ValidationState::default();
    validate_group(tree.root(), &mut state)?;
    if state.paths == 0 {
        return Err(Error::new("icon contains no visible filled paths"));
    }
    Ok(())
}

fn validate_group(group: &usvg::Group, state: &mut ValidationState) -> Result<(), Error> {
    if group.opacity() != Opacity::ONE
        || group.blend_mode() != BlendMode::Normal
        || group.isolate()
        || group.clip_path().is_some()
        || group.mask().is_some()
        || !group.filters().is_empty()
    {
        return Err(Error::new("icon groups cannot alter paint or geometry"));
    }

    for node in group.children() {
        match node {
            Node::Group(child) => validate_group(child, state)?,
            Node::Path(path) => validate_path(path, state)?,
            Node::Image(_) | Node::Text(_) => {
                return Err(Error::new("icon contains unsupported rendered content"));
            }
        }
    }
    Ok(())
}

fn validate_path(path: &usvg::Path, state: &mut ValidationState) -> Result<(), Error> {
    if !path.is_visible() {
        return Err(Error::new("icon paths must be visible"));
    }
    if path.stroke().is_some() {
        return Err(Error::new("icon strokes must be converted to filled paths"));
    }
    let fill = path
        .fill()
        .ok_or_else(|| Error::new("icon paths must have a fill"))?;
    if fill.opacity() != Opacity::ONE {
        return Err(Error::new("icon paths must be opaque"));
    }
    let Paint::Color(color) = fill.paint() else {
        return Err(Error::new("icon paths must use a solid color"));
    };
    if state
        .color
        .replace(*color)
        .is_some_and(|seen| seen != *color)
    {
        return Err(Error::new("icon paths must be monochrome"));
    }
    state.paths += 1;
    Ok(())
}

#[derive(Default)]
struct ValidationState {
    color: Option<usvg::Color>,
    paths: usize,
}

#[derive(Debug)]
pub struct Error(String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    fn from_display(error: impl fmt::Display) -> Self {
        Self::new(error.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::from_display(error)
    }
}
