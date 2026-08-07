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
    if document.len() > MAX_DOCUMENT_BYTES {
        return Err(XmlError::Unsupported);
    }
    let mut reader = Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut depth = 0_usize;
    let mut capturing = false;
    let mut found: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::DocType(_) | Event::PI(_)) => return Err(XmlError::Unsupported),
            Ok(Event::Start(element)) => {
                depth += 1;
                if depth > 32 {
                    return Err(XmlError::Unsupported);
                }
                if element.name().as_ref() == tag.as_bytes() {
                    if found.is_some() || capturing {
                        return Err(XmlError::Duplicate);
                    }
                    capturing = true;
                    text.clear();
                }
            }
            Ok(Event::Text(chunk)) if capturing => {
                let decoded = chunk.decode().map_err(|_| XmlError::Malformed)?;
                if text.len() + decoded.len() > MAX_DOCUMENT_BYTES {
                    return Err(XmlError::Unsupported);
                }
                text.push_str(&decoded);
            }
            // The reader surfaces every entity reference separately. Only the
            // five predefined names are resolved; anything else would need a
            // document type declaration, which is refused above.
            Ok(Event::GeneralRef(reference)) if capturing => {
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
                if capturing && element.name().as_ref() == tag.as_bytes() {
                    capturing = false;
                    found = Some(std::mem::take(&mut text));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(XmlError::Malformed),
        }
    }
    found.ok_or(XmlError::NotFound)
}

/// True when the document contains the given element at all.
#[must_use]
pub fn contains_element(document: &str, tag: &str, value: &str) -> bool {
    unique_text(document, tag).is_ok_and(|text| text.eq_ignore_ascii_case(value))
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
