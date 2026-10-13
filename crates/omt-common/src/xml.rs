//! Bounded XML for the documents this project reads from outside the
//! appliance: the OMT storage settings file and discovery-server
//! announcements.
//!
//! This is a pull tokenizer, not a parser of the whole language: a document
//! type declaration, a processing instruction, an entity other than the five
//! predefined ones, or a mismatched end tag is surfaced so the caller can
//! refuse it, and nothing is ever expanded.

pub const MAX_DOCUMENT: usize = 64 * 1024;
pub const MAX_DEPTH: usize = 32;
const STACK: usize = 64;

/// Why a document was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Malformed,
    Unsupported,
    NotFound,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// `<name ...>`
    Start,
    /// `<name .../>`
    Empty,
    /// `</name>`
    End,
    /// Character data between markup and references.
    Text,
    /// `&name;` -- `name` holds what is between `&` and `;`.
    Ref,
    /// `<?xml ...?>`
    Decl,
    /// Any other `<?...?>`.
    Pi,
    /// `<!DOCTYPE ...>`
    Doctype,
    /// `<!-- ... -->`
    Comment,
    /// `<![CDATA[ ... ]]>`
    Cdata,
    Eof,
}

#[derive(Clone, Copy, Debug)]
pub struct Event<'a> {
    pub kind: EventKind,
    /// Element or reference name.
    pub name: &'a [u8],
    /// Text, attributes, or the body of a markup construct.
    pub content: &'a [u8],
}

#[derive(Debug)]
pub struct Reader<'a> {
    doc: &'a [u8],
    pos: usize,
    open: Vec<&'a [u8]>,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

impl<'a> Reader<'a> {
    pub fn new(doc: &'a [u8]) -> Self {
        Self { doc, pos: 0, open: Vec::new() }
    }

    /// The offset of the next unread byte: where the next event starts, or
    /// just past the one returned last.
    pub fn position(&self) -> usize {
        self.pos
    }

    fn rest(&self) -> &'a [u8] {
        &self.doc[self.pos..]
    }

    /// The offset of `terminator` at or after `from`, or the document length.
    fn find(&self, from: usize, terminator: &[u8]) -> usize {
        self.doc[from..]
            .windows(terminator.len())
            .position(|w| w == terminator)
            .map_or(self.doc.len(), |i| from + i)
    }

    fn event(kind: EventKind, name: &'a [u8], content: &'a [u8]) -> Event<'a> {
        Event { kind, name, content }
    }

    /// The next event. Mismatched and unmatched end tags are malformed;
    /// nesting past 64 open elements is unsupported.
    pub fn next_event(&mut self) -> Result<Event<'a>, Status> {
        let doc = self.doc;
        let Some(&c) = doc.get(self.pos) else {
            return Ok(Self::event(EventKind::Eof, b"", b""));
        };
        if c == b'&' {
            let start = self.pos + 1;
            let mut i = start;
            while i < doc.len() && !matches!(doc[i], b';' | b'&' | b'<') && !is_space(doc[i]) {
                i += 1;
            }
            if i >= doc.len() || doc[i] != b';' || i == start {
                return Err(Status::Malformed);
            }
            self.pos = i + 1;
            return Ok(Self::event(EventKind::Ref, &doc[start..i], b""));
        }
        if c != b'<' {
            let start = self.pos;
            while self.pos < doc.len() && !matches!(doc[self.pos], b'<' | b'&') {
                self.pos += 1;
            }
            return Ok(Self::event(EventKind::Text, b"", &doc[start..self.pos]));
        }
        let rest = self.rest();
        if rest.starts_with(b"<?") {
            let body = self.pos + 2;
            let end = self.find(body, b"?>");
            if end >= doc.len() {
                return Err(Status::Malformed);
            }
            let inner = &doc[body..end];
            let decl = inner.starts_with(b"xml") && (inner.len() == 3 || is_space(inner[3]));
            self.pos = end + 2;
            let kind = if decl { EventKind::Decl } else { EventKind::Pi };
            return Ok(Self::event(kind, b"", inner));
        }
        for (open, close, kind) in [
            (&b"<!--"[..], &b"-->"[..], EventKind::Comment),
            (b"<![CDATA[", b"]]>", EventKind::Cdata),
        ] {
            if rest.starts_with(open) {
                let body = self.pos + open.len();
                let end = self.find(body, close);
                if end >= doc.len() {
                    return Err(Status::Malformed);
                }
                self.pos = end + close.len();
                return Ok(Self::event(kind, b"", &doc[body..end]));
            }
        }
        if rest.len() >= 9 && rest[..9].eq_ignore_ascii_case(b"<!DOCTYPE") {
            // Skip to the matching '>', honouring an internal subset in [...].
            let mut i = self.pos + 9;
            let mut brackets = 0_i64;
            let mut quote = 0_u8;
            while i < doc.len() {
                let d = doc[i];
                if quote != 0 {
                    if d == quote {
                        quote = 0;
                    }
                } else if d == b'"' || d == b'\'' {
                    quote = d;
                } else if d == b'[' {
                    brackets += 1;
                } else if d == b']' {
                    brackets -= 1;
                } else if d == b'>' && brackets <= 0 {
                    break;
                }
                i += 1;
            }
            if i >= doc.len() {
                return Err(Status::Malformed);
            }
            let content = &doc[self.pos + 9..i];
            self.pos = i + 1;
            return Ok(Self::event(EventKind::Doctype, b"", content));
        }
        if rest.starts_with(b"<!") {
            return Err(Status::Malformed);
        }

        let closing = rest.starts_with(b"</");
        let name_start = self.pos + if closing { 2 } else { 1 };
        let mut i = name_start;
        while i < doc.len() && !is_space(doc[i]) && doc[i] != b'>' && doc[i] != b'/' {
            i += 1;
        }
        let name_end = i;
        // Find the end of the tag, skipping '>' inside quoted attribute values.
        let mut quote = 0_u8;
        while i < doc.len() {
            let d = doc[i];
            if quote != 0 {
                if d == quote {
                    quote = 0;
                }
            } else if d == b'"' || d == b'\'' {
                quote = d;
            } else if d == b'>' {
                break;
            }
            i += 1;
        }
        if i >= doc.len() {
            return Err(Status::Malformed);
        }
        let name = &doc[name_start..name_end];
        if name.is_empty() {
            return Err(Status::Malformed);
        }
        if closing {
            if !doc[name_end..i].iter().all(|&b| is_space(b)) || self.open.last() != Some(&name) {
                return Err(Status::Malformed);
            }
            self.open.pop();
            self.pos = i + 1;
            return Ok(Self::event(EventKind::End, name, b""));
        }
        let empty = i > name_end && doc[i - 1] == b'/';
        let content = &doc[name_end..if empty { i - 1 } else { i }];
        self.pos = i + 1;
        if empty {
            return Ok(Self::event(EventKind::Empty, name, content));
        }
        if self.open.len() >= STACK {
            return Err(Status::Unsupported);
        }
        self.open.push(name);
        Ok(Self::event(EventKind::Start, name, content))
    }
}

