// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! Just enough XML to read the small files other programs write for their own
//! use: Snapper's `info.xml` (Stage 17) and GNOME's timed wallpaper slideshows
//! (Stage 18). Each is a handful of elements holding text, written by a
//! program rather than a person, and a parser of a few dozen lines reads them
//! without a dependency. It is not a general XML parser: no namespaces, no
//! CDATA, and `children` assumes an element does not hold another of its own
//! name; `tags` reads a file whose elements do, such as Xfce's settings
//! (S18-T19), one tag at a time.

/// What is between `<name>` and `</name>`, the first such element in `text`.
pub fn element<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&format!("</{name}>"))?;
    Some(&text[start..end])
}

/// An element: its name, what is in its tag after the name, and what is
/// between its tags, still escaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element<'a> {
    pub name: &'a str,
    attributes: &'a str,
    pub body: &'a str,
}

impl Element<'_> {
    /// The value of the attribute `name`, unescaped; None when the tag does
    /// not have it, or its value cannot be read.
    pub fn attribute(&self, name: &str) -> Option<String> {
        attribute_in(self.attributes, name)
    }
}

fn attribute_in(attributes: &str, name: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let value_end = after[1..].find(quote)? + 1;
        if key == name {
            return unescape(&after[1..value_end]);
        }
        rest = &after[value_end + 1..];
    }
}

/// Whether a tag opens an element, closes one, or is one: `<a>`, `</a>`,
/// `<a/>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    Open,
    Close,
    Empty,
}

/// A tag, as `tags` meets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag<'a> {
    pub name: &'a str,
    pub kind: TagKind,
    attributes: &'a str,
}

impl Tag<'_> {
    /// As `Element::attribute`.
    pub fn attribute(&self, name: &str) -> Option<String> {
        attribute_in(self.attributes, name)
    }
}

/// Every tag in `text`, in order, the declaration and comments passed over:
/// for a file whose elements hold others of their own name, read by keeping
/// track of where each opens and closes.
pub fn tags(text: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut rest = text;
    std::iter::from_fn(move || loop {
        let open = rest.find('<')?;
        rest = &rest[open + 1..];
        if let Some(after) = rest.strip_prefix("!--") {
            rest = &after[after.find("-->")? + 3..];
            continue;
        }
        let tag_end = rest.find('>')?;
        let tag = &rest[..tag_end];
        rest = &rest[tag_end + 1..];
        if tag.starts_with(['?', '!']) {
            continue;
        }
        let (tag, kind) = if let Some(name) = tag.strip_prefix('/') {
            (name, TagKind::Close)
        } else if let Some(tag) = tag.strip_suffix('/') {
            (tag, TagKind::Empty)
        } else {
            (tag, TagKind::Open)
        };
        let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let (name, attributes) = tag.split_at(name_end);
        return Some(Tag {
            name: name.trim(),
            kind,
            attributes,
        });
    })
}

/// The elements directly inside `body`, in order; an element written
/// `<name/>` has an empty body. Anything that is not an element (the
/// whitespace between them, a comment) is passed over.
pub fn children(body: &str) -> impl Iterator<Item = Element<'_>> {
    let mut rest = body;
    std::iter::from_fn(move || loop {
        let open = rest.find('<')?;
        rest = &rest[open + 1..];
        if let Some(after) = rest.strip_prefix("!--") {
            rest = &after[after.find("-->")? + 3..];
            continue;
        }
        let tag_end = rest.find('>')?;
        let tag = &rest[..tag_end];
        rest = &rest[tag_end + 1..];
        if tag.starts_with(['/', '?', '!']) {
            continue;
        }
        let (tag, closed) = match tag.strip_suffix('/') {
            Some(tag) => (tag, true),
            None => (tag, false),
        };
        let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let (name, attributes) = tag.split_at(name_end);
        if closed {
            return Some(Element {
                name,
                attributes,
                body: "",
            });
        }
        let close = format!("</{name}>");
        let end = rest.find(&close)?;
        let body = &rest[..end];
        rest = &rest[end + close.len()..];
        return Some(Element {
            name,
            attributes,
            body,
        });
    })
}

/// XML's escapes undone: the five named ones and character references, which
/// libxml2 writes for every character outside ASCII when the file names no
/// encoding. None for any other `&`.
pub fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let end = rest.find(';')?;
        let entity = &rest[..end];
        rest = &rest[end + 1..];
        let c = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = match entity.strip_prefix("#x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => entity.strip_prefix('#')?.parse().ok()?,
                };
                char::from_u32(code)?
            }
        };
        out.push(c);
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_are_read_in_order_past_comments_and_declarations() {
        let body = r#"<?xml version="1.0"?>
            <!-- a comment <with> a tag in it -->
            <num>3</num>
            <empty/>
            <file>a &amp; b</file>"#;
        let names: Vec<(&str, &str)> = children(body).map(|e| (e.name, e.body)).collect();
        assert_eq!(names, [("num", "3"), ("empty", ""), ("file", "a &amp; b")]);
    }

    #[test]
    fn an_attribute_is_read_whichever_quotes_it_has() {
        let body = r#"<size width="1920" height='1080'>/a.png</size><transition type="overlay"/>"#;
        let mut all = children(body);
        let size = all.next().unwrap();
        assert_eq!((size.name, size.body), ("size", "/a.png"));
        assert_eq!(size.attribute("width").as_deref(), Some("1920"));
        assert_eq!(size.attribute("height").as_deref(), Some("1080"));
        assert_eq!(size.attribute("depth"), None);
        let transition = all.next().unwrap();
        assert_eq!(transition.name, "transition");
        assert_eq!(transition.attribute("type").as_deref(), Some("overlay"));
    }

    #[test]
    fn tags_follow_elements_that_hold_their_own_name() {
        let text = r#"<?xml version="1.0"?>
<channel name="d"><property name="a" type="empty"><!-- note -->
<property name="b" type="string" value="x &amp; y"/></property></channel>"#;
        let seen: Vec<(&str, TagKind, Option<String>)> = tags(text)
            .map(|tag| (tag.name, tag.kind, tag.attribute("name")))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("channel", TagKind::Open, Some("d".into())),
                ("property", TagKind::Open, Some("a".into())),
                ("property", TagKind::Empty, Some("b".into())),
                ("property", TagKind::Close, None),
                ("channel", TagKind::Close, None),
            ]
        );
        let value = tags(text).find(|t| t.kind == TagKind::Empty).unwrap();
        assert_eq!(value.attribute("value").as_deref(), Some("x & y"));
    }

    #[test]
    fn escapes_are_undone_and_an_unknown_one_refuses() {
        assert_eq!(
            unescape("&lt;&#x2014;&#38;&gt;").as_deref(),
            Some("<\u{2014}&>")
        );
        assert_eq!(unescape("&nbsp;"), None);
    }
}
