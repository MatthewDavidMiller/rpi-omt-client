/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/xml.h"
#include "test.h"

static omt_xml_status text(const char *doc, const char *tag, omt_buf *out) {
    return omt_xml_unique_text(doc, strlen(doc), tag, out);
}

static void reads_and_decodes_a_unique_element(void) {
    const char *doc =
        "<OMTAddress><Name>STUDIO (Camera &amp; One)</Name><Port>6400</Port></OMTAddress>";
    omt_buf out;
    CHECK_INT(text(doc, "Name", &out), OMT_XML_OK);
    CHECK_STR(omt_buf_cstr(&out), "STUDIO (Camera & One)");
    omt_buf_free(&out);
    CHECK_INT(text(doc, "Port", &out), OMT_XML_OK);
    CHECK_STR(omt_buf_cstr(&out), "6400");
    omt_buf_free(&out);
    CHECK_INT(text(doc, "Missing", &out), OMT_XML_NOT_FOUND);
}

static void rejects_duplicated_and_declared_documents(void) {
    omt_buf out;
    CHECK_INT(text("<a><Name>x</Name><Name>y</Name></a>", "Name", &out), OMT_XML_DUPLICATE);
    CHECK_INT(text("<!DOCTYPE a [<!ENTITY x \"y\">]><a><Name>&x;</Name></a>", "Name", &out),
              OMT_XML_UNSUPPORTED);
    CHECK_INT(text("<a><Name>x</a>", "Name", &out), OMT_XML_MALFORMED);
    CHECK_INT(text("<a><Name>&unknown;</Name></a>", "Name", &out), OMT_XML_MALFORMED);
    CHECK_INT(text("<a><Name>&#65;</Name></a>", "Name", &out), OMT_XML_MALFORMED);
    CHECK_INT(text("<a><?pi x?><Name>x</Name></a>", "Name", &out), OMT_XML_UNSUPPORTED);
    CHECK_INT(text("<a><Name>x</Name></a></b>", "Name", &out), OMT_XML_MALFORMED);
    CHECK_INT(text("<a><Name>x", "Name", &out), OMT_XML_NOT_FOUND);
    CHECK_INT(text("<a><Name", "Name", &out), OMT_XML_MALFORMED);
}

static void reads_every_requested_tag_in_one_pass(void) {
    const char *doc = "<OMTAddress><Name>Camera &amp; Two</Name><Removed>True</Removed>"
                      "<Addresses><IPAddress>192.0.2.10</IPAddress></Addresses>"
                      "<Port>6400</Port></OMTAddress>";
    const char *tags[] = {"Name", "Removed", "IPAddress", "Port"};
    omt_buf out[4];
    bool found[4];
    CHECK_INT(omt_xml_unique_texts(doc, strlen(doc), tags, 4, out, found), OMT_XML_OK);
    CHECK_STR(omt_buf_cstr(&out[0]), "Camera & Two");
    CHECK(omt_xml_element_is(&out[1], found[1], "true"));
    CHECK_STR(omt_buf_cstr(&out[2]), "192.0.2.10");
    CHECK_STR(omt_buf_cstr(&out[3]), "6400");
    for (int i = 0; i < 4; i++) omt_buf_free(&out[i]);

    const char *sparse = "<OMTAddress><Name>Camera</Name></OMTAddress>";
    CHECK_INT(omt_xml_unique_texts(sparse, strlen(sparse), tags, 4, out, found), OMT_XML_OK);
    CHECK(found[0] && !found[3]);
    CHECK(!omt_xml_element_is(&out[1], found[1], "True"));
    for (int i = 0; i < 4; i++) omt_buf_free(&out[i]);

    const char *dups[] = {"<a><Name>x</Name><Name>y</Name><Port>1</Port></a>",
                          "<a><Name>x</Name><Port>1</Port><Port>2</Port></a>"};
    const char *two[] = {"Name", "Port"};
    for (int i = 0; i < 2; i++)
        CHECK_INT(omt_xml_unique_texts(dups[i], strlen(dups[i]), two, 2, out, found),
                  OMT_XML_DUPLICATE);
    const char *declared = "<!DOCTYPE a><a><Name>x</Name></a>";
    CHECK_INT(omt_xml_unique_texts(declared, strlen(declared), two, 2, out, found),
              OMT_XML_UNSUPPORTED);
}

static void bounds_depth_and_size(void) {
    char doc[512];
    size_t n = 0;
    for (int i = 0; i < 33; i++) n += (size_t)snprintf(doc + n, sizeof(doc) - n, "<a>");
    omt_buf out;
    CHECK_INT(omt_xml_unique_text(doc, n, "Name", &out), OMT_XML_UNSUPPORTED);
    static char big[OMT_XML_MAX_DOCUMENT + 2];
    memset(big, 'x', sizeof(big));
    CHECK_INT(omt_xml_unique_text(big, sizeof(big), "Name", &out), OMT_XML_UNSUPPORTED);
}

static void checks_the_document_root(void) {
    const char *a = "<Settings><A>1</A></Settings>";
    const char *b = "<?xml version=\"1.0\"?><Settings/>";
    const char *c = "<Other/>";
    CHECK_INT(omt_xml_root_is(a, strlen(a), "Settings"), OMT_XML_OK);
    CHECK_INT(omt_xml_root_is(b, strlen(b), "Settings"), OMT_XML_OK);
    CHECK_INT(omt_xml_root_is(c, strlen(c), "Settings"), OMT_XML_UNSUPPORTED);
    CHECK_INT(omt_xml_root_is("", 0, "Settings"), OMT_XML_NOT_FOUND);
}

static void attributes_and_comments(void) {
    const char *doc =
        "<a x=\"1>2\"><!-- <Name>no</Name> --><Name y='z'>v</Name><![CDATA[<Name>]]></a>";
    omt_buf out;
    CHECK_INT(text(doc, "Name", &out), OMT_XML_OK);
    CHECK_STR(omt_buf_cstr(&out), "v");
    omt_buf_free(&out);
}

int main(void) {
    RUN(reads_and_decodes_a_unique_element);
    RUN(rejects_duplicated_and_declared_documents);
    RUN(reads_every_requested_tag_in_one_pass);
    RUN(bounds_depth_and_size);
    RUN(checks_the_document_root);
    RUN(attributes_and_comments);
    return TEST_EXIT();
}