/// Resolves one of the five predefined entity names.
pub fn predefined_entity(name: &[u8]) -> Option<u8> {
    match name {
        b"amp" => Some(b'&'),
        b"lt" => Some(b'<'),
        b"gt" => Some(b'>'),
        b"quot" => Some(b'"'),
        b"apos" => Some(b'\''),
        _ => None,
    }
}

/// Reads the decoded text of each named element in one pass; entry `i` is
/// the text of `tags[i]`, or `None` when it did not appear complete.
///
/// A duplicate of any requested tag, a declaration, a processing
/// instruction, an unknown entity inside a captured element, nesting past
/// [`MAX_DEPTH`], or a document over [`MAX_DOCUMENT`] refuses all of them
/// together.
pub fn unique_texts(doc: &[u8], tags: &[&str]) -> Result<Vec<Option<Vec<u8>>>, Status> {
    let mut out: Vec<Option<Vec<u8>>> = vec![None; tags.len()];
    if doc.len() > MAX_DOCUMENT {
        return Err(Status::Unsupported);
    }
    let mut reader = Reader::new(doc);
    let mut depth = 0;
    let mut capturing: Option<(usize, Vec<u8>)> = None;
    loop {
        let ev = reader.next_event()?;
        match ev.kind {
            EventKind::Eof => break,
            EventKind::Doctype | EventKind::Pi => return Err(Status::Unsupported),
            EventKind::Start => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Status::Unsupported);
                }
                if let Some(i) = tags.iter().position(|t| t.as_bytes() == ev.name) {
                    if out[i].is_some() || capturing.is_some() {
                        return Err(Status::Duplicate);
                    }
                    capturing = Some((i, Vec::new()));
                }
            }
            EventKind::Text => {
                if let Some((_, text)) = capturing.as_mut() {
                    text.extend_from_slice(ev.content);
                    if text.len() > MAX_DOCUMENT {
                        return Err(Status::Unsupported);
                    }
                }
            }
            EventKind::Ref => {
                if let Some((_, text)) = capturing.as_mut() {
                    text.push(predefined_entity(ev.name).ok_or(Status::Malformed)?);
                }
            }
            EventKind::End => {
                depth = depth.saturating_sub(1);
                if let Some((i, text)) = capturing.take_if(|(i, _)| tags[*i].as_bytes() == ev.name)
                {
                    out[i] = Some(text);
                }
            }
            _ => {}
        }
    }
    // An element still open at the end of input was never completed.
    Ok(out)
}

/// The decoded text of the one element named `tag`.
pub fn unique_text(doc: &[u8], tag: &str) -> Result<Vec<u8>, Status> {
    unique_texts(doc, &[tag])?.pop().flatten().ok_or(Status::NotFound)
}

