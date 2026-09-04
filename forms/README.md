# Forms

`forms` is the embedded registry of official government PDF forms used by Navigator's form-backed Notations. It exposes
each pinned blank and its answer-to-AcroForm field map by stable form code.

It serves document and workflow code that must fill the exact government artifact an attorney expects. Vendoring the
bytes, provenance, and mapping together makes rendering reproducible and reviewable instead of depending on a changing
external download.

The canonical files live under `templates/notations/forms/`. See [government forms](../docs/gov-forms.md) for the sync,
mapping, validation, and filing lifecycle.
