//! Icon build-boundary tests.

#[path = "../icon_build.rs"]
mod icon_build;

use usvg::{Node, Paint, Tree};

const VALID: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor"><path d="M1 1h14v14H1z"/></svg>"#;

#[test]
fn accepted_icons_are_normalized_to_white() -> Result<(), Box<dyn std::error::Error>> {
    let normalized = icon_build::normalize(VALID)?;
    let tree = Tree::from_str(&normalized, &usvg::Options::default())?;
    let Node::Path(path) = &tree.root().children()[0] else {
        panic!("normalized icon must contain a path");
    };
    assert_eq!(
        path.fill().map(usvg::Fill::paint),
        Some(&Paint::Color(usvg::Color::white()))
    );
    Ok(())
}

#[test]
fn unsupported_or_ambiguous_svg_is_rejected() {
    let cases = [
        r#"<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="7"/></svg>"#,
        r##"<svg viewBox="0 0 16 16"><path d="M1 1h14v14H1z" stroke="#fff"/></svg>"##,
        r##"<svg viewBox="0 0 16 16"><path d="M1 1h7v14H1z" fill="#000"/><path d="M8 1h7v14H8z" fill="#fff"/></svg>"##,
        r#"<svg viewBox="0 0 16 16"></svg>"#,
    ];

    for svg in cases {
        assert!(icon_build::normalize(svg).is_err());
    }
}

#[test]
fn namespaced_attributes_are_rejected() {
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="urn:test" viewBox="0 0 16 16"><path d="M0 0h1v1H0z" xlink:href="#bad"/></svg>"##;
    let error =
        icon_build::normalize(svg).expect_err("a namespaced attribute must not bypass validation");

    assert!(
        error
            .to_string()
            .contains("unsupported `path` attribute `href`")
    );
}

#[test]
fn catalog_names_must_be_unique_uppercase_identifiers() {
    assert!(icon_build::validate_catalog(&[("VALID_NAME", VALID)]).is_ok());
    assert!(icon_build::validate_catalog(&[("bad-name", VALID)]).is_err());
    assert!(icon_build::validate_catalog(&[("DUPLICATE", VALID), ("DUPLICATE", VALID)]).is_err());
}
