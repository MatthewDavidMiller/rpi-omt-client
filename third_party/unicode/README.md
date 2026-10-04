# Unicode Character Database extract

`16.0.0/nfc-data.txt` is the subset of the Unicode Character Database 16.0.0
that canonical normalization needs: each code point's canonical combining
class, its canonical decomposition, and whether that decomposition is a
primary composite. It was extracted by `tools/gen/extract_ucd.py` and is the
input `tools/gen/gen_nfc_tables.py` builds `src/common/nfc_tables.c` from.

The data is Copyright © 1991-2024 Unicode, Inc., distributed under the Unicode
License v3; see THIRD_PARTY_NOTICES.txt.
