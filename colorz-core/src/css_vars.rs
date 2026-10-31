//! CSS custom property export, import and merge.

use std::collections::HashSet;

use crate::color::{Color, RgbSpace};
use crate::format::{Form, Format, FormatOptions, format_color};
use crate::palette::{Palette, PaletteColor, is_valid_var_name};
use crate::parse::parse_css_color;
use crate::target::BuiltinTarget;

#[derive(Clone, Debug, PartialEq)]
pub struct CssVar {
    pub name: String,
    pub color: Color,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CssBlock {
    /// The selector with any enclosing at-rule prelude, for display.
    pub selector: String,
    /// Only the rule's own selector; empty for a bare or at-rule block.
    pub inner_selector: String,
    pub vars: Vec<CssVar>,
    pub skipped: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeSummary {
    pub updated: usize,
    pub added: usize,
    pub skipped: usize,
}

/// Exports with sRGB/CSS semantics, ignoring any "Copy as" target. A format
/// that is not valid CSS (see [`Format::is_css`]) is written as HEX.
pub fn export_css(p: &Palette, format: Format, selector: &str, opts: &FormatOptions) -> String {
    let format = if format.is_css() { format } else { Format::Hex };
    let used: HashSet<&str> = p
        .colors
        .iter()
        .map(|c| c.var_name.as_str())
        .filter(|n| !n.is_empty())
        .collect();
    let mut generated: Vec<String> = Vec::new();
    let mut out = format!("{selector} {{\n");
    for (i, c) in p.colors.iter().enumerate() {
        let name = if c.var_name.is_empty() {
            let base = format!("color-{}", i + 1);
            let mut cand = base.clone();
            let mut n = 2;
            while used.contains(cand.as_str()) || generated.contains(&cand) {
                cand = format!("{base}-{n}");
                n += 1;
            }
            generated.push(cand.clone());
            cand
        } else {
            c.var_name.clone()
        };
        let value = format_color(
            &c.color,
            format,
            Form::Full,
            opts,
            &BuiltinTarget(RgbSpace::Srgb),
        )
        .map(|f| f.text)
        .unwrap_or_default();
        out.push_str(&format!("  --{name}: {value};"));
        if !c.label.is_empty() {
            let label = c.label.replace(['\r', '\n'], " ").replace("*/", "* /");
            out.push_str(&format!(" /* {label} */"));
        }
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

struct Builder {
    selector: String,
    inner: String,
    vars: Vec<CssVar>,
    skipped: usize,
}

impl Builder {
    fn new(selector: String, inner: String) -> Self {
        Builder {
            selector,
            inner,
            vars: Vec::new(),
            skipped: 0,
        }
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_important(v: &str) -> &str {
    let v = v.trim_end();
    let lower = v.to_ascii_lowercase();
    match lower.strip_suffix("important") {
        Some(rest) if rest.trim_end().ends_with('!') => v[..rest.trim_end().len() - 1].trim_end(),
        _ => v,
    }
}

/// Finishes the declaration collected in `buf`; returns where it landed.
fn finish_decl(
    buf: &mut String,
    blocks: &mut Vec<Builder>,
    cur: Option<usize>,
) -> Option<(usize, usize)> {
    let decl = std::mem::take(buf);
    let decl = decl.trim();
    let rest = decl.strip_prefix("--")?;
    let (name, value) = rest.split_once(':')?;
    let name = name.trim();
    let bi = match cur {
        Some(i) => i,
        None => blocks
            .iter()
            .position(|b| b.selector.is_empty())
            .unwrap_or_else(|| {
                blocks.push(Builder::new(String::new(), String::new()));
                blocks.len() - 1
            }),
    };
    add_var(blocks, bi, name, strip_important(value))
}

fn add_var(blocks: &mut [Builder], bi: usize, name: &str, value: &str) -> Option<(usize, usize)> {
    let b = &mut blocks[bi];
    let color = if name.is_empty() || !is_valid_var_name(name) {
        None
    } else {
        parse_css_color(value.trim())
    };
    let Some(color) = color else {
        b.skipped += 1;
        return None;
    };
    let var = CssVar {
        name: name.to_string(),
        color,
        label: None,
    };
    match b.vars.iter().position(|v| v.name == name) {
        Some(i) => {
            b.vars[i] = var;
            Some((bi, i))
        }
        None => {
            b.vars.push(var);
            Some((bi, b.vars.len() - 1))
        }
    }
}

/// Parses every rule block that defines custom properties. Comments, strings,
/// escapes and parentheses are opaque to the structure scan; nested rules keep
/// the enclosing at-rule prelude in their selector.
pub fn parse_css_blocks(text: &str) -> Vec<CssBlock> {
    let chars: Vec<char> = text.chars().collect();
    let mut blocks: Vec<Builder> = Vec::new();
    // (block index, at-rule prefix inherited by children)
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut buf = String::new();
    // The declaration that ended with `;` with no other text since.
    let mut last: Option<(usize, usize)> = None;
    // A comment at the end of the declaration being collected.
    let mut inline: Option<String> = None;
    let mut parens = 0usize;
    let mut i = 0;
    macro_rules! finish {
        () => {{
            let r = finish_decl(&mut buf, &mut blocks, stack.last().map(|s| s.0));
            if let (Some((b, v)), Some(c)) = (r, inline.take()) {
                if !c.is_empty() {
                    blocks[b].vars[v].label = Some(c);
                }
            }
            r
        }};
    }
    while i < chars.len() {
        let c = chars[i];
        match c {
            '/' if chars.get(i + 1) == Some(&'*') => {
                let start = i + 2;
                let mut end = start;
                while end < chars.len() && !(chars[end] == '*' && chars.get(end + 1) == Some(&'/'))
                {
                    end += 1;
                }
                let comment: String = chars[start..end.min(chars.len())].iter().collect();
                let comment = collapse_ws(&comment);
                if let Some((b, v)) = last.take() {
                    if !comment.is_empty() {
                        blocks[b].vars[v].label = Some(comment);
                    }
                } else if !buf.trim().is_empty() {
                    inline = Some(comment);
                }
                buf.push(' ');
                i = end + 2;
                continue;
            }
            '"' | '\'' => {
                last = None;
                inline = None;
                buf.push(c);
                i += 1;
                while i < chars.len() && chars[i] != c && chars[i] != '\n' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        buf.push(chars[i]);
                        i += 1;
                    }
                    buf.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() && chars[i] == c {
                    buf.push(c);
                    i += 1;
                }
                continue;
            }
            '\\' => {
                last = None;
                inline = None;
                buf.push(c);
                if let Some(&n) = chars.get(i + 1) {
                    buf.push(n);
                    i += 1;
                }
            }
            '(' => {
                parens += 1;
                last = None;
                inline = None;
                buf.push(c);
            }
            ')' => {
                parens = parens.saturating_sub(1);
                last = None;
                inline = None;
                buf.push(c);
            }
            '\n' | '\r' => {
                last = None;
                buf.push(' ');
            }
            '{' if parens == 0 => {
                let own = collapse_ws(&buf);
                buf.clear();
                last = None;
                inline = None;
                let prefix = stack.last().map(|s| s.1.clone()).unwrap_or_default();
                let selector = collapse_ws(&format!("{prefix} {own}"));
                let child_prefix = if own.starts_with('@') {
                    selector.clone()
                } else {
                    prefix
                };
                let inner = if own.starts_with('@') {
                    String::new()
                } else {
                    own.clone()
                };
                blocks.push(Builder::new(selector, inner));
                stack.push((blocks.len() - 1, child_prefix));
            }
            '}' if parens == 0 => {
                finish!();
                stack.pop();
                last = None;
            }
            ';' if parens == 0 => {
                last = finish!();
            }
            _ => {
                if !c.is_whitespace() {
                    last = None;
                    inline = None;
                }
                buf.push(c);
            }
        }
        i += 1;
    }
    finish!();
    blocks
        .into_iter()
        .filter(|b| !b.vars.is_empty() || b.skipped > 0)
        .map(|b| CssBlock {
            selector: b.selector,
            inner_selector: b.inner,
            vars: b.vars,
            skipped: b.skipped,
        })
        .collect()
}

/// Stores the block's own selector (never an at-rule prelude) on `p`.
fn set_selector(p: &mut Palette, b: &CssBlock) {
    if !b.inner_selector.is_empty() {
        p.selector = b.inner_selector.clone();
    }
}

pub fn palette_from_block(name: &str, b: &CssBlock) -> Palette {
    let mut p = Palette::new(name);
    set_selector(&mut p, b);
    p.colors = b
        .vars
        .iter()
        .map(|v| PaletteColor {
            color: v.color,
            label: v.label.clone().unwrap_or_default(),
            var_name: v.name.clone(),
        })
        .collect();
    p
}

pub fn merge_into(p: &mut Palette, b: &CssBlock) -> MergeSummary {
    let mut s = MergeSummary {
        skipped: b.skipped,
        ..Default::default()
    };
    for v in &b.vars {
        match p.index_of_var(&v.name) {
            Some(i) => {
                p.colors[i].color = v.color;
                if let Some(l) = &v.label {
                    p.colors[i].label = l.clone();
                }
                s.updated += 1;
            }
            None => {
                p.push(PaletteColor {
                    color: v.color,
                    label: v.label.clone().unwrap_or_default(),
                    var_name: v.name.clone(),
                });
                s.added += 1;
            }
        }
    }
    s
}

pub fn merge_preview(p: &Palette, b: &CssBlock) -> MergeSummary {
    merge_into(&mut p.clone(), b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(rgb: [u8; 3], label: &str, var: &str) -> PaletteColor {
        PaletteColor {
            color: Color::from_srgb8(rgb),
            label: label.into(),
            var_name: var.into(),
        }
    }

    fn sample() -> Palette {
        let mut p = Palette::new("p");
        p.push(pc([0x1B, 0x2A, 0x4A], "Navy", "navy"));
        p.push(pc([0x34, 0x78, 0xF6], "Primary blue", "primary"));
        p.push(pc([0xF6, 0xB2, 0x34], "", ""));
        p
    }

    #[test]
    fn export_matches_spec() {
        let mut p = sample();
        let o = FormatOptions::default();
        assert_eq!(
            export_css(&p, Format::Hex, ":root", &o),
            ":root {\n  --navy: #1B2A4A; /* Navy */\n  --primary: #3478F6; /* Primary blue */\n  --color-3: #F6B234;\n}\n"
        );
        p.push(pc([1, 2, 3], "", "color-3"));
        let s = export_css(&p, Format::Hex, "[data-theme=\"dark\"]", &o);
        assert!(s.starts_with("[data-theme=\"dark\"] {\n"));
        assert!(s.contains("--color-3-2: #F6B234;"));
        assert!(s.contains("--color-3: #010203;"));
    }

    #[test]
    fn export_ignores_target_and_escapes_comment() {
        let mut p = Palette::new("p");
        p.push(pc([0x34, 0x78, 0xF6], "a */ b", "x"));
        let o = FormatOptions::default();
        assert_eq!(
            export_css(&p, Format::Hex, ":root", &o),
            ":root {\n  --x: #3478F6; /* a * / b */\n}\n"
        );
        let css = export_css(&p, Format::CssColor, ":root", &o);
        assert!(css.contains("--x: color(display-p3 "), "{css}");
    }

    #[test]
    fn import_rules() {
        let text = ":root{ --a: #fff; /* White */ --gap: 4px; --b: var(--a); --c: rgb(0 0 0); --d: nonsense; }\n[data-theme=\"dark\"] { --a: #000; }";
        let blocks = parse_css_blocks(text);
        assert_eq!(blocks.len(), 2);
        let b = &blocks[0];
        assert_eq!(b.selector, ":root");
        assert_eq!(b.vars.len(), 2);
        assert_eq!(b.vars[0].name, "a");
        assert_eq!(b.vars[0].label.as_deref(), Some("White"));
        assert_eq!(b.vars[1].name, "c");
        assert_eq!(b.vars[1].label, None);
        assert_eq!(b.skipped, 3);
        assert_eq!(blocks[1].selector, "[data-theme=\"dark\"]");
        assert_eq!(blocks[1].vars[0].color.to_srgb8(), [0, 0, 0]);
    }

    #[test]
    fn import_edge_cases() {
        let text = "--top: red;\n.x { color: red; } /* c */\n.y { /* lead */ --a: #111;\n --a: #222; /* l1 */\n--e: #333\n; }\n.z { --q: #444;\n /* next line */ }";
        let blocks = parse_css_blocks(text);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].selector, "");
        assert_eq!(blocks[0].vars[0].name, "top");
        let y = &blocks[1];
        assert_eq!(y.vars[0].color.to_srgb8(), [0x22; 3]);
        assert_eq!(y.vars[0].label.as_deref(), Some("l1"));
        assert_eq!(y.vars[1].label, None);
        assert_eq!(blocks[2].vars[0].label, None);
    }

    fn names(b: &CssBlock) -> Vec<(&str, Option<&str>)> {
        b.vars
            .iter()
            .map(|v| (v.name.as_str(), v.label.as_deref()))
            .collect()
    }

    #[test]
    fn trailing_comment_attaches_to_its_own_declaration() {
        let blocks = parse_css_blocks(
            ":root { --a: #fff !important; --b: #000 /* c */; --c: #123 /* t */ }",
        );
        assert_eq!(
            names(&blocks[0]),
            [("a", None), ("b", Some("c")), ("c", Some("t"))]
        );
        assert_eq!(blocks[0].vars[0].color.to_srgb8(), [255; 3]);
        let blocks = parse_css_blocks(":root { --a: #fff; --b: #000; /* only b */ }");
        assert_eq!(names(&blocks[0]), [("a", None), ("b", Some("only b"))]);
    }

    #[test]
    fn structure_chars_in_comments_and_strings_are_opaque() {
        let b = parse_css_blocks(":root { /* } ; { */ --a: #111; /* x }\n y ; */ }");
        assert_eq!(b.len(), 1);
        assert_eq!(names(&b[0]), [("a", Some("x } y ;"))]);
        let b = parse_css_blocks("a[title=\"x}y\"] { --a:#111 }");
        assert_eq!(b[0].selector, "a[title=\"x}y\"]");
        assert_eq!(names(&b[0]), [("a", None)]);
        let b = parse_css_blocks(
            ".x { --u: url(\"a/*b\"); --c:#333 } .y{--d:#444} .z { --e: 'it\\'s ; }'; --f: #555 }",
        );
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].skipped, 1);
        assert_eq!(names(&b[0]), [("c", None)]);
        assert_eq!(b[1].selector, ".y");
        assert_eq!(names(&b[2]), [("f", None)]);
    }

    #[test]
    fn crlf_media_nesting_and_missing_final_semicolon() {
        let css = ":root { --a: #111; /* multi\r\n line\tlabel */\r\n  --b: #222\r\n}\r\n@media (prefers-color-scheme: dark) {\r\n  :root { --a: #eee }\r\n}\r\n";
        let b = parse_css_blocks(css);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].selector, ":root");
        assert_eq!(names(&b[0]), [("a", Some("multi line label")), ("b", None)]);
        assert_eq!(b[1].selector, "@media (prefers-color-scheme: dark) :root");
        assert_eq!(b[1].vars[0].color.to_srgb8(), [0xEE; 3]);
    }