/// True when the element text is present and equals `value`, ignoring ASCII
/// case.
pub fn element_is(text: Option<&[u8]>, value: &str) -> bool {
    text.is_some_and(|t| t.eq_ignore_ascii_case(value.as_bytes()))
}

/// Confirms the document's first element is named `tag`.
pub fn root_is(doc: &[u8], tag: &str) -> Result<(), Status> {
    let mut reader = Reader::new(doc);
    loop {
        let ev = reader
            .next_event()
            .map_err(|s| if s == Status::Unsupported { s } else { Status::Malformed })?;
        match ev.kind {
            EventKind::Doctype => return Err(Status::Unsupported),
            EventKind::Start | EventKind::Empty => {
                return if ev.name == tag.as_bytes() { Ok(()) } else { Err(Status::Unsupported) };
            }
            EventKind::Eof => return Err(Status::NotFound),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(doc: &str, tag: &str) -> Result<String, Status> {
        unique_text(doc.as_bytes(), tag).map(|t| String::from_utf8(t).unwrap())
    }

    #[test]
    fn reads_and_decodes_a_unique_element() {
        let doc =
            "<OMTAddress><Name>STUDIO (Camera &amp; One)</Name><Port>6400</Port></OMTAddress>";
        assert_eq!(text(doc, "Name").as_deref(), Ok("STUDIO (Camera & One)"));
        assert_eq!(text(doc, "Port").as_deref(), Ok("6400"));
        assert_eq!(text(doc, "Missing"), Err(Status::NotFound));
    }

    #[test]
    fn rejects_duplicated_and_declared_documents() {
        let cases = [
            ("<a><Name>x</Name><Name>y</Name></a>", Status::Duplicate),
            ("<!DOCTYPE a [<!ENTITY x \"y\">]><a><Name>&x;</Name></a>", Status::Unsupported),
            ("<a><Name>x</a>", Status::Malformed),
            ("<a><Name>&unknown;</Name></a>", Status::Malformed),
            ("<a><Name>&#65;</Name></a>", Status::Malformed),
            ("<a><?pi x?><Name>x</Name></a>", Status::Unsupported),
            ("<a><Name>x</Name></a></b>", Status::Malformed),
            ("<a><Name>x", Status::NotFound),
            ("<a><Name", Status::Malformed),
        ];
        for (doc, status) in cases {
            assert_eq!(text(doc, "Name"), Err(status), "{doc}");
        }
    }

    #[test]
    fn reads_every_requested_tag_in_one_pass() {
        let doc = "<OMTAddress><Name>Camera &amp; Two</Name><Removed>True</Removed>\
                   <Addresses><IPAddress>192.0.2.10</IPAddress></Addresses>\
                   <Port>6400</Port></OMTAddress>";
        let tags = ["Name", "Removed", "IPAddress", "Port"];
        let out = unique_texts(doc.as_bytes(), &tags).unwrap();
        assert_eq!(out[0].as_deref(), Some(&b"Camera & Two"[..]));
        assert!(element_is(out[1].as_deref(), "true"));
        assert_eq!(out[2].as_deref(), Some(&b"192.0.2.10"[..]));
        assert_eq!(out[3].as_deref(), Some(&b"6400"[..]));
        let sparse = unique_texts(b"<OMTAddress><Name>Camera</Name></OMTAddress>", &tags).unwrap();
        assert!(sparse[0].is_some() && sparse[3].is_none());
        assert!(!element_is(sparse[1].as_deref(), "True"));
        for dup in [
            "<a><Name>x</Name><Name>y</Name><Port>1</Port></a>",
            "<a><Name>x</Name><Port>1</Port><Port>2</Port></a>",
        ] {
            assert_eq!(unique_texts(dup.as_bytes(), &["Name", "Port"]), Err(Status::Duplicate));
        }
        assert_eq!(
            unique_texts(b"<!DOCTYPE a><a><Name>x</Name></a>", &["Name", "Port"]),
            Err(Status::Unsupported)
        );
    }

    #[test]
    fn bounds_depth_and_size() {
        assert_eq!(text(&"<a>".repeat(33), "Name"), Err(Status::Unsupported));
        let big = vec![b'x'; MAX_DOCUMENT + 2];
        assert_eq!(unique_text(&big, "Name"), Err(Status::Unsupported));
    }

    #[test]
    fn checks_the_document_root() {
        assert_eq!(root_is(b"<Settings><A>1</A></Settings>", "Settings"), Ok(()));
        assert_eq!(root_is(b"<?xml version=\"1.0\"?><Settings/>", "Settings"), Ok(()));
        assert_eq!(root_is(b"<Other/>", "Settings"), Err(Status::Unsupported));
        assert_eq!(root_is(b"", "Settings"), Err(Status::NotFound));
    }

    #[test]
    fn attributes_and_comments() {
        let doc = "<a x=\"1>2\"><!-- <Name>no</Name> --><Name y='z'>v</Name><![CDATA[<Name>]]></a>";
        assert_eq!(text(doc, "Name").as_deref(), Ok("v"));
    }
}
