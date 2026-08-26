# Tinos

This directory contains the four static Tinos faces (Regular, Italic, Bold, Bold Italic) embedded by the `pdf` crate for
court-paper rendering (`pdf::pleading`).

Tinos (Steve Matteson / Ascender) is metrically compatible with Times New Roman, which Times New Roman itself cannot be:
it is Monotype-licensed and may never be embedded here. Under CRC 2.105's "essentially equivalent to ... Times New
Roman" standard, Tinos is the strongest available match, and it ships from Google Fonts under the same licence family as
the Noto Serif faces already embedded in this crate, so no separate licensing analysis was needed to vendor it.

It serves pleading rendering only; Navigator's web fonts are separate deployment assets. The files come from Google
Fonts and are distributed under the SIL Open Font License 1.1 in <OFL.txt>, preserving the provenance and permission
needed to ship them inside the renderer.