    #[test]
    fn at_rule_prelude_is_not_stored_as_selector() {
        let css = "@media (prefers-color-scheme: dark) { .dark { --a: #eee } :root { --b: #111 } }\n@font-face { --c: #222 }";
        let b = parse_css_blocks(css);
        assert_eq!(b[0].selector, "@media (prefers-color-scheme: dark) .dark");
        assert_eq!(b[0].inner_selector, ".dark");
        let p = palette_from_block("p", &b[0]);
        assert_eq!(p.selector, ".dark");
        assert_eq!(palette_from_block("p", &b[2]).selector, ":root");
        assert!(
            export_css(&p, Format::Hex, &p.selector, &FormatOptions::default())
                .starts_with(".dark {")
        );
    }

    #[test]
    fn merge_rules() {
        let mut p = Palette::new("p");
        p.push(pc([0x11; 3], "Old", "a"));
        p.push(pc([0x22; 3], "", "z"));
        let mut b = CssBlock {
            selector: ":root".into(),
            inner_selector: ":root".into(),
            vars: vec![
                CssVar {
                    name: "a".into(),
                    color: Color::from_srgb8([255; 3]),
                    label: None,
                },
                CssVar {
                    name: "n".into(),
                    color: Color::from_srgb8([0; 3]),
                    label: None,
                },
            ],
            skipped: 0,
        };
        let preview = merge_preview(&p, &b);
        assert_eq!(p.colors.len(), 2);
        let s = merge_into(&mut p.clone(), &b);
        assert_eq!(preview, s);
        let s = merge_into(&mut p, &b);
        assert_eq!((s.updated, s.added, s.skipped), (1, 1, 0));
        assert_eq!(p.colors[0].color.to_srgb8(), [255; 3]);
        assert_eq!(p.colors[0].label, "Old");
        assert_eq!(p.colors[1].color.to_srgb8(), [0x22; 3]);
        assert_eq!(p.colors[2].var_name, "n");
        b.vars[0].label = Some("New".into());
        merge_into(&mut p, &b);
        assert_eq!(p.colors[0].label, "New");
    }

