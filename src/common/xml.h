/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Bounded XML for the documents this project reads from outside the
 * appliance: the OMT storage settings file and discovery-server
 * announcements. It is a pull tokenizer, not a parser of the whole language:
 * a document type declaration, a processing instruction, an entity other than
 * the five predefined ones, or a mismatched end tag is surfaced so the caller
 * can refuse it, and nothing is ever expanded.
 */
#ifndef OMT_XML_H
#define OMT_XML_H

#include "common/base.h"
#include "common/buf.h"

#define OMT_XML_MAX_DOCUMENT (64u * 1024u)
#define OMT_XML_MAX_DEPTH 32

typedef enum {
    OMT_XML_OK = 0,
    OMT_XML_MALFORMED,
    OMT_XML_UNSUPPORTED,
    OMT_XML_NOT_FOUND,
    OMT_XML_DUPLICATE,
} omt_xml_status;

typedef enum {
    OMT_XML_EV_START,   /* <name ...> */
    OMT_XML_EV_EMPTY,   /* <name .../> */
    OMT_XML_EV_END,     /* </name> */
    OMT_XML_EV_TEXT,    /* character data between markup and references */
    OMT_XML_EV_REF,     /* &name; -- name holds what is between & and ; */
    OMT_XML_EV_DECL,    /* <?xml ...?> */
    OMT_XML_EV_PI,      /* any other <?...?> */
    OMT_XML_EV_DOCTYPE, /* <!DOCTYPE ...> */
    OMT_XML_EV_COMMENT, /* <!-- ... --> */
    OMT_XML_EV_CDATA,   /* <![CDATA[ ... ]]> */
    OMT_XML_EV_EOF,
} omt_xml_event_type;

typedef struct {
    omt_xml_event_type type;
    omt_span name;    /* element or reference name */
    omt_span content; /* text, attributes, or the body of a markup construct */
} omt_xml_event;

#define OMT_XML_STACK 64
typedef struct {
    const char *doc;
    size_t len;
    size_t pos;
    size_t depth;
    omt_span open[OMT_XML_STACK];
} omt_xml_reader;

void omt_xml_reader_init(omt_xml_reader *r, const char *doc, size_t len);
/* Returns OMT_XML_OK with the next event, or OMT_XML_MALFORMED. Mismatched
 * and unmatched end tags are malformed; nesting past OMT_XML_STACK is
 * unsupported. */
omt_xml_status omt_xml_next(omt_xml_reader *r, omt_xml_event *ev);
/* Resolves one of the five predefined entity names; -1 for anything else. */
int omt_xml_predefined_entity(omt_span name);

/* Reads the decoded text of each named element in one pass. out[i] receives
 * the text and found[i] whether the element appeared. A duplicate of any
 * requested tag, a declaration, a processing instruction, an unknown entity
 * inside a captured element, nesting past OMT_XML_MAX_DEPTH, or a document
 * over OMT_XML_MAX_DOCUMENT refuses all of them together. */
omt_xml_status omt_xml_unique_texts(const char *doc, size_t len, const char *const *tags,
                                    size_t count, omt_buf *out, bool *found);
omt_xml_status omt_xml_unique_text(const char *doc, size_t len, const char *tag, omt_buf *out);
/* True when the element text is present and equals `value`, ignoring ASCII case. */
bool omt_xml_element_is(const omt_buf *text, bool found, const char *value);
/* Confirms the document's first element is named `tag`. */
omt_xml_status omt_xml_root_is(const char *doc, size_t len, const char *tag);

#endif
