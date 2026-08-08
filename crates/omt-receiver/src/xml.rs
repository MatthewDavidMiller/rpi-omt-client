// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Bounded XML reads for the two documents the receiver consumes: the storage
// settings file and the discovery server's source announcements. Both arrive
// from outside the appliance, so a document type declaration, an entity
// definition, or a repeated element is rejected rather than interpreted.

use quick_xml::Reader;
use quick_xml::events::Event;

/// Largest document either caller will read.
pub const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub enum XmlError {
    Malformed,
    Unsupported,
    NotFound,
    Duplicate,
}

/// Returns the decoded text of the single element named `tag`.
///
/// A document containing the tag more than once is rejected: the announcement
/// format gives each field once, and a duplicate is how a crafted document
/// would try to make two readers disagree.
pub fn unique_text(document: &str, tag: &str) -> Result<String, XmlError> {
    let [found] = unique_texts(document, &[tag])?;
    found.ok_or(XmlError::NotFound)
}

/// Returns the decoded text of each named element, in one pass.
///
/// The discovery stream wants four fields off every announcement, and a reader
/// per field walked the whole document four times -- over a thousand parses for
/// the 256 sources the receiver will track. Every rejection is unchanged and
/// applies to the document as a whole: a duplicate of *any* requested tag, a
/// document type declaration, a processing instruction, an unknown entity, more
/// than 32 levels of nesting, or an oversized document refuses all of them
/// together, so no caller can read a field out of a document another caller
/// would have thrown away.
///
/// A tag that is simply absent is `None` rather than an error, because callers
/// differ on which of their fields are optional.
pub fn unique_texts<const N: usize>(
    document: &str,
    tags: &[&str; N],
) -> Result<[Option<String>; N], XmlError> {
    if document.len() > MAX_DOCUMENT_BYTES {
        return Err(XmlError::Unsupported);
    }
    let mut reader = Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut depth = 0_usize;
    // Which of `tags` is being captured, and the text collected for it so far.
    let mut capturing: Option<usize> = None;
    let mut found: [Option<String>; N] = std::array::from_fn(|_| None);
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::DocType(_) | Event::PI(_)) => return Err(XmlError::Unsupported),
            Ok(Event::Start(element)) => {
                depth += 1;
                if depth > 32 {
                    return Err(XmlError::Unsupported);
                }
                let name = element.name();
                if let Some(index) = tags.iter().position(|tag| name.as_ref() == tag.as_bytes()) {
                    if found[index].is_some() || capturing.is_some() {
                        return Err(XmlError::Duplicate);
                    }
                    capturing = Some(index);
                    text.clear();
                }
            }
            Ok(Event::Text(chunk)) if capturing.is_some() => {
                let decoded = chunk.decode().map_err(|_| XmlError::Malformed)?;
                if text.len() + decoded.len() > MAX_DOCUMENT_BYTES {
                    return Err(XmlError::Unsupported);
                }
                text.push_str(&decoded);
            }
            // The reader surfaces every entity reference separately. Only the
            // five predefined names are resolved; anything else would need a
            // document type declaration, which is refused above.
            Ok(Event::GeneralRef(reference)) if capturing.is_some() => {
                let name = reference.decode().map_err(|_| XmlError::Malformed)?;
                let resolved = match name.as_ref() {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    _ => return Err(XmlError::Malformed),
                };
                if text.len() + 1 > MAX_DOCUMENT_BYTES {
                    return Err(XmlError::Unsupported);
                }
                text.push(resolved);
            }
            Ok(Event::End(element)) => {
                depth = depth.saturating_sub(1);
                if let Some(index) = capturing
                    && element.name().as_ref() == tags[index].as_bytes()
                {
                    capturing = None;
                    found[index] = Some(std::mem::take(&mut text));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(XmlError::Malformed),
        }
    }
    Ok(found)
}

/// True when the given element is present with the given value.
#[must_use]
pub fn element_is(text: Option<&str>, value: &str) -> bool {
    text.is_some_and(|text| text.eq_ignore_ascii_case(value))
}

