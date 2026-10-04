/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The text parsers: strict JSON, the restricted XML reader, NFC, and the
 * discovery announcement that combines the last two.
 */
#include "common/json.h"
#include "common/nfc.h"
#include "common/xml.h"
#include "receiver/discovery.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    const char *text = (const char *)data;
    omt_json_doc doc;
    if (omt_json_parse(&doc, text, size, 0)) omt_json_doc_free(&doc);
    const char *tags[] = {"Name", "Removed", "IPAddress", "Port"};
    omt_buf out[4];
    bool found[4];
    if (omt_xml_unique_texts(text, size, tags, 4, out, found) == OMT_XML_OK)
        for (int i = 0; i < 4; i++) omt_buf_free(&out[i]);
    (void)omt_xml_root_is(text, size, "Settings");
    omt_announcement a;
    if (omt_utf8_valid(text, size)) (void)omt_announcement_read(text, size, &a);
    omt_buf nfc;
    omt_buf_init(&nfc, size * 4 + 64);
    (void)omt_nfc_normalize(text, size, &nfc);
    omt_buf_free(&nfc);
    omt_buf lossy;
    omt_buf_init(&lossy, size * 3 + 8);
    omt_utf8_lossy(&lossy, text, size);
    omt_buf_free(&lossy);
    return 0;
}