    #[test]
    fn every_css_format_round_trips() {
        use crate::format::{CssSyntax, HexCase};
        let mut p = sample();
        p.push(pc([0, 0, 0], "Black", "black"));
        p.push(pc([255, 255, 255], "", "white"));
        p.push(pc([0x80, 0x10, 0xE0], "Purple */ x", "purple"));
        let offered: Vec<Format> = Format::ALL.into_iter().filter(|f| f.is_css()).collect();
        assert!(!offered.contains(&Format::Hsv));
        assert_eq!(offered.len(), Format::ALL.len() - 1);
        for format in offered {
            // 8-bit units the format's printed precision may lose: the
            // worst case over the 8-bit cube (sampled every 3rd value). The
            // OK* errors come from channels near 0, where the sRGB curve is
            // steepest.
            let tolerance = match format {
                Format::Hsl => 5,
                Format::Oklch | Format::Oklab => 10,
                _ => 0,
            };
            for css_syntax in [CssSyntax::Modern, CssSyntax::Legacy] {
                let o = FormatOptions {
                    hex_case: HexCase::Lower,
                    css_syntax,
                    ..Default::default()
                };
                let css = export_css(&p, format, ":root", &o);
                let blocks = parse_css_blocks(&css);
                assert_eq!(blocks.len(), 1, "{css}");
                assert_eq!(blocks[0].skipped, 0, "{css}");
                let q = palette_from_block("q", &blocks[0]);
                assert_eq!(q.colors.len(), p.colors.len(), "{css}");
                for (a, b) in p.colors.iter().zip(&q.colors) {
                    let want = if a.var_name.is_empty() {
                        "color-3"
                    } else {
                        &a.var_name
                    };
                    assert_eq!(b.var_name, want, "{css}");
                    assert_eq!(a.label.replace("*/", "* /"), b.label, "{css}");
                    let (x, y) = (a.color.to_srgb8(), b.color.to_srgb8());
                    for (u, v) in x.iter().zip(y) {
                        assert!(
                            u.abs_diff(v) <= tolerance,
                            "{format:?}: {x:?} vs {y:?}\n{css}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn export_never_writes_hsv() {
        let css = export_css(&sample(), Format::Hsv, ":root", &FormatOptions::default());
        assert!(!css.contains("hsv"), "{css}");
        assert!(css.contains("--primary: #3478F6;"), "{css}");
    }

    #[test]
    fn css_round_trip() {
        let mut p = sample();
        p.push(pc([0, 0, 0], "Black", "black"));
        p.push(pc([255, 255, 255], "", "white"));
        let css = export_css(&p, Format::Hex, ":root", &FormatOptions::default());
        let blocks = parse_css_blocks(&css);
        assert_eq!(blocks.len(), 1);
        let q = palette_from_block("q", &blocks[0]);
        assert_eq!(q.colors.len(), p.colors.len());
        for (a, b) in p.colors.iter().zip(&q.colors) {
            assert_eq!(a.label, b.label);
            assert_eq!(a.color.to_srgb8(), b.color.to_srgb8());
        }
        let names: Vec<&str> = q.colors.iter().map(|c| c.var_name.as_str()).collect();
        assert_eq!(names, ["navy", "primary", "color-3", "black", "white"]);
    }
}