/// Confirms the document's single root element is named `tag`.
pub fn root_is(document: &str, tag: &str) -> Result<(), XmlError> {
    let mut reader = Reader::from_str(document);
    loop {
        match reader.read_event() {
            Ok(Event::DocType(_)) => return Err(XmlError::Unsupported),
            Ok(Event::Start(element) | Event::Empty(element)) => {
                return if element.name().as_ref() == tag.as_bytes() {
                    Ok(())
                } else {
                    Err(XmlError::Unsupported)
                };
            }
            Ok(Event::Eof) => return Err(XmlError::NotFound),
            Ok(_) => {}
            Err(_) => return Err(XmlError::Malformed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_decodes_a_unique_element() {
        let document =
            "<OMTAddress><Name>STUDIO (Camera &amp; One)</Name><Port>6400</Port></OMTAddress>";
        assert_eq!(
            unique_text(document, "Name").as_deref(),
            Ok("STUDIO (Camera & One)")
        );
        assert_eq!(unique_text(document, "Port").as_deref(), Ok("6400"));
        assert_eq!(unique_text(document, "Missing"), Err(XmlError::NotFound));
    }

    #[test]
    fn rejects_duplicated_and_declared_documents() {
        assert_eq!(
            unique_text("<a><Name>x</Name><Name>y</Name></a>", "Name"),
            Err(XmlError::Duplicate)
        );
        assert_eq!(
            unique_text(
                "<!DOCTYPE a [<!ENTITY x \"y\">]><a><Name>&x;</Name></a>",
                "Name"
            ),
            Err(XmlError::Unsupported)
        );
        assert_eq!(
            unique_text("<a><Name>x</a>", "Name"),
            Err(XmlError::Malformed)
        );
        assert_eq!(
            unique_text("<a><Name>&unknown;</Name></a>", "Name"),
            Err(XmlError::Malformed)
        );
    }

    /// The single pass has to reach the same verdict the four separate passes
    /// did, including on the tags it was not asked about first: a duplicate of
    /// *any* requested tag refuses the whole document, so a crafted
    /// announcement cannot get one field read out of a document that another
    /// field's reader would have rejected.
    #[test]
    fn reads_every_requested_tag_in_one_pass() {
        let document = "<OMTAddress><Name>Camera &amp; Two</Name><Removed>True</Removed>\
            <Addresses><IPAddress>192.0.2.10</IPAddress></Addresses><Port>6400</Port></OMTAddress>";
        let found = unique_texts(document, &["Name", "Removed", "IPAddress", "Port"])
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(found[0].as_deref(), Some("Camera & Two"));
        assert!(element_is(found[1].as_deref(), "true"));
        assert_eq!(found[2].as_deref(), Some("192.0.2.10"));
        assert_eq!(found[3].as_deref(), Some("6400"));

        // An absent tag is None, not a failure -- only some fields are required.
        let sparse = unique_texts(
            "<OMTAddress><Name>Camera</Name></OMTAddress>",
            &["Name", "Port"],
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(sparse[0].as_deref(), Some("Camera"));
        assert_eq!(sparse[1], None);
        assert!(!element_is(None, "True"));

        // A duplicate of any one of them refuses all of them.
        for duplicated in [
            "<a><Name>x</Name><Name>y</Name><Port>1</Port></a>",
            "<a><Name>x</Name><Port>1</Port><Port>2</Port></a>",
        ] {
            assert_eq!(
                unique_texts(duplicated, &["Name", "Port"]),
                Err(XmlError::Duplicate),
                "{duplicated}"
            );
        }
        // And so does a declaration, however far from the requested tags.
        assert_eq!(
            unique_texts("<!DOCTYPE a><a><Name>x</Name></a>", &["Name", "Port"]),
            Err(XmlError::Unsupported)
        );
    }

    #[test]
    fn checks_the_document_root() {
        assert_eq!(root_is("<Settings><A>1</A></Settings>", "Settings"), Ok(()));
        assert_eq!(
            root_is("<?xml version=\"1.0\"?><Settings/>", "Settings"),
            Ok(())
        );
        assert_eq!(root_is("<Other/>", "Settings"), Err(XmlError::Unsupported));
    }
}
