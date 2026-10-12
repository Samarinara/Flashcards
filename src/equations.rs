//! Equations are stored as ordinary card text; only the study view interprets them.

use latex_rust::{Color, Dim, MathFont, PngOptions};

#[derive(Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Math {
        source: &'a str,
        latex: &'a str,
        display: bool,
    },
}

/// Recognize paired LaTeX delimiters, retaining the original source for fallback.
/// Dollar inline math cannot start/end with whitespace or span multiple lines.
pub fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut cursor = 0;
    while cursor < text.len() {
        if !text.is_char_boundary(cursor) || escaped(text, cursor) {
            cursor += 1;
            continue;
        }
        let rest = &text[cursor..];
        let delimiter = if rest.starts_with("$$") {
            Some(("$$", "$$", true))
        } else if rest.starts_with(r"\[") {
            Some((r"\[", r"\]", true))
        } else if rest.starts_with(r"\(") {
            Some((r"\(", r"\)", false))
        } else if rest.starts_with('$') {
            Some(("$", "$", false))
        } else {
            None
        };
        let Some((open, close, display)) = delimiter else {
            cursor += 1;
            continue;
        };
        let body_start = cursor + open.len();
        let end = text[body_start..]
            .match_indices(close)
            .map(|(index, _)| body_start + index)
            .find(|&index| !escaped(text, index));
        if let Some(end) = end {
            let body = &text[body_start..end];
            let valid = !body.trim().is_empty()
                && (open != "$"
                    || (!body.starts_with(char::is_whitespace)
                        && !body.ends_with(char::is_whitespace)
                        && !body.contains('\n')
                        && !text[end..].starts_with("$$")));
            if valid {
                if start < cursor {
                    result.push(Segment::Text(&text[start..cursor]));
                }
                let next = end + close.len();
                result.push(Segment::Math {
                    source: &text[cursor..next],
                    latex: body,
                    display,
                });
                cursor = next;
                start = next;
                continue;
            }
        }
        cursor += open.len();
    }
    if start < text.len() {
        result.push(Segment::Text(&text[start..]));
    }
    result
}

fn escaped(text: &str, index: usize) -> bool {
    text.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        == 1
}

/// Rasterize with a bundled font, without network access or external TeX tools.
pub fn render_png(
    latex: &str,
    display: bool,
    font_size_pt: i64,
    dark: bool,
) -> Result<Vec<u8>, latex_rust::Error> {
    thread_local! {
        static FONT: Result<MathFont, latex_rust::Error> = MathFont::stix_two_math();
    }
    FONT.with(|font| {
        let font = font.as_ref().map_err(|_| latex_rust::Error::Unsupported {
            what: "Could not load the equation font".into(),
        })?;
        latex_rust::latex_to_png(
            latex,
            font,
            &PngOptions {
                dpi: Dim::from_i64(96),
                font_size_pt: Dim::from_i64(font_size_pt),
                color: if dark {
                    Color::rgb(255, 255, 255)
                } else {
                    Color::rgb(0, 0, 0)
                },
                display,
                ..PngOptions::default()
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_text_and_all_delimiters() {
        let text = r"α: $x^2$ and \(y_1\). $$\frac{1}{2}$$ \[\sqrt{x}\]";
        let parts = segments(text);
        let equations: Vec<_> = parts
            .iter()
            .filter_map(|part| match part {
                Segment::Math { latex, display, .. } => Some((*latex, *display)),
                _ => None,
            })
            .collect();
        assert_eq!(
            equations,
            vec![
                ("x^2", false),
                ("y_1", false),
                (r"\frac{1}{2}", true),
                (r"\sqrt{x}", true)
            ]
        );
        let reconstructed: String = parts
            .iter()
            .map(|part| match part {
                Segment::Text(text) => *text,
                Segment::Math { source, .. } => *source,
            })
            .collect();
        assert_eq!(reconstructed, text);
    }

    #[test]
    fn ordinary_text_currency_and_unclosed_delimiters_are_preserved() {
        for text in [
            "Plain <text> & Unicode: 日本語\nNext line",
            "Costs $5 and $10",
            r"Escaped \$x\$ and an unmatched $",
            "Unfinished $$x",
            r"Unfinished \(x",
            "$x\ny$",
            "$$",
        ] {
            assert_eq!(segments(text), vec![Segment::Text(text)], "{text}");
        }
        assert!(segments("").is_empty());
    }

    #[test]
    fn display_math_can_span_lines_and_escaped_closers_are_skipped() {
        assert_eq!(
            segments("$$x +\ny$$"),
            vec![Segment::Math {
                source: "$$x +\ny$$",
                latex: "x +\ny",
                display: true
            }]
        );
        assert_eq!(
            segments(r"$x + \$y$"),
            vec![Segment::Math {
                source: r"$x + \$y$",
                latex: r"x + \$y",
                display: false
            }]
        );
    }

    #[test]
    fn renders_real_equations_in_both_themes_and_rejects_bad_latex() {
        for latex in [
            r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
            r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
            r"\int_0^1 x^2\,dx = \frac{1}{3}",
            r"\alpha^2 + \beta_1",
        ] {
            let light = render_png(latex, true, 20, false).unwrap();
            let dark = render_png(latex, true, 20, true).unwrap();
            assert!(light.starts_with(b"\x89PNG"));
            assert!(dark.starts_with(b"\x89PNG"));
            assert_ne!(light, dark);
        }
        assert!(render_png(r"\frac{", false, 20, false).is_err());
        assert!(render_png(r"\notarealcommand{x}", false, 20, false).is_err());
    }
}
