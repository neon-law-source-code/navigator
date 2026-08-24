#![allow(clippy::doc_markdown, clippy::similar_names)]
//! Read-modify-write filling of existing fillable PDFs (AcroForm).
//!
//! Distinct from the Typst [`render`](crate::render) path, which only
//! *emits* fresh PDFs and cannot read one. A blank government form (a
//! Nevada SoS articles form, an IRS 990) is loaded, its AcroForm
//! `/Fields` are populated from a `name → value` map, and the modified
//! PDF bytes are written back. Backed by `lopdf` (pure Rust).
//!
//! ## What it does
//!
//! Walks the AcroForm `/Fields` (top-level named fields), and for each
//! requested field name sets its value by field type, then sets
//! `/NeedAppearances true` on the AcroForm so a viewer regenerates the
//! field appearance streams. We do **not** hand-author `/AP` appearance
//! streams — `NeedAppearances` is the pragmatic, viewer-portable choice.
//!
//! - **Text (`Tx`) and choice (`Ch`)** — `/V` is set as a literal
//!   string on the top-level field. A `Tx` parent whose kids are
//!   `/T`-less duplicate widgets (the same value printed on two pages —
//!   the Nevada SoS packets do this) inherits correctly: kids without
//!   their own `/V` render the parent's.
//! - **Checkbox / radio (`Btn`)** — `/V` is set as a *Name* object that
//!   must match one of the field's appearance states (the keys of each
//!   widget's `/AP /N` dictionary; `Off` is always allowed). For a
//!   radio group (parent with `/T`-less kids) the matching kid's `/AS`
//!   is set to the chosen state and every other kid to `Off`; a
//!   kid-less checkbox gets its own `/AS`. On-state names in real
//!   government forms are arbitrary (`Yes`, `1`, `managers`,
//!   `24HOUR Expedite`) — field maps record the exact state, derived
//!   from the vendored bytes, never guessed.
//!
//! ## What it refuses (loud failures, never a silent blank)
//!
//! - **Dynamic XFA forms** (`/NeedsRendering true` or an XFA
//!   `<dynamicRender>required</dynamicRender>` config packet) — Adobe's
//!   XML form layer has no authoritative AcroForm `/V` to set, so
//!   filling one would silently produce a pristine blank that *looks*
//!   ready to file. No Rust crate fills dynamic XFA; we
//!   [`PdfError::XfaUnsupported`] instead. Static hybrid XFA packets
//!   with real AcroForm widgets can be filled through that AcroForm
//!   layer, and [`strip_static_xfa`] removes their XFA packet before
//!   vendoring a re-authored blank.
//! - **A field name with no match** — a mis-mapped or renamed
//!   government field is a competence trap if dropped silently, so an
//!   unmatched key is [`PdfError::UnmatchedField`], not a no-op.
//! - **A `Btn` value matching no appearance state** — including a
//!   pushbutton (which has no states at all) —
//!   [`PdfError::InvalidChoice`], carrying the allowed states.
//! - **No AcroForm at all** — [`PdfError::NoAcroForm`].
//!
//! Dotted hierarchical `/T` names (kids carrying their own `/T`) remain
//! out of scope; none of the forms we fill use them.
//!
//! ## Flattening (before filing)
//!
//! [`flatten`] turns a filled form into static page content: each value
//! is painted onto the page it sits on (text as page text, a checked box
//! as its own `/AP` appearance stream), the widget annotations are
//! removed, and the AcroForm `/Fields` is emptied. The result carries no
//! interactive form, so once lawyers have approved a packet no downstream
//! viewer can re-edit a value on the way to a government office — and a
//! viewer that ignores `/NeedAppearances` shows the filled values rather
//! than a blank form. It runs at the end of the fill path, past
//! `lawyer_review`.

use std::collections::{BTreeMap, BTreeSet};

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use crate::PdfError;

#[derive(Debug, Clone)]
struct NamedField {
    id: ObjectId,
    name: String,
}

/// One field's planned mutation, computed during the read phase so the
/// write phase needs no immutable borrows of the document.
enum FillPlan {
    /// Set `/V` to a literal string on the field (text / choice).
    Text { fid: ObjectId, value: String },
    /// Set `/V` to a Name on the field and `/AS` on each listed widget.
    Button {
        fid: ObjectId,
        state: Vec<u8>,
        widget_as: Vec<(ObjectId, Vec<u8>)>,
    },
}

/// Fill an existing fillable PDF's AcroForm fields.
///
/// `fields` maps a field's `/T` name to the value to set. For text and
/// choice fields the value becomes the `/V` string; for checkbox /
/// radio (`Btn`) fields it must be one of the field's appearance-state
/// names (or `Off`). Returns the modified PDF bytes.
///
/// # Errors
///
/// - [`PdfError::Lopdf`] if `blank_pdf` is not a parseable PDF.
/// - [`PdfError::NoAcroForm`] if the document has no AcroForm.
/// - [`PdfError::XfaUnsupported`] if the form is dynamic XFA.
/// - [`PdfError::UnmatchedField`] if a key in `fields` matches no
///   form field — never silently dropped.
/// - [`PdfError::InvalidChoice`] if a `Btn` value matches none of the
///   field's appearance states — never a silently unchecked box.
pub fn fill_acroform(
    blank_pdf: &[u8],
    fields: &BTreeMap<String, String>,
) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(blank_pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;

    let (acroform_id, form_fields) = locate_acroform(&doc)?;

    // Build name → field ObjectId for the top-level named fields.
    let mut by_name: BTreeMap<String, ObjectId> = BTreeMap::new();
    for field in &form_fields {
        by_name.insert(field.name.clone(), field.id);
    }

    // Every requested key must match a field — a mis-mapped name is a
    // loud error, not a silent drop (competence guardrail).
    for name in fields.keys() {
        if !by_name.contains_key(name) {
            return Err(PdfError::UnmatchedField(name.clone()));
        }
    }

    // Read phase: plan every mutation while the document is borrowed
    // immutably; validate Btn values against the appearance states.
    let mut plans: Vec<FillPlan> = Vec::with_capacity(fields.len());
    for (name, value) in fields {
        let fid = by_name[name];
        let dict = doc
            .get_object(fid)
            .and_then(Object::as_dict)
            .map_err(|e| PdfError::Lopdf(e.to_string()))?;
        let is_button = dict.get(b"FT").and_then(Object::as_name).ok() == Some(b"Btn");
        if is_button {
            plans.push(plan_button(&doc, fid, dict, name, value)?);
        } else {
            plans.push(FillPlan::Text {
                fid,
                value: value.clone(),
            });
        }
    }

    // Write phase: apply the plans.
    for plan in plans {
        match plan {
            FillPlan::Text { fid, value } => {
                if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(&fid) {
                    dict.set(
                        "V",
                        Object::String(value.into_bytes(), StringFormat::Literal),
                    );
                }
            }
            FillPlan::Button {
                fid,
                state,
                widget_as,
            } => {
                if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(&fid) {
                    dict.set("V", Object::Name(state));
                }
                for (wid, as_state) in widget_as {
                    if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(&wid) {
                        dict.set("AS", Object::Name(as_state));
                    }
                }
            }
        }
    }

    // NeedAppearances=true so viewers regenerate field appearances from
    // the new values rather than showing stale/empty boxes.
    set_need_appearances(&mut doc, acroform_id);

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    Ok(out)
}

/// Plan a checkbox / radio fill: validate `value` against the field's
/// appearance states and compute the `/AS` to set on each widget.
///
/// A radio group is a parent with `/T`-less kid widgets, each carrying
/// its own on-state in `/AP /N`; a checkbox is a kid-less field whose
/// own `/AP /N` holds the on-state. `Off` is always a valid value (it
/// unchecks). A pushbutton has no states at all, so any value for it is
/// [`PdfError::InvalidChoice`] with an empty allowed list.
fn plan_button(
    doc: &Document,
    fid: ObjectId,
    dict: &Dictionary,
    name: &str,
    value: &str,
) -> Result<FillPlan, PdfError> {
    let kid_ids: Vec<ObjectId> = dict
        .get(b"Kids")
        .and_then(Object::as_array)
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();

    // (widget id, that widget's on-states) — the field itself when the
    // checkbox has no kids.
    let widgets: Vec<(ObjectId, Vec<String>)> = if kid_ids.is_empty() {
        vec![(fid, appearance_states(doc, dict))]
    } else {
        kid_ids
            .iter()
            .map(|kid| {
                let states = doc
                    .get_object(*kid)
                    .and_then(Object::as_dict)
                    .map(|d| appearance_states(doc, d))
                    .unwrap_or_default();
                (*kid, states)
            })
            .collect()
    };

    let mut allowed: Vec<String> = widgets
        .iter()
        .flat_map(|(_, states)| states.iter().cloned())
        .filter(|s| s != "Off")
        .collect();
    allowed.sort();
    allowed.dedup();

    if value != "Off" && !allowed.iter().any(|s| s == value) {
        return Err(PdfError::InvalidChoice {
            field: name.to_string(),
            value: value.to_string(),
            allowed,
        });
    }

    let widget_as = widgets
        .into_iter()
        .map(|(wid, states)| {
            let as_state = if value != "Off" && states.iter().any(|s| s == value) {
                value.as_bytes().to_vec()
            } else {
                b"Off".to_vec()
            };
            (wid, as_state)
        })
        .collect();

    Ok(FillPlan::Button {
        fid,
        state: value.as_bytes().to_vec(),
        widget_as,
    })
}

/// The appearance-state names of one widget: the keys of its `/AP /N`
/// dictionary (`/AP` and `/N` may each be indirect). Streams behind the
/// keys are irrelevant here — only the names are.
fn appearance_states(doc: &Document, widget: &Dictionary) -> Vec<String> {
    let Some(ap) = resolve_dict(doc, widget.get(b"AP").ok()) else {
        return Vec::new();
    };
    let Some(n) = resolve_dict(doc, ap.get(b"N").ok()) else {
        return Vec::new();
    };
    n.iter()
        .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
        .collect()
}

/// Resolve an object that may be a dictionary or a reference to one.
fn resolve_dict<'a>(doc: &'a Document, obj: Option<&'a Object>) -> Option<&'a Dictionary> {
    match obj? {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_object(*id).and_then(Object::as_dict).ok(),
        _ => None,
    }
}

/// Resolve the AcroForm and its top-level field ids. The AcroForm is
/// always stored as an indirect object here (the builder writes it that
/// way and real forms do too), so `acroform_id` is its `ObjectId`.
fn locate_acroform(doc: &Document) -> Result<(ObjectId, Vec<NamedField>), PdfError> {
    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let catalog = doc
        .get_object(root_id)
        .and_then(Object::as_dict)
        .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let acroform_id = catalog
        .get(b"AcroForm")
        .and_then(Object::as_reference)
        .map_err(|_| PdfError::NoAcroForm)?;
    let acroform = doc
        .get_object(acroform_id)
        .and_then(Object::as_dict)
        .map_err(|_| PdfError::NoAcroForm)?;

    if is_dynamic_xfa(doc, acroform) {
        return Err(PdfError::XfaUnsupported);
    }

    let root_fields = acroform
        .get(b"Fields")
        .and_then(Object::as_array)
        .map_err(|_| PdfError::NoAcroForm)?
        .iter()
        .filter_map(|o| o.as_reference().ok())
        .collect::<Vec<_>>();
    let mut fields = Vec::new();
    let mut seen = BTreeSet::new();
    for field_id in root_fields {
        collect_named_fields(doc, field_id, None, &mut fields, &mut seen);
    }
    Ok((acroform_id, fields))
}

fn collect_named_fields(
    doc: &Document,
    field_id: ObjectId,
    parent_name: Option<&str>,
    out: &mut Vec<NamedField>,
    seen: &mut BTreeSet<ObjectId>,
) {
    if !seen.insert(field_id) {
        return;
    }
    let Ok(dict) = doc.get_object(field_id).and_then(Object::as_dict) else {
        return;
    };
    let kids: Vec<ObjectId> = dict
        .get(b"Kids")
        .and_then(Object::as_array)
        .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default();
    let own_name = field_name(dict);
    let full_name = match (parent_name, own_name.as_deref()) {
        (_, Some(name)) if is_reauthored_name(name) => Some(name.to_string()),
        (Some(parent), Some(name)) => Some(format!("{parent}.{name}")),
        (None, Some(name)) => Some(name.to_string()),
        (Some(_) | None, None) => None,
    };
    let is_widget = dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget");
    let is_terminal_field = dict.has(b"FT") || is_widget || kids.is_empty();
    if let Some(name) = full_name.as_ref().filter(|_| is_terminal_field) {
        out.push(NamedField {
            id: field_id,
            name: name.clone(),
        });
        return;
    }
    for kid in kids {
        collect_named_fields(doc, kid, full_name.as_deref(), out, seen);
    }
}

fn is_reauthored_name(name: &str) -> bool {
    name.starts_with("unmapped__") || name.contains("__")
}

fn field_name(dict: &Dictionary) -> Option<String> {
    dict.get(b"T").and_then(Object::as_str).ok().map(pdf_string)
}

fn pdf_string(bytes: &[u8]) -> String {
    if let Some(body) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return String::from_utf16_lossy(
            &body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect::<Vec<_>>(),
        );
    }
    if let Some(body) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return String::from_utf16_lossy(
            &body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect::<Vec<_>>(),
        );
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Remove the XFA packet from a static hybrid AcroForm while preserving
/// the AcroForm fields and widget appearances. Dynamic XFA still fails
/// loudly through the same guard as filling.
///
/// # Errors
///
/// The same parse / locate failures as [`fill_acroform`]. An encrypted
/// document is opened with the empty user password and saved without an
/// `/Encrypt` trailer entry, which strips permissions-only encryption
/// from public government blanks before the vendored pin is written.
pub fn strip_static_xfa(pdf: &[u8]) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    decrypt_empty_password_if_needed(&mut doc)?;
    let (acroform_id, _) = locate_acroform(&doc)?;
    if let Some(Object::Dictionary(acroform)) = doc.objects.get_mut(&acroform_id) {
        acroform.remove(b"XFA");
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
    doc.prune_objects();

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    Ok(out)
}

fn decrypt_empty_password_if_needed(doc: &mut Document) -> Result<(), PdfError> {
    if doc.is_encrypted() {
        doc.decrypt("")
            .map_err(|e| PdfError::Lopdf(format!("decrypt empty user password: {e}")))?;
    }
    Ok(())
}

fn is_dynamic_xfa(doc: &Document, acroform: &Dictionary) -> bool {
    acroform
        .get(b"NeedsRendering")
        .and_then(Object::as_bool)
        .is_ok_and(|needs| needs)
        || acroform
            .get(b"XFA")
            .is_ok_and(|xfa| xfa_has_required_dynamic_render(doc, xfa))
}

fn xfa_has_required_dynamic_render(doc: &Document, xfa: &Object) -> bool {
    match xfa {
        Object::Array(entries) => entries
            .iter()
            .any(|entry| xfa_has_required_dynamic_render(doc, entry)),
        // A real XFA config packet is almost always FlateDecode-
        // compressed, so `stream.content` (the raw stored bytes) never
        // shows the `<dynamicRender>` marker. Decode before matching,
        // and fail closed — a packet we cannot inspect is treated as
        // dynamic and rejected rather than silently stripped and filled
        // to a blank filing.
        Object::Stream(stream) => stream
            .decompressed_content()
            .map_or(true, |bytes| dynamic_render_required(&bytes)),
        Object::String(bytes, _) => dynamic_render_required(bytes),
        Object::Reference(id) => doc
            .get_object(*id)
            .is_ok_and(|obj| xfa_has_required_dynamic_render(doc, obj)),
        _ => false,
    }
}

fn dynamic_render_required(bytes: &[u8]) -> bool {
    let compact = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    compact.contains("<dynamicrender>required</dynamicrender>")
}

fn set_need_appearances(doc: &mut Document, acroform_id: ObjectId) {
    if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(&acroform_id) {
        dict.set("NeedAppearances", Object::Boolean(true));
    }
}

/// One field in a synthetic fixture form — mirrors the three field
/// shapes the real government packets carry.
#[derive(Debug, Clone)]
pub enum FieldSpec {
    /// A `Tx` text field.
    Text { name: String },
    /// A kid-less `Btn` checkbox whose `/AP /N` carries `on_state` +
    /// `Off` (real on-state names are arbitrary: `Yes`, `1`,
    /// `managers`, …).
    Checkbox { name: String, on_state: String },
    /// A `Btn` radio group: a `/T`-named parent with one `/T`-less kid
    /// widget per option, each kid's `/AP /N` carrying its option +
    /// `Off`.
    Radio { name: String, options: Vec<String> },
}

impl FieldSpec {
    fn text(name: &str) -> Self {
        Self::Text { name: name.into() }
    }
}

/// Build a minimal but genuinely-valid fillable PDF with one text field
/// per name in `field_names` (each `/V` empty). Shorthand for
/// [`blank_acroform_with`] over [`FieldSpec::Text`] entries.
#[must_use]
pub fn blank_acroform(field_names: &[&str]) -> Vec<u8> {
    let specs: Vec<FieldSpec> = field_names.iter().map(|n| FieldSpec::text(n)).collect();
    blank_acroform_with(&specs)
}

/// Append the widget-annotation keys shared by every fixture field.
fn set_widget_keys(field: &mut Dictionary, page_id: ObjectId) {
    field.set("Type", Object::Name(b"Annot".to_vec()));
    field.set("Subtype", Object::Name(b"Widget".to_vec()));
    field.set("P", Object::Reference(page_id));
    field.set(
        "Rect",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(200),
            Object::Integer(20),
        ]),
    );
}

/// An `/AP` dictionary whose `/N` carries the given state names (the
/// streams behind them are irrelevant to filling, so `Null` stands in).
fn appearance_dict(states: &[&str]) -> Object {
    let mut n = Dictionary::new();
    for state in states {
        n.set(state.as_bytes().to_vec(), Object::Null);
    }
    let mut ap = Dictionary::new();
    ap.set("N", Object::Dictionary(n));
    Object::Dictionary(ap)
}

/// Add one `Tx` text field widget; returns its id.
fn add_text_field(doc: &mut Document, page_id: ObjectId, name: &str) -> ObjectId {
    let mut field = Dictionary::new();
    field.set("FT", Object::Name(b"Tx".to_vec()));
    field.set(
        "T",
        Object::String(name.as_bytes().to_vec(), StringFormat::Literal),
    );
    field.set("V", Object::String(Vec::new(), StringFormat::Literal));
    set_widget_keys(&mut field, page_id);
    doc.add_object(Object::Dictionary(field))
}

/// Add one kid-less `Btn` checkbox with an arbitrary on-state; returns
/// its id.
fn add_checkbox_field(
    doc: &mut Document,
    page_id: ObjectId,
    name: &str,
    on_state: &str,
) -> ObjectId {
    let mut field = Dictionary::new();
    field.set("FT", Object::Name(b"Btn".to_vec()));
    field.set(
        "T",
        Object::String(name.as_bytes().to_vec(), StringFormat::Literal),
    );
    field.set("V", Object::Name(b"Off".to_vec()));
    field.set("AS", Object::Name(b"Off".to_vec()));
    field.set("AP", appearance_dict(&[on_state, "Off"]));
    set_widget_keys(&mut field, page_id);
    doc.add_object(Object::Dictionary(field))
}

/// Add a `Btn` radio group — a `/T`-named parent with one `/T`-less kid
/// widget per option; returns (parent id, kid ids).
fn add_radio_group(
    doc: &mut Document,
    page_id: ObjectId,
    name: &str,
    options: &[String],
) -> (ObjectId, Vec<ObjectId>) {
    let parent_id = doc.new_object_id();
    let mut kid_ids: Vec<ObjectId> = Vec::new();
    for option in options {
        let mut kid = Dictionary::new();
        kid.set("Parent", Object::Reference(parent_id));
        kid.set("AS", Object::Name(b"Off".to_vec()));
        kid.set("AP", appearance_dict(&[option, "Off"]));
        set_widget_keys(&mut kid, page_id);
        kid_ids.push(doc.add_object(Object::Dictionary(kid)));
    }
    let mut parent = Dictionary::new();
    parent.set("FT", Object::Name(b"Btn".to_vec()));
    parent.set(
        "T",
        Object::String(name.as_bytes().to_vec(), StringFormat::Literal),
    );
    // Radio flag (bit 16) + no-toggle-to-off (bit 15), as the real
    // packets set.
    parent.set("Ff", Object::Integer(49152));
    parent.set("V", Object::Name(b"Off".to_vec()));
    parent.set(
        "Kids",
        Object::Array(kid_ids.iter().copied().map(Object::Reference).collect()),
    );
    doc.objects.insert(parent_id, Object::Dictionary(parent));
    (parent_id, kid_ids)
}

/// Build a minimal but genuinely-valid fillable PDF from field specs.
/// Used to produce blank form fixtures for tests and synthetic forms —
/// the same AcroForm structures (text widgets, checkboxes with arbitrary
/// on-states, radio groups with `/T`-less kids) real government forms
/// carry, so [`fill_acroform`] exercises the real path.
#[must_use]
pub fn blank_acroform_with(specs: &[FieldSpec]) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");

    let page_id = doc.new_object_id();
    let pages_id = doc.new_object_id();

    let mut field_ids: Vec<ObjectId> = Vec::new();
    let mut annot_ids: Vec<ObjectId> = Vec::new();
    for spec in specs {
        match spec {
            FieldSpec::Text { name } => {
                let fid = add_text_field(&mut doc, page_id, name);
                field_ids.push(fid);
                annot_ids.push(fid);
            }
            FieldSpec::Checkbox { name, on_state } => {
                let fid = add_checkbox_field(&mut doc, page_id, name, on_state);
                field_ids.push(fid);
                annot_ids.push(fid);
            }
            FieldSpec::Radio { name, options } => {
                let (parent_id, kid_ids) = add_radio_group(&mut doc, page_id, name, options);
                field_ids.push(parent_id);
                annot_ids.extend(kid_ids);
            }
        }
    }

    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    page.set(
        "Annots",
        Object::Array(annot_ids.iter().copied().map(Object::Reference).collect()),
    );
    doc.objects.insert(page_id, Object::Dictionary(page));

    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let mut acroform = Dictionary::new();
    acroform.set(
        "Fields",
        Object::Array(field_ids.iter().copied().map(Object::Reference).collect()),
    );
    acroform.set(
        "DA",
        Object::String(b"/Helv 0 Tf 0 g".to_vec(), StringFormat::Literal),
    );
    let acroform_id = doc.add_object(Object::Dictionary(acroform));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    catalog.set("AcroForm", Object::Reference(acroform_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));

    doc.trailer.set("Root", Object::Reference(catalog_id));

    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save synthetic AcroForm");
    out
}

/// Read back the `/V` value of a top-level field by `/T` name. A test
/// helper for round-trip assertions; returns `None` if the field or its
/// value is absent. A `Btn` field's Name-typed `/V` reads back as the
/// state name string.
#[must_use]
pub fn read_field_value(pdf: &[u8], name: &str) -> Option<String> {
    let doc = Document::load_mem(pdf).ok()?;
    let (_, fields) = locate_acroform(&doc).ok()?;
    for field in fields {
        let dict = doc.get_object(field.id).and_then(Object::as_dict).ok()?;
        if field.name == name {
            return match dict.get(b"V").ok()? {
                Object::String(v, _) => Some(pdf_string(v)),
                Object::Name(v) => Some(String::from_utf8_lossy(v).into_owned()),
                _ => None,
            };
        }
    }
    None
}

/// Read back every top-level field's `/V` in one parse — the bulk
/// sibling of [`read_field_value`] for round-trip assertions over big
/// government packets, where a per-field parse is prohibitively slow.
/// Name-typed `/V` values read back as their state-name strings;
/// fields with no readable `/V` are absent from the map.
///
/// # Errors
///
/// The same parse / locate failures as [`fill_acroform`].
pub fn read_field_values(pdf: &[u8]) -> Result<BTreeMap<String, String>, PdfError> {
    let doc = Document::load_mem(pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let (_, fields) = locate_acroform(&doc)?;
    let mut out = BTreeMap::new();
    for field in fields {
        let Ok(dict) = doc.get_object(field.id).and_then(Object::as_dict) else {
            continue;
        };
        let value = match dict.get(b"V") {
            Ok(Object::String(v, _)) => pdf_string(v),
            Ok(Object::Name(v)) => String::from_utf8_lossy(v).into_owned(),
            _ => continue,
        };
        out.insert(field.name, value);
    }
    Ok(out)
}

/// The `/T` names of every top-level AcroForm field. Used by field-map
/// guard tests to assert that each mapped name exists in the vendored
/// bytes before a fill is ever attempted in production.
///
/// # Errors
///
/// The same parse / locate failures as [`fill_acroform`].
pub fn field_names(pdf: &[u8]) -> Result<Vec<String>, PdfError> {
    let doc = Document::load_mem(pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let (_, fields) = locate_acroform(&doc)?;
    Ok(fields.into_iter().map(|field| field.name).collect())
}

/// Count the widget annotations reachable from the pages' `/Annots`
/// arrays (dereferenced when indirect). This is the interactive layer a
/// viewer can rebuild form fields from even when the AcroForm `/Fields`
/// array is empty — so `0` here, not an empty [`field_names`], is the
/// property that proves a [`flatten`]ed packet cannot be re-edited.
///
/// # Errors
///
/// [`PdfError::Lopdf`] if `pdf` is not a parseable PDF.
pub fn widget_annotation_count(pdf: &[u8]) -> Result<usize, PdfError> {
    let doc = Document::load_mem(pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let mut count = 0usize;
    for page_id in doc.get_pages().values().copied() {
        let annots: &Vec<Object> = match annots_slot(&doc, page_id) {
            Some(AnnotsSlot::Inline(pid)) => match doc
                .get_dictionary(pid)
                .ok()
                .and_then(|p| p.get(b"Annots").ok())
            {
                Some(Object::Array(a)) => a,
                _ => continue,
            },
            Some(AnnotsSlot::Indirect(id)) => match doc.objects.get(&id) {
                Some(Object::Array(a)) => a,
                _ => continue,
            },
            None => continue,
        };
        count += annots
            .iter()
            .filter(|entry| {
                let dict = match entry {
                    Object::Reference(id) => doc.get_object(*id).and_then(Object::as_dict).ok(),
                    Object::Dictionary(d) => Some(d),
                    _ => None,
                };
                dict.is_some_and(|d| {
                    d.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget")
                })
            })
            .count();
    }
    Ok(count)
}

/// Where a page's `/Annots` array lives: inline on the page dictionary
/// itself, or behind an indirect reference — the shape every vendored
/// NV packet uses. Chained references are followed a bounded number of
/// hops so a malformed cycle cannot loop.
enum AnnotsSlot {
    /// The array is a direct value of the page's `/Annots` key.
    Inline(ObjectId),
    /// The array is the standalone object with this id.
    Indirect(ObjectId),
}

fn annots_slot(doc: &Document, page_id: ObjectId) -> Option<AnnotsSlot> {
    match doc.get_dictionary(page_id).ok()?.get(b"Annots").ok()? {
        Object::Array(_) => Some(AnnotsSlot::Inline(page_id)),
        Object::Reference(first) => {
            let mut id = *first;
            for _ in 0..8 {
                match doc.objects.get(&id)? {
                    Object::Reference(next) => id = *next,
                    Object::Array(_) => return Some(AnnotsSlot::Indirect(id)),
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

/// One text field's value drawn onto the page it sits on.
struct TextDraw {
    page: ObjectId,
    rect: [f32; 4],
    value: String,
}

/// A checkbox / radio widget's on-state appearance, stamped onto its page.
struct StampDraw {
    page: ObjectId,
    rect: [f32; 4],
    /// The `/AP /N /<state>` Form XObject: an existing indirect object we
    /// can reference directly, or an inline stream we must add first.
    ap: ApSource,
    bbox: [f32; 4],
    matrix: [f32; 6],
}

/// The source of a widget's on-state appearance stream.
enum ApSource {
    Existing(ObjectId),
    Inline(Stream),
}

/// Flatten a filled AcroForm to static page content: every filled field's
/// value is painted onto the page it sits on (text values as page text,
/// checked checkbox / radio widgets as their own appearance streams), the
/// widget annotations are removed, and the AcroForm `/Fields` is emptied.
/// The result renders identically but carries **no interactive form**, so
/// no downstream viewer can re-edit a value after lawyer review — the
/// filing-integrity step that sits at the end of the fill path.
///
/// Idempotent: a form with no fields left (already flattened) round-trips
/// unchanged. `NeedAppearances` is dropped — there are no appearances to
/// regenerate once the values are static content.
///
/// # Errors
///
/// The same parse / locate failures as [`fill_acroform`] ([`PdfError::Lopdf`],
/// [`PdfError::NoAcroForm`], [`PdfError::XfaUnsupported`]), plus
/// [`PdfError::UnencodableChar`] when a value carries a character the
/// overlay font's `WinAnsiEncoding` cannot draw.
pub fn flatten(pdf: &[u8]) -> Result<Vec<u8>, PdfError> {
    let mut doc = Document::load_mem(pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let (acroform_id, fields) = locate_acroform(&doc)?;
    let field_ids: Vec<ObjectId> = fields.iter().map(|field| field.id).collect();

    let (texts, stamps, widget_ids) = collect_flatten_draws(&doc, &field_ids);
    paint_overlays(&mut doc, texts, stamps)?;
    strip_interactive_layer(&mut doc, acroform_id, &widget_ids);

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    Ok(out)
}

/// Read phase: plan every value to paint while the document is borrowed
/// immutably, and record every widget id so its annotation can be
/// stripped later. Returns `(text draws, stamp draws, widget ids)`.
fn collect_flatten_draws(
    doc: &Document,
    field_ids: &[ObjectId],
) -> (Vec<TextDraw>, Vec<StampDraw>, BTreeSet<ObjectId>) {
    let first_page = doc.get_pages().values().next().copied();
    let mut texts: Vec<TextDraw> = Vec::new();
    let mut stamps: Vec<StampDraw> = Vec::new();
    let mut widget_ids: BTreeSet<ObjectId> = BTreeSet::new();

    for fid in field_ids {
        let Ok(dict) = doc.get_object(*fid).and_then(Object::as_dict) else {
            continue;
        };
        widget_ids.insert(*fid);
        let kids: Vec<ObjectId> = dict
            .get(b"Kids")
            .and_then(Object::as_array)
            .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
            .unwrap_or_default();
        for kid in &kids {
            widget_ids.insert(*kid);
        }
        let field_page = dict.get(b"P").and_then(Object::as_reference).ok();
        // The widgets carrying the visible box: the field itself when it
        // has no kids, else each kid.
        let widgets: Vec<ObjectId> = if kids.is_empty() { vec![*fid] } else { kids };
        let page_of = |wdict: &Dictionary| {
            wdict
                .get(b"P")
                .and_then(Object::as_reference)
                .ok()
                .or(field_page)
                .or(first_page)
        };

        if dict.get(b"FT").and_then(Object::as_name).ok() == Some(b"Btn") {
            for wid in widgets {
                let Ok(wdict) = doc.get_object(wid).and_then(Object::as_dict) else {
                    continue;
                };
                let Ok(state) = wdict.get(b"AS").and_then(Object::as_name) else {
                    continue;
                };
                if state == b"Off" {
                    continue; // an unchecked box paints nothing
                }
                let Some((ap, bbox, matrix)) = appearance_stream(doc, wdict, state) else {
                    continue;
                };
                let (Some(rect), Some(page)) = (rect_of(wdict), page_of(wdict)) else {
                    continue;
                };
                stamps.push(StampDraw {
                    page,
                    rect,
                    ap,
                    bbox,
                    matrix,
                });
            }
        } else {
            // Text / choice: the value lives on the (parent) field's /V as
            // a literal string; a Name-typed /V is a button and is skipped.
            let value = match dict.get(b"V") {
                Ok(Object::String(v, _)) => String::from_utf8_lossy(v).into_owned(),
                _ => continue,
            };
            if value.is_empty() {
                continue;
            }
            for wid in widgets {
                let Ok(wdict) = doc.get_object(wid).and_then(Object::as_dict) else {
                    continue;
                };
                let (Some(rect), Some(page)) = (rect_of(wdict), page_of(wdict)) else {
                    continue;
                };
                texts.push(TextDraw {
                    page,
                    rect,
                    value: value.clone(),
                });
            }
        }
    }
    (texts, stamps, widget_ids)
}

/// Write phase: group the draws per page, wire each page's resources, and
/// append one overlay content stream per page.
fn paint_overlays(
    doc: &mut Document,
    texts: Vec<TextDraw>,
    stamps: Vec<StampDraw>,
) -> Result<(), PdfError> {
    let mut per_page: BTreeMap<ObjectId, (Vec<TextDraw>, Vec<StampDraw>)> = BTreeMap::new();
    for t in texts {
        per_page.entry(t.page).or_default().0.push(t);
    }
    for s in stamps {
        per_page.entry(s.page).or_default().1.push(s);
    }

    let needs_font = per_page.values().any(|(t, _)| !t.is_empty());
    let font_id = needs_font.then(|| doc.add_object(helvetica_font()));

    let mut xobj_counter = 0u32;
    for (page_id, (page_texts, page_stamps)) in per_page {
        let mut ops: Vec<Operation> = Vec::new();
        for t in &page_texts {
            ops.extend(text_ops(t.rect, &t.value)?);
        }
        let mut xobjects: Vec<(Vec<u8>, ObjectId)> = Vec::new();
        for s in page_stamps {
            let ap_id = match s.ap {
                ApSource::Existing(id) => id,
                ApSource::Inline(stream) => doc.add_object(Object::Stream(stream)),
            };
            let name = format!("NavFlatX{xobj_counter}").into_bytes();
            xobj_counter += 1;
            ops.extend(stamp_ops(s.rect, s.bbox, s.matrix, &name));
            xobjects.push((name, ap_id));
        }

        let res_id = ensure_own_resources(doc, page_id)?;
        if !page_texts.is_empty() {
            if let Some(fid) = font_id {
                add_resource_entry(doc, res_id, b"Font", b"NavFlatHelv".to_vec(), fid)?;
            }
        }
        for (name, id) in xobjects {
            add_resource_entry(doc, res_id, b"XObject", name, id)?;
        }

        let bytes = Content { operations: ops }
            .encode()
            .map_err(|e| PdfError::Lopdf(e.to_string()))?;
        doc.add_page_contents(page_id, bytes)
            .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    }
    Ok(())
}

/// Drop every widget annotation from the pages — dereferencing a page's
/// `/Annots` when it is an indirect reference, the shape every vendored
/// NV packet uses — empty the AcroForm's `/Fields`, clear
/// `/NeedAppearances`, and prune the now-unreferenced field / widget
/// objects so nothing can resurrect them as editable fields.
fn strip_interactive_layer(
    doc: &mut Document,
    acroform_id: ObjectId,
    widget_ids: &BTreeSet<ObjectId>,
) {
    for page_id in doc.get_pages().values().copied().collect::<Vec<_>>() {
        let annots = match annots_slot(doc, page_id) {
            Some(AnnotsSlot::Inline(pid)) => match doc.get_dictionary_mut(pid) {
                Ok(page) => match page.get_mut(b"Annots") {
                    Ok(Object::Array(a)) => a,
                    _ => continue,
                },
                Err(_) => continue,
            },
            Some(AnnotsSlot::Indirect(id)) => match doc.objects.get_mut(&id) {
                Some(Object::Array(a)) => a,
                _ => continue,
            },
            None => continue,
        };
        annots.retain(|o| !o.as_reference().is_ok_and(|id| widget_ids.contains(&id)));
    }
    if let Some(Object::Dictionary(af)) = doc.objects.get_mut(&acroform_id) {
        af.set("Fields", Object::Array(Vec::new()));
        af.remove(b"NeedAppearances");
    }
    doc.prune_objects();
}

/// A standard Helvetica Type1 font dictionary — the overlay text font.
/// Declares `/WinAnsiEncoding` so the bytes [`winansi_encode`] writes
/// map to the intended glyphs in every viewer; without it a viewer
/// falls back to StandardEncoding and accented characters garble.
fn helvetica_font() -> Object {
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    Object::Dictionary(font)
}

/// The `WinAnsiEncoding` (Windows-1252) code points that differ from
/// Unicode's first 256: bytes 0x80–0x9F. Every other byte maps 1:1.
const WIN_ANSI_SPECIALS: [(u8, char); 27] = [
    (0x80, '€'),
    (0x82, '‚'),
    (0x83, 'ƒ'),
    (0x84, '„'),
    (0x85, '…'),
    (0x86, '†'),
    (0x87, '‡'),
    (0x88, 'ˆ'),
    (0x89, '‰'),
    (0x8A, 'Š'),
    (0x8B, '‹'),
    (0x8C, 'Œ'),
    (0x8E, 'Ž'),
    (0x91, '\u{2018}'),
    (0x92, '\u{2019}'),
    (0x93, '\u{201C}'),
    (0x94, '\u{201D}'),
    (0x95, '•'),
    (0x96, '–'),
    (0x97, '—'),
    (0x98, '˜'),
    (0x99, '™'),
    (0x9A, 'š'),
    (0x9B, '›'),
    (0x9C, 'œ'),
    (0x9E, 'ž'),
    (0x9F, 'Ÿ'),
];

/// Encode `value` as the `WinAnsiEncoding` bytes the overlay font
/// declares. ASCII whitespace controls become a space (a `Tj` draws a
/// single line); a character with no WinAnsi byte fails loudly —
/// never a garbled glyph in a packet on its way to a government office.
fn winansi_encode(value: &str) -> Result<Vec<u8>, PdfError> {
    value
        .chars()
        .map(|ch| {
            if matches!(ch, '\n' | '\r' | '\t') {
                return Ok(b' ');
            }
            if let Ok(b @ (0x20..=0x7E | 0xA0..=0xFF)) = u8::try_from(u32::from(ch)) {
                // ASCII printable and the Latin-1 block map 1:1.
                return Ok(b);
            }
            WIN_ANSI_SPECIALS
                .iter()
                .find(|&&(_, c)| c == ch)
                .map(|&(b, _)| b)
                .ok_or_else(|| PdfError::UnencodableChar {
                    ch,
                    value: value.to_owned(),
                })
        })
        .collect()
}

/// Decode `WinAnsiEncoding` bytes back to text — the inverse of
/// [`winansi_encode`], used by [`page_text`] for string operands that
/// are not valid UTF-8. Unassigned bytes decode as U+FFFD.
fn winansi_decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => WIN_ANSI_SPECIALS
                .iter()
                .find(|&&(sb, _)| sb == b)
                .map_or('\u{FFFD}', |&(_, c)| c),
            _ => char::from(b),
        })
        .collect()
}

/// Decode one text-showing operand: UTF-8 when valid (what a form's own
/// content streams typically carry), else WinAnsi — the single-byte
/// encoding [`flatten`]'s overlay writes.
fn decode_shown_text(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes).map_or_else(|_| winansi_decode(bytes), str::to_owned)
}

/// The `/Rect` of a widget as normalized `[x0, y0, x1, y1]` (x0 ≤ x1,
/// y0 ≤ y1), or `None` if absent / malformed.
fn rect_of(widget: &Dictionary) -> Option<[f32; 4]> {
    let r = four_floats(widget.get(b"Rect").ok())?;
    Some([
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ])
}

/// Read an array of at least four numbers as `[f32; 4]`.
fn four_floats(obj: Option<&Object>) -> Option<[f32; 4]> {
    let arr = obj?.as_array().ok()?;
    if arr.len() < 4 {
        return None;
    }
    Some([
        arr[0].as_float().ok()?,
        arr[1].as_float().ok()?,
        arr[2].as_float().ok()?,
        arr[3].as_float().ok()?,
    ])
}

/// The on-state appearance Form XObject for a widget: its `/AP /N /<state>`
/// entry, plus that form's `/BBox` and `/Matrix` (identity when absent).
/// `None` when there is no such stream to stamp.
fn appearance_stream(
    doc: &Document,
    widget: &Dictionary,
    state: &[u8],
) -> Option<(ApSource, [f32; 4], [f32; 6])> {
    let ap = resolve_dict(doc, widget.get(b"AP").ok())?;
    let n = resolve_dict(doc, ap.get(b"N").ok())?;
    let entry = n.get(state).ok()?;
    let (source, dict) = match entry {
        Object::Reference(id) => {
            let stream = doc.get_object(*id).and_then(Object::as_stream).ok()?;
            (ApSource::Existing(*id), &stream.dict)
        }
        Object::Stream(stream) => (ApSource::Inline(stream.clone()), &stream.dict),
        _ => return None,
    };
    let bbox = four_floats(dict.get(b"BBox").ok())?;
    let matrix = six_floats(dict.get(b"Matrix").ok()).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    Some((source, bbox, matrix))
}

/// Read an array of at least six numbers as a `[f32; 6]` matrix.
fn six_floats(obj: Option<&Object>) -> Option<[f32; 6]> {
    let arr = obj?.as_array().ok()?;
    if arr.len() < 6 {
        return None;
    }
    let mut m = [0.0f32; 6];
    for (slot, o) in m.iter_mut().zip(arr) {
        *slot = o.as_float().ok()?;
    }
    Some(m)
}

/// The content-stream operations that draw `value` inside `rect` in a
/// standard Helvetica sized to the box, encoded to match the font's
/// declared `WinAnsiEncoding`.
///
/// # Errors
///
/// [`PdfError::UnencodableChar`] when a character in `value` has no
/// WinAnsi byte.
fn text_ops(rect: [f32; 4], value: &str) -> Result<Vec<Operation>, PdfError> {
    let [x0, y0, _, y1] = rect;
    let height = y1 - y0;
    let size = (height * 0.62).clamp(6.0, 11.0);
    let tx = x0 + 2.0;
    // Baseline roughly vertically centred within the box.
    let ty = y0 + (height - size) / 2.0 + size * 0.2;
    Ok(vec![
        Operation::new("q", vec![]),
        Operation::new(
            "rg",
            vec![Object::Real(0.0), Object::Real(0.0), Object::Real(0.0)],
        ),
        Operation::new("BT", vec![]),
        Operation::new(
            "Tf",
            vec![Object::Name(b"NavFlatHelv".to_vec()), Object::Real(size)],
        ),
        Operation::new("Td", vec![Object::Real(tx), Object::Real(ty)]),
        Operation::new(
            "Tj",
            vec![Object::String(
                winansi_encode(value)?,
                StringFormat::Literal,
            )],
        ),
        Operation::new("ET", vec![]),
        Operation::new("Q", vec![]),
    ])
}

/// The content-stream operations that stamp the named appearance XObject
/// so its (matrix-transformed) `/BBox` maps onto `rect` — the placement
/// PDF viewers compute for a widget's appearance (spec 12.5.5).
fn stamp_ops(rect: [f32; 4], bbox: [f32; 4], matrix: [f32; 6], name: &[u8]) -> Vec<Operation> {
    let [rx0, ry0, rx1, ry1] = rect;
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[2], bbox[3]),
        (bbox[0], bbox[3]),
    ];
    // Map each corner through the appearance's /Matrix [m0 m1 m2 m3 m4 m5].
    let m = matrix;
    let tx = corners.map(|(x, y)| m[0] * x + m[2] * y + m[4]);
    let ty = corners.map(|(x, y)| m[1] * x + m[3] * y + m[5]);
    let tx0 = tx.iter().copied().fold(f32::INFINITY, f32::min);
    let tx1 = tx.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let ty0 = ty.iter().copied().fold(f32::INFINITY, f32::min);
    let ty1 = ty.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let bw = tx1 - tx0;
    let bh = ty1 - ty0;
    let sx = if bw.abs() > f32::EPSILON {
        (rx1 - rx0) / bw
    } else {
        1.0
    };
    let sy = if bh.abs() > f32::EPSILON {
        (ry1 - ry0) / bh
    } else {
        1.0
    };
    vec![
        Operation::new("q", vec![]),
        Operation::new(
            "cm",
            vec![
                Object::Real(sx),
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(sy),
                Object::Real(rx0 - sx * tx0),
                Object::Real(ry0 - sy * ty0),
            ],
        ),
        Operation::new("Do", vec![Object::Name(name.to_vec())]),
        Operation::new("Q", vec![]),
    ]
}

/// Ensure `page_id` has its own resources object (materializing an inline
/// or inherited dictionary into an indirect one) and return its id, so
/// overlay fonts / XObjects can be added without shadowing inherited
/// resources the page's own content already relies on.
fn ensure_own_resources(doc: &mut Document, page_id: ObjectId) -> Result<ObjectId, PdfError> {
    let lo = |e: lopdf::Error| PdfError::Lopdf(e.to_string());
    if let Ok(Object::Reference(id)) = doc.get_dictionary(page_id).map_err(lo)?.get(b"Resources") {
        return Ok(*id);
    }
    // Inline or inherited: merge everything reachable up the page tree into
    // one owned dictionary, add it, and point the page at it.
    let merged = {
        let (inline, ids) = doc.get_page_resources(page_id).map_err(lo)?;
        let mut merged = inline.cloned().unwrap_or_else(Dictionary::new);
        for rid in ids {
            if let Ok(dict) = doc.get_dictionary(rid) {
                for (k, v) in dict {
                    if !merged.has(k) {
                        merged.set(k.clone(), v.clone());
                    }
                }
            }
        }
        merged
    };
    let id = doc.add_object(Object::Dictionary(merged));
    doc.get_dictionary_mut(page_id)
        .map_err(lo)?
        .set("Resources", Object::Reference(id));
    Ok(id)
}

/// Add `name → target` under the `category` sub-dictionary (`/Font`,
/// `/XObject`, …) of the resources object `res_id`, resolving a
/// referenced sub-dictionary or creating an inline one as needed.
fn add_resource_entry(
    doc: &mut Document,
    res_id: ObjectId,
    category: &[u8],
    name: Vec<u8>,
    target: ObjectId,
) -> Result<(), PdfError> {
    let lo = |e: lopdf::Error| PdfError::Lopdf(e.to_string());
    let sub_ref = match doc.get_dictionary(res_id).map_err(lo)?.get(category) {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    if let Some(sub_id) = sub_ref {
        doc.get_dictionary_mut(sub_id)
            .map_err(lo)?
            .set(name, Object::Reference(target));
        return Ok(());
    }
    let res = doc.get_dictionary_mut(res_id).map_err(lo)?;
    if !matches!(res.get(category), Ok(Object::Dictionary(_))) {
        res.set(category.to_vec(), Object::Dictionary(Dictionary::new()));
    }
    if let Ok(Object::Dictionary(sub)) = res.get_mut(category) {
        sub.set(name, Object::Reference(target));
    }
    Ok(())
}

/// Extract the text shown on every page — the operands of the text-showing
/// operators (`Tj`, `TJ`, `'`, `"`) across all page content streams — as
/// one whitespace-joined string. Streams that fail to decode are skipped
/// (a form's own content may use encodings we don't model); the overlay we
/// append flattening a form always decodes. Used by round-trip guards to
/// prove a flattened packet still shows its filled values.
///
/// # Errors
///
/// [`PdfError::Lopdf`] if `pdf` is not a parseable PDF.
pub fn page_text(pdf: &[u8]) -> Result<String, PdfError> {
    let doc = Document::load_mem(pdf).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let mut out = String::new();
    for page_id in doc.page_iter() {
        for sid in doc.get_page_contents(page_id) {
            let Ok(stream) = doc.get_object(sid).and_then(Object::as_stream) else {
                continue;
            };
            let data = stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone());
            let Ok(content) = Content::decode(&data) else {
                continue;
            };
            for op in content.operations {
                match op.operator.as_str() {
                    "Tj" | "'" | "\"" => {
                        for o in &op.operands {
                            if let Object::String(bytes, _) = o {
                                out.push_str(&decode_shown_text(bytes));
                                out.push(' ');
                            }
                        }
                    }
                    "TJ" => {
                        if let Some(Object::Array(arr)) = op.operands.first() {
                            for el in arr {
                                if let Object::String(bytes, _) = el {
                                    out.push_str(&decode_shown_text(bytes));
                                }
                            }
                            out.push(' ');
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(out)
}

/// Read back the `/AS` appearance state of a widget: the field itself
/// for a kid-less checkbox, or the `index`-th kid of a radio group. A
/// test helper for asserting the visible checked state, not just `/V`.
#[must_use]
pub fn read_widget_appearance_state(
    pdf: &[u8],
    name: &str,
    index: Option<usize>,
) -> Option<String> {
    let doc = Document::load_mem(pdf).ok()?;
    let (_, fields) = locate_acroform(&doc).ok()?;
    for field in fields {
        let dict = doc.get_object(field.id).and_then(Object::as_dict).ok()?;
        if field.name != name {
            continue;
        }
        let widget = match index {
            None => dict,
            Some(i) => {
                let kids = dict.get(b"Kids").and_then(Object::as_array).ok()?;
                let kid_id = kids.get(i)?.as_reference().ok()?;
                doc.get_object(kid_id).and_then(Object::as_dict).ok()?
            }
        };
        let v = widget.get(b"AS").and_then(Object::as_name).ok()?;
        return Some(String::from_utf8_lossy(v).into_owned());
    }
    None
}

/// One checkbox-pair → radio-group merge in a [`ReauthorSpec`]: the
/// listed kid-less checkboxes become the `/T`-less kid widgets of a new
/// radio parent named `name`. Each kid keeps its widget appearance
/// streams, renamed from the source checkbox export to the final radio
/// export when those differ.
#[derive(Debug, Clone)]
pub struct RadioMergeSpec {
    /// The merged radio group's `/T` name.
    pub name: String,
    /// The `/T` names of the kid-less checkboxes to absorb.
    pub members: Vec<RadioMergeMember>,
}

#[derive(Debug, Clone)]
pub struct RadioMergeMember {
    /// The source checkbox field's `/T` name.
    pub field: String,
    /// The checkbox's current appearance/export state.
    pub source_export: String,
    /// The merged radio member's final export state.
    pub final_export: String,
}

/// The complete field-layer transformation [`reauthor`] applies. Every
/// top-level field of the input must be covered by exactly one entry —
/// re-authoring a document that files is total or it is refused.
#[derive(Debug, Clone, Default)]
pub struct ReauthorSpec {
    /// Old `/T` → new `/T`. Several olds may share one new name: those
    /// fields merge into a single field whose widgets are `/T`-less
    /// kids, so one value prints in every place the form repeats it.
    pub renames: BTreeMap<String, String>,
    /// Checkbox pairs merged into radio groups.
    pub radios: Vec<RadioMergeSpec>,
    /// Old `/T` → fixed value: filled, painted as static page content,
    /// and removed from the interactive layer (pre-printed).
    pub literals: BTreeMap<String, String>,
}

/// Re-author a blank AcroForm's field layer in place: rename fields to
/// their questionnaire state paths, merge checkbox pairs into radio
/// groups, and pre-print literal values as static content. The output is
/// still a fillable blank — only the names and the pre-printed values
/// change — so `fill_acroform` + `flatten` work on it unchanged.
///
/// # Errors
///
/// - The same parse / locate failures as [`fill_acroform`].
/// - [`PdfError::UnmatchedField`] if a spec entry names no form field.
/// - [`PdfError::UnaccountedField`] if a form field is covered by no
///   spec entry — the plan must be total.
/// - [`PdfError::Reauthor`] on a structural conflict: a duplicate
///   source `/T`, a merge across mismatched field types, or a radio
///   member that is not a kid-less checkbox.
pub fn reauthor(pdf: &[u8], spec: &ReauthorSpec) -> Result<Vec<u8>, PdfError> {
    // Pre-print literals first: `fill_acroform` validates each literal
    // name and value shape exactly like a production fill would.
    let filled = if spec.literals.is_empty() {
        pdf.to_vec()
    } else {
        fill_acroform(pdf, &spec.literals)?
    };
    let mut doc = Document::load_mem(&filled).map_err(|e| PdfError::Lopdf(e.to_string()))?;
    let (acroform_id, fields) = locate_acroform(&doc)?;

    let by_name = named_fields(&doc, &fields)?;
    check_totality(spec, &by_name)?;

    // Literals: paint their filled values as static page content and
    // drop exactly those widgets from the interactive layer.
    let literal_fids: Vec<ObjectId> = spec.literals.keys().map(|n| by_name[n]).collect();
    if !literal_fids.is_empty() {
        let (texts, stamps, widget_ids) = collect_flatten_draws(&doc, &literal_fids);
        paint_overlays(&mut doc, texts, stamps)?;
        detach_fields(&mut doc, acroform_id, &widget_ids, &literal_fids);
    }

    for merge in &spec.radios {
        merge_radio(&mut doc, acroform_id, &by_name, merge)?;
    }

    apply_renames(&mut doc, acroform_id, &by_name, &spec.renames)?;

    doc.prune_objects();
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| PdfError::Lopdf(e.to_string()))?;
    Ok(out)
}

/// Name → id for the top-level fields, refusing a duplicate source `/T`
/// (which would make every by-name transform ambiguous).
fn named_fields(
    doc: &Document,
    fields: &[NamedField],
) -> Result<BTreeMap<String, ObjectId>, PdfError> {
    let mut by_name: BTreeMap<String, ObjectId> = BTreeMap::new();
    for field in fields {
        let Ok(_dict) = doc.get_object(field.id).and_then(Object::as_dict) else {
            continue;
        };
        if by_name.insert(field.name.clone(), field.id).is_some() {
            return Err(PdfError::Reauthor(format!(
                "duplicate source field `/T` `{}`",
                field.name
            )));
        }
    }
    Ok(by_name)
}

/// Every spec entry must name a real field, every field must be covered
/// by exactly one spec entry, and the *final* name set must be
/// collision-free: two surviving fields sharing a `/T` would make
/// [`fill_acroform`]'s by-name index fill only one of them — a silent
/// blank box on a filed form. Several renames sharing one target are the
/// deliberate multi-widget merge, so the collisions left to refuse are a
/// radio group named like a rename target and two radio groups sharing a
/// name (pre-printed literals leave the interactive layer, so they
/// survive as no name at all).
fn check_totality(
    spec: &ReauthorSpec,
    by_name: &BTreeMap<String, ObjectId>,
) -> Result<(), PdfError> {
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    let spec_names = spec.renames.keys().chain(spec.literals.keys()).chain(
        spec.radios
            .iter()
            .flat_map(|m| m.members.iter().map(|member| &member.field)),
    );
    for name in spec_names {
        if !by_name.contains_key(name) {
            return Err(PdfError::UnmatchedField(name.clone()));
        }
        if !covered.insert(name) {
            return Err(PdfError::Reauthor(format!(
                "field `{name}` is covered by more than one spec entry"
            )));
        }
    }
    for name in by_name.keys() {
        if !covered.contains(name.as_str()) {
            return Err(PdfError::UnaccountedField(name.clone()));
        }
    }
    let rename_targets: BTreeSet<&str> = spec.renames.values().map(String::as_str).collect();
    let mut radio_names: BTreeSet<&str> = BTreeSet::new();
    for merge in &spec.radios {
        if rename_targets.contains(merge.name.as_str()) || !radio_names.insert(&merge.name) {
            return Err(PdfError::Reauthor(format!(
                "duplicate final field `/T` `{}` — a rename target and a radio group (or two \
                 radio groups) collide, and a by-name fill would leave one a silent blank box",
                merge.name
            )));
        }
    }
    Ok(())
}

/// Remove `widget_ids` from the pages' `/Annots` and `field_ids` from
/// the AcroForm `/Fields` — the partial sibling of the full
/// [`strip_interactive_layer`], used when only the pre-printed literal
/// fields leave the interactive layer.
fn detach_fields(
    doc: &mut Document,
    acroform_id: ObjectId,
    widget_ids: &BTreeSet<ObjectId>,
    field_ids: &[ObjectId],
) {
    for page_id in doc.get_pages().values().copied().collect::<Vec<_>>() {
        let annots = match annots_slot(doc, page_id) {
            Some(AnnotsSlot::Inline(pid)) => match doc.get_dictionary_mut(pid) {
                Ok(page) => match page.get_mut(b"Annots") {
                    Ok(Object::Array(a)) => a,
                    _ => continue,
                },
                Err(_) => continue,
            },
            Some(AnnotsSlot::Indirect(id)) => match doc.objects.get_mut(&id) {
                Some(Object::Array(a)) => a,
                _ => continue,
            },
            None => continue,
        };
        annots.retain(|o| !o.as_reference().is_ok_and(|id| widget_ids.contains(&id)));
    }
    remove_from_fields_array(doc, acroform_id, field_ids);
}

/// Drop the given ids from the AcroForm `/Fields` array.
fn remove_from_fields_array(doc: &mut Document, acroform_id: ObjectId, field_ids: &[ObjectId]) {
    if let Some(Object::Dictionary(af)) = doc.objects.get_mut(&acroform_id) {
        if let Ok(Object::Array(fields)) = af.get_mut(b"Fields") {
            fields.retain(|o| !o.as_reference().is_ok_and(|id| field_ids.contains(&id)));
        }
    }
}

/// Append `field_id` to the AcroForm `/Fields` array.
fn push_to_fields_array(doc: &mut Document, acroform_id: ObjectId, field_id: ObjectId) {
    if let Some(Object::Dictionary(af)) = doc.objects.get_mut(&acroform_id) {
        if let Ok(Object::Array(fields)) = af.get_mut(b"Fields") {
            fields.push(Object::Reference(field_id));
        }
    }
}

/// Merge the named kid-less checkboxes into one radio group. Each member
/// keeps its widget dictionary (rect, page, `/AP` on-state) but loses
/// its `/T` and becomes a kid of the new parent, exactly the radio
/// structure [`fill_acroform`] and [`flatten`] already speak.
fn merge_radio(
    doc: &mut Document,
    acroform_id: ObjectId,
    by_name: &BTreeMap<String, ObjectId>,
    merge: &RadioMergeSpec,
) -> Result<(), PdfError> {
    let mut final_exports = BTreeSet::new();
    for member in &merge.members {
        if !final_exports.insert(member.final_export.as_str()) {
            return Err(PdfError::Reauthor(format!(
                "radio `{}` has duplicate final export `{}`",
                merge.name, member.final_export
            )));
        }
    }
    let member_ids: Vec<ObjectId> = merge
        .members
        .iter()
        .map(|member| by_name[&member.field])
        .collect();
    // Validate each member is a kid-less Btn checkbox with its own
    // on-state before touching anything.
    for (member, fid) in merge.members.iter().zip(&member_ids) {
        let dict = doc
            .get_object(*fid)
            .and_then(Object::as_dict)
            .map_err(|e| PdfError::Lopdf(e.to_string()))?;
        let is_btn = dict.get(b"FT").and_then(Object::as_name).ok() == Some(b"Btn");
        let kid_less = dict.get(b"Kids").is_err();
        let states = appearance_states(doc, dict);
        if !is_btn || !kid_less || !states.iter().any(|s| s == &member.source_export) {
            return Err(PdfError::Reauthor(format!(
                "radio member `{}` is not a kid-less checkbox with source export `{}`",
                member.field, member.source_export
            )));
        }
    }

    let parent_id = doc.new_object_id();
    for (member, fid) in merge.members.iter().zip(&member_ids) {
        if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(fid) {
            rename_appearance_state(dict, &member.source_export, &member.final_export);
            dict.remove(b"T");
            dict.remove(b"FT");
            dict.remove(b"V");
            dict.set("Parent", Object::Reference(parent_id));
            dict.set("AS", Object::Name(b"Off".to_vec()));
        }
    }
    let mut parent = Dictionary::new();
    parent.set("FT", Object::Name(b"Btn".to_vec()));
    parent.set(
        "T",
        Object::String(merge.name.as_bytes().to_vec(), StringFormat::Literal),
    );
    // Radio flag (bit 16) + no-toggle-to-off (bit 15), matching the
    // groups real packets carry.
    parent.set("Ff", Object::Integer(49152));
    parent.set("V", Object::Name(b"Off".to_vec()));
    parent.set(
        "Kids",
        Object::Array(member_ids.iter().copied().map(Object::Reference).collect()),
    );
    doc.objects.insert(parent_id, Object::Dictionary(parent));

    remove_from_fields_array(doc, acroform_id, &member_ids);
    push_to_fields_array(doc, acroform_id, parent_id);
    Ok(())
}

fn rename_appearance_state(widget: &mut Dictionary, source: &str, target: &str) {
    if source == target {
        return;
    }
    let Ok(Object::Dictionary(ap)) = widget.get_mut(b"AP") else {
        return;
    };
    for key in [b"N".as_slice(), b"D".as_slice(), b"R".as_slice()] {
        let Ok(Object::Dictionary(states)) = ap.get_mut(key) else {
            continue;
        };
        if let Some(value) = states.remove(source.as_bytes()) {
            states.set(target.as_bytes().to_vec(), value);
        }
    }
}

/// Apply the renames. Olds sharing one target merge into a single field
/// whose widgets are `/T`-less kids — one value, printed everywhere the
/// form repeats it (the NV packets restate the same person on several
/// pages). A single old is renamed in place.
fn apply_renames(
    doc: &mut Document,
    acroform_id: ObjectId,
    by_name: &BTreeMap<String, ObjectId>,
    renames: &BTreeMap<String, String>,
) -> Result<(), PdfError> {
    let mut by_target: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (old, new) in renames {
        by_target.entry(new).or_default().push(old);
    }
    for (target, olds) in by_target {
        if let [only] = olds.as_slice() {
            let fid = by_name[*only];
            if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(&fid) {
                dict.set(
                    "T",
                    Object::String(target.as_bytes().to_vec(), StringFormat::Literal),
                );
            }
            continue;
        }
        merge_text_fields(doc, acroform_id, by_name, target, &olds)?;
    }
    Ok(())
}

/// Merge several same-typed text fields into one parent named `target`
/// whose kids are the former fields' widgets.
fn merge_text_fields(
    doc: &mut Document,
    acroform_id: ObjectId,
    by_name: &BTreeMap<String, ObjectId>,
    target: &str,
    olds: &[&str],
) -> Result<(), PdfError> {
    let old_ids: Vec<ObjectId> = olds.iter().map(|n| by_name[*n]).collect();
    for (name, fid) in olds.iter().zip(&old_ids) {
        let dict = doc
            .get_object(*fid)
            .and_then(Object::as_dict)
            .map_err(|e| PdfError::Lopdf(e.to_string()))?;
        let is_text = dict.get(b"FT").and_then(Object::as_name).ok() == Some(b"Tx");
        let kid_less = dict.get(b"Kids").is_err();
        if !is_text || !kid_less {
            return Err(PdfError::Reauthor(format!(
                "`{name}` cannot merge into `{target}`: only kid-less text fields merge"
            )));
        }
    }
    let parent_id = doc.new_object_id();
    for fid in &old_ids {
        if let Some(Object::Dictionary(dict)) = doc.objects.get_mut(fid) {
            dict.remove(b"T");
            dict.remove(b"FT");
            dict.remove(b"V");
            dict.set("Parent", Object::Reference(parent_id));
        }
    }
    let mut parent = Dictionary::new();
    parent.set("FT", Object::Name(b"Tx".to_vec()));
    parent.set(
        "T",
        Object::String(target.as_bytes().to_vec(), StringFormat::Literal),
    );
    parent.set("V", Object::String(Vec::new(), StringFormat::Literal));
    parent.set(
        "Kids",
        Object::Array(old_ids.iter().copied().map(Object::Reference).collect()),
    );
    doc.objects.insert(parent_id, Object::Dictionary(parent));

    remove_from_fields_array(doc, acroform_id, &old_ids);
    push_to_fields_array(doc, acroform_id, parent_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        appearance_states, blank_acroform, blank_acroform_with, field_names, fill_acroform,
        flatten, locate_acroform, page_text, read_field_value, read_widget_appearance_state,
        reauthor, strip_static_xfa, widget_annotation_count, FieldSpec, RadioMergeMember,
        RadioMergeSpec, ReauthorSpec,
    };
    use crate::PdfError;
    use lopdf::content::Content;
    use lopdf::{Dictionary, Document, Object, Stream};
    use std::collections::BTreeMap;

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn radio_members(pairs: &[(&str, &str)]) -> Vec<RadioMergeMember> {
        pairs
            .iter()
            .map(|(field, export)| RadioMergeMember {
                field: (*field).to_string(),
                source_export: (*export).to_string(),
                final_export: (*export).to_string(),
            })
            .collect()
    }

    fn radio_member(field: &str, source_export: &str, final_export: &str) -> RadioMergeMember {
        RadioMergeMember {
            field: field.to_string(),
            source_export: source_export.to_string(),
            final_export: final_export.to_string(),
        }
    }

    fn with_xfa(blank: &[u8], xfa: lopdf::Object, needs_rendering: bool) -> Vec<u8> {
        let mut doc = lopdf::Document::load_mem(blank).unwrap();
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let af_id = doc
            .get_object(root)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        if let Some(lopdf::Object::Dictionary(d)) = doc.objects.get_mut(&af_id) {
            d.set("XFA", xfa);
            if needs_rendering {
                d.set("NeedsRendering", lopdf::Object::Boolean(true));
            }
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    fn with_down_and_rollover_appearances(pdf: &[u8], field: &str, source_export: &str) -> Vec<u8> {
        let mut doc = lopdf::Document::load_mem(pdf).unwrap();
        let source_export = source_export.as_bytes();
        let ids: Vec<_> = doc.objects.keys().copied().collect();
        for id in ids {
            if let Some(Object::Dictionary(widget)) = doc.objects.get_mut(&id) {
                if widget.get(b"T").and_then(Object::as_str).ok() != Some(field.as_bytes()) {
                    continue;
                }
                let Ok(Object::Dictionary(ap)) = widget.get_mut(b"AP") else {
                    continue;
                };
                let Ok(Object::Dictionary(n)) = ap.get(b"N") else {
                    continue;
                };
                let Some(appearance) = n.get(source_export).ok().cloned() else {
                    continue;
                };
                for key in ["D", "R"] {
                    let mut states = Dictionary::new();
                    states.set(source_export.to_vec(), appearance.clone());
                    ap.set(key, Object::Dictionary(states));
                }
            }
        }
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    fn radio_kid_appearance_states(
        pdf: &[u8],
        field: &str,
        kid_index: usize,
        ap_key: &[u8],
    ) -> Vec<String> {
        let doc = lopdf::Document::load_mem(pdf).unwrap();
        let (_, fields) = locate_acroform(&doc).unwrap();
        let parent = fields
            .iter()
            .find(|named| named.name == field)
            .expect("radio parent field");
        let parent = doc
            .get_object(parent.id)
            .and_then(Object::as_dict)
            .expect("radio parent dict");
        let kid = parent
            .get(b"Kids")
            .and_then(Object::as_array)
            .expect("kids")
            .get(kid_index)
            .expect("kid index")
            .as_reference()
            .expect("kid reference");
        let kid = doc
            .get_object(kid)
            .and_then(Object::as_dict)
            .expect("kid dict");
        let states = kid
            .get(b"AP")
            .and_then(Object::as_dict)
            .and_then(|ap| ap.get(ap_key))
            .and_then(Object::as_dict)
            .expect("appearance states");
        states
            .iter()
            .map(|(name, _)| String::from_utf8_lossy(name).into_owned())
            .collect()
    }

    fn acroform_has_xfa(pdf: &[u8]) -> bool {
        let doc = lopdf::Document::load_mem(pdf).unwrap();
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let af_id = doc
            .get_object(root)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_object(af_id)
            .unwrap()
            .as_dict()
            .unwrap()
            .has(b"XFA")
    }

    #[test]
    fn fill_populates_field_values_that_round_trip() {
        let blank = blank_acroform(&["entity_name", "registered_agent"]);
        let filled = fill_acroform(
            &blank,
            &fields(&[
                ("entity_name", "Neon Law LLC"),
                ("registered_agent", "Jane Doe"),
            ]),
        )
        .expect("fill succeeds");

        assert_eq!(
            read_field_value(&filled, "entity_name").as_deref(),
            Some("Neon Law LLC")
        );
        assert_eq!(
            read_field_value(&filled, "registered_agent").as_deref(),
            Some("Jane Doe")
        );
    }

    #[test]
    fn fill_sets_need_appearances_so_viewers_regenerate() {
        let blank = blank_acroform(&["x"]);
        let filled = fill_acroform(&blank, &fields(&[("x", "v")])).unwrap();
        let doc = lopdf::Document::load_mem(&filled).unwrap();
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let catalog = doc.get_object(root).unwrap().as_dict().unwrap();
        let af_id = catalog.get(b"AcroForm").unwrap().as_reference().unwrap();
        let af = doc.get_object(af_id).unwrap().as_dict().unwrap();
        assert!(af.get(b"NeedAppearances").unwrap().as_bool().unwrap());
    }

    #[test]
    fn unmatched_field_is_a_loud_error_not_a_silent_drop() {
        let blank = blank_acroform(&["known"]);
        let err = fill_acroform(&blank, &fields(&[("nonexistent", "v")])).unwrap_err();
        match err {
            PdfError::UnmatchedField(name) => assert_eq!(name, "nonexistent"),
            other => panic!("expected UnmatchedField, got {other:?}"),
        }
    }

    #[test]
    fn static_hybrid_xfa_fills_through_its_acroform_layer() {
        let blank = with_xfa(&blank_acroform(&["a"]), lopdf::Object::Array(vec![]), false);

        let filled = fill_acroform(&blank, &fields(&[("a", "v")])).unwrap();

        assert_eq!(read_field_value(&filled, "a").as_deref(), Some("v"));
    }

    #[test]
    fn xfa_form_is_rejected_loudly() {
        let blank = with_xfa(&blank_acroform(&["a"]), lopdf::Object::Array(vec![]), true);

        let err = fill_acroform(&blank, &fields(&[("a", "v")])).unwrap_err();
        assert!(matches!(err, PdfError::XfaUnsupported));
    }

    #[test]
    fn xfa_dynamic_render_config_is_rejected_loudly() {
        let config = lopdf::Object::String(
            b"<config><dynamicRender>required</dynamicRender></config>".to_vec(),
            lopdf::StringFormat::Literal,
        );
        let blank = with_xfa(&blank_acroform(&["a"]), config, false);

        let err = fill_acroform(&blank, &fields(&[("a", "v")])).unwrap_err();
        assert!(matches!(err, PdfError::XfaUnsupported));
    }

    #[test]
    fn xfa_dynamic_render_config_rejects_normal_xml_whitespace() {
        let config = lopdf::Object::String(
            b"<config>\n  <dynamicRender>\n    required\n  </dynamicRender>\n</config>".to_vec(),
            lopdf::StringFormat::Literal,
        );
        let blank = with_xfa(&blank_acroform(&["a"]), config, false);

        let err = strip_static_xfa(&blank).unwrap_err();
        assert!(matches!(err, PdfError::XfaUnsupported));
    }

    #[test]
    fn xfa_dynamic_render_config_in_compressed_stream_is_rejected() {
        // A dynamic XFA config packet is virtually always
        // FlateDecode-compressed, so scanning the raw stream bytes never
        // sees `<dynamicRender>required</dynamicRender>`. The guard must
        // decode the stream before matching; otherwise a dynamic form
        // that does not also set `/NeedsRendering` is misread as static,
        // stripped, and filled to a visually blank filing.
        let mut stream = lopdf::Stream::new(
            lopdf::Dictionary::new(),
            b"<config xmlns=\"http://www.xfa.org/schema/xci/3.0/\">\
              <present><pdf><interactive>1</interactive>\
              <renderPolicy>client</renderPolicy></pdf>\
              <script><runScripts>server</runScripts></script>\
              <dynamicRender>required</dynamicRender></present></config>"
                .to_vec(),
        );
        stream.compress().expect("compress config packet");
        assert!(
            !stream
                .content
                .windows(b"dynamicRender".len())
                .any(|w| w == b"dynamicRender"),
            "fixture must be compressed so the decode path is what rejects it"
        );

        // A PDF stream must be an indirect object; the XFA array holds a
        // `(name, config-stream ref)` pair the way real packets do.
        let mut doc = lopdf::Document::load_mem(&blank_acroform(&["a"])).unwrap();
        let config_ref = doc.add_object(lopdf::Object::Stream(stream));
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let af_id = doc
            .get_object(root)
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"AcroForm")
            .unwrap()
            .as_reference()
            .unwrap();
        if let Some(lopdf::Object::Dictionary(d)) = doc.objects.get_mut(&af_id) {
            d.set(
                "XFA",
                lopdf::Object::Array(vec![
                    lopdf::Object::string_literal("config"),
                    lopdf::Object::Reference(config_ref),
                ]),
            );
        }
        let mut blank = Vec::new();
        doc.save_to(&mut blank).unwrap();

        let err = strip_static_xfa(&blank).unwrap_err();
        assert!(matches!(err, PdfError::XfaUnsupported));
    }

    #[test]
    fn strip_static_xfa_removes_xfa_and_preserves_fields() {
        let blank = blank_acroform(&["a"]);
        let hybrid = with_xfa(&blank, lopdf::Object::Array(vec![]), false);
        assert!(acroform_has_xfa(&hybrid));

        let stripped = strip_static_xfa(&hybrid).unwrap();

        assert!(!acroform_has_xfa(&stripped));
        assert_eq!(field_names(&stripped).unwrap(), vec!["a".to_string()]);
        assert_eq!(
            widget_annotation_count(&stripped).unwrap(),
            widget_annotation_count(&blank).unwrap()
        );
        let filled = fill_acroform(&stripped, &fields(&[("a", "v")])).unwrap();
        assert_eq!(read_field_value(&filled, "a").as_deref(), Some("v"));
    }

    #[test]
    fn strip_static_xfa_leaves_pure_acroforms_unchanged_by_contract() {
        let blank = blank_acroform(&["a"]);

        let stripped = strip_static_xfa(&blank).unwrap();

        assert_eq!(field_names(&stripped).unwrap(), vec!["a".to_string()]);
        assert_eq!(
            widget_annotation_count(&stripped).unwrap(),
            widget_annotation_count(&blank).unwrap()
        );
    }

    #[test]
    #[ignore = "requires N400_PDF=/path/to/downloaded/uscis-n-400.pdf"]
    fn real_n400_static_xfa_probe() {
        let path = std::env::var("N400_PDF").expect("set N400_PDF");
        let blank = std::fs::read(path).expect("read N-400");
        let stripped = strip_static_xfa(&blank).expect("static XFA strips");
        let names = field_names(&stripped).expect("field names");
        println!("field_count={}", names.len());
        println!(
            "widget_count={}",
            widget_annotation_count(&stripped).expect("widget count")
        );
        println!("stripped_sha256={}", sha256_for_test(&stripped));
        println!(
            "first_fields={:?}",
            names.iter().take(20).collect::<Vec<_>>()
        );
        if let Ok(path) = std::env::var("N400_STRIPPED_OUT") {
            std::fs::write(path, &stripped).expect("write stripped N-400");
        }
        if let Ok(path) = std::env::var("N400_FIELDS_OUT") {
            std::fs::write(path, names.join("\n") + "\n").expect("write N-400 fields");
        }
        if let Ok(path) = std::env::var("N400_FIELD_INFO_OUT") {
            std::fs::write(path, field_info_for_test(&stripped)).expect("write N-400 field info");
        }
        assert_eq!(names.len(), 440);
        assert_eq!(widget_annotation_count(&stripped).unwrap(), 440);
        assert!(!acroform_has_xfa(&stripped));
    }

    fn sha256_for_test(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(bytes);
        let mut out = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    fn field_info_for_test(pdf: &[u8]) -> String {
        use std::fmt::Write;

        let doc = lopdf::Document::load_mem(pdf).expect("parse stripped");
        let (_, fields) = locate_acroform(&doc).expect("locate fields");
        let mut out = String::new();
        for field in fields {
            let dict = doc
                .get_object(field.id)
                .and_then(lopdf::Object::as_dict)
                .expect("field dict");
            let ft = dict
                .get(b"FT")
                .and_then(lopdf::Object::as_name)
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .unwrap_or_default();
            let states = appearance_states(&doc, dict);
            let _ = writeln!(out, "{}\t{}\t{:?}", field.name, ft, states);
        }
        out
    }

    #[test]
    fn a_pdf_without_acroform_errors() {
        // A Typst-rendered PDF has no AcroForm.
        let plain = crate::render("Hello, no form here.").unwrap();
        let err = fill_acroform(&plain, &fields(&[("a", "v")])).unwrap_err();
        assert!(matches!(err, PdfError::NoAcroForm));
    }

    #[test]
    fn garbage_bytes_are_a_parse_error() {
        let err = fill_acroform(b"not a pdf at all", &fields(&[("a", "v")])).unwrap_err();
        assert!(matches!(err, PdfError::Lopdf(_)));
    }

    #[test]
    fn checkbox_checks_with_its_arbitrary_on_state() {
        // Real packets use on-states like `managers`, `NRS 86`, `1` —
        // never assume `Yes`.
        let blank = blank_acroform_with(&[FieldSpec::Checkbox {
            name: "managers_a".into(),
            on_state: "managers".into(),
        }]);
        let filled = fill_acroform(&blank, &fields(&[("managers_a", "managers")])).unwrap();
        assert_eq!(
            read_field_value(&filled, "managers_a").as_deref(),
            Some("managers")
        );
        assert_eq!(
            read_widget_appearance_state(&filled, "managers_a", None).as_deref(),
            Some("managers"),
            "/AS must show the box as visibly checked, not just /V"
        );
    }

    #[test]
    fn checkbox_unchecks_with_off() {
        let blank = blank_acroform_with(&[FieldSpec::Checkbox {
            name: "x".into(),
            on_state: "Yes".into(),
        }]);
        let filled = fill_acroform(&blank, &fields(&[("x", "Off")])).unwrap();
        assert_eq!(read_field_value(&filled, "x").as_deref(), Some("Off"));
    }

    #[test]
    fn radio_group_selects_one_kid_and_offs_the_rest() {
        // The packets' processing-request groups: a /T-named parent,
        // /T-less kids each carrying one on-state.
        let blank = blank_acroform_with(&[FieldSpec::Radio {
            name: "processing".into(),
            options: vec!["Regular".into(), "24HOUR Expedite".into()],
        }]);
        let filled = fill_acroform(&blank, &fields(&[("processing", "24HOUR Expedite")])).unwrap();
        assert_eq!(
            read_field_value(&filled, "processing").as_deref(),
            Some("24HOUR Expedite")
        );
        assert_eq!(
            read_widget_appearance_state(&filled, "processing", Some(0)).as_deref(),
            Some("Off"),
            "the unchosen kid must be /AS Off"
        );
        assert_eq!(
            read_widget_appearance_state(&filled, "processing", Some(1)).as_deref(),
            Some("24HOUR Expedite"),
            "the chosen kid must carry the selected state"
        );
    }

    #[test]
    fn invalid_choice_is_a_loud_error_with_the_allowed_states() {
        let blank = blank_acroform_with(&[FieldSpec::Radio {
            name: "processing".into(),
            options: vec!["Regular".into(), "1HOUR Expedite".into()],
        }]);
        let err = fill_acroform(&blank, &fields(&[("processing", "Overnight")])).unwrap_err();
        match err {
            PdfError::InvalidChoice {
                field,
                value,
                allowed,
            } => {
                assert_eq!(field, "processing");
                assert_eq!(value, "Overnight");
                assert_eq!(allowed, vec!["1HOUR Expedite", "Regular"]);
            }
            other => panic!("expected InvalidChoice, got {other:?}"),
        }
    }

    #[test]
    fn pushbutton_with_no_states_rejects_any_value() {
        // A pushbutton (PRINT, Get_Checklist) has no /AP states — any
        // attempt to fill it must fail with an empty allowed list.
        let blank = blank_acroform_with(&[FieldSpec::Checkbox {
            name: "PRINT".into(),
            on_state: "ignored".into(),
        }]);
        // Strip the /AP to model a pushbutton.
        let mut doc = lopdf::Document::load_mem(&blank).unwrap();
        let ids: Vec<_> = doc.objects.keys().copied().collect();
        for id in ids {
            if let Some(lopdf::Object::Dictionary(d)) = doc.objects.get_mut(&id) {
                if d.get(b"T").and_then(lopdf::Object::as_str).ok() == Some(b"PRINT".as_slice()) {
                    d.remove(b"AP");
                }
            }
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let err = fill_acroform(&bytes, &fields(&[("PRINT", "clicked")])).unwrap_err();
        assert!(matches!(
            err,
            PdfError::InvalidChoice { allowed, .. } if allowed.is_empty()
        ));
    }

    #[test]
    fn mixed_text_checkbox_radio_fill_in_one_pass() {
        let blank = blank_acroform_with(&[
            FieldSpec::Text {
                name: "entity_name".into(),
            },
            FieldSpec::Checkbox {
                name: "series".into(),
                on_state: "1".into(),
            },
            FieldSpec::Radio {
                name: "management".into(),
                options: vec!["managers".into(), "members".into()],
            },
        ]);
        let filled = fill_acroform(
            &blank,
            &fields(&[
                ("entity_name", "Neon Law LLC"),
                ("series", "1"),
                ("management", "members"),
            ]),
        )
        .unwrap();
        assert_eq!(
            read_field_value(&filled, "entity_name").as_deref(),
            Some("Neon Law LLC")
        );
        assert_eq!(read_field_value(&filled, "series").as_deref(), Some("1"));
        assert_eq!(
            read_field_value(&filled, "management").as_deref(),
            Some("members")
        );
    }

    #[test]
    fn flatten_removes_fields_but_keeps_text_as_page_content() {
        let blank = blank_acroform(&["entity_name", "registered_agent"]);
        let filled = fill_acroform(
            &blank,
            &fields(&[
                ("entity_name", "Neon Law LLC"),
                ("registered_agent", "Jane Doe"),
            ]),
        )
        .unwrap();
        assert!(
            !field_names(&filled).unwrap().is_empty(),
            "filled form is still interactive before flattening"
        );

        let flat = flatten(&filled).unwrap();
        assert!(
            field_names(&flat).unwrap().is_empty(),
            "flattening leaves no interactive fields to re-edit"
        );
        assert_eq!(
            widget_annotation_count(&flat).unwrap(),
            0,
            "no widget annotation survives for a viewer to rebuild a field from"
        );
        let text = page_text(&flat).unwrap();
        assert!(text.contains("Neon Law LLC"), "value survives as page text");
        assert!(text.contains("Jane Doe"), "value survives as page text");
        // The stored /V is gone with the field, so a form reader sees none.
        assert_eq!(read_field_value(&flat, "entity_name"), None);
    }

    /// Rewrite each page's inline `/Annots` array into an indirect
    /// object — the shape every real vendored NV packet uses.
    fn with_indirect_annots(pdf: &[u8]) -> Vec<u8> {
        let mut doc = Document::load_mem(pdf).unwrap();
        for page_id in doc.get_pages().values().copied().collect::<Vec<_>>() {
            let arr = match doc.get_dictionary(page_id).unwrap().get(b"Annots") {
                Ok(Object::Array(a)) => a.clone(),
                _ => continue,
            };
            let arr_id = doc.add_object(Object::Array(arr));
            doc.get_dictionary_mut(page_id)
                .unwrap()
                .set("Annots", Object::Reference(arr_id));
        }
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn flatten_strips_widgets_behind_an_indirect_annots_array() {
        let blank = with_indirect_annots(&blank_acroform(&["entity_name"]));
        let filled = fill_acroform(&blank, &fields(&[("entity_name", "Neon Law LLC")])).unwrap();
        assert!(
            widget_annotation_count(&filled).unwrap() > 0,
            "the fixture carries a widget behind the indirect array"
        );

        let flat = flatten(&filled).unwrap();
        assert_eq!(
            widget_annotation_count(&flat).unwrap(),
            0,
            "an indirect /Annots array must be dereferenced and stripped, \
             or every widget survives flattening"
        );
        assert!(
            page_text(&flat).unwrap().contains("Neon Law LLC"),
            "value survives as page text"
        );
    }

    #[test]
    fn flatten_writes_win_ansi_bytes_with_a_declared_encoding() {
        let blank = blank_acroform(&["name"]);
        let filled = fill_acroform(&blank, &fields(&[("name", "José Núñez")])).unwrap();
        let flat = flatten(&filled).unwrap();

        // The overlay font must declare the encoding its bytes use…
        let doc = Document::load_mem(&flat).unwrap();
        let declares = doc.objects.values().any(|o| {
            o.as_dict().is_ok_and(|d| {
                d.get(b"BaseFont").and_then(Object::as_name).ok() == Some(b"Helvetica")
                    && d.get(b"Encoding").and_then(Object::as_name).ok() == Some(b"WinAnsiEncoding")
            })
        });
        assert!(declares, "overlay Helvetica must declare /WinAnsiEncoding");

        // …and the Tj operand must be WinAnsi bytes (é = 0xE9), not raw
        // UTF-8 (0xC3 0xA9), which StandardEncoding-era viewers would
        // render as two unrelated glyphs.
        let mut operands: Vec<Vec<u8>> = Vec::new();
        for pid in doc.page_iter() {
            for sid in doc.get_page_contents(pid) {
                let Ok(stream) = doc.get_object(sid).and_then(Object::as_stream) else {
                    continue;
                };
                let data = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                let Ok(content) = Content::decode(&data) else {
                    continue;
                };
                for op in content.operations {
                    if op.operator == "Tj" {
                        if let Some(Object::String(b, _)) = op.operands.first() {
                            operands.push(b.clone());
                        }
                    }
                }
            }
        }
        assert!(
            operands
                .iter()
                .any(|b| b.as_slice() == b"Jos\xe9 N\xfa\xf1ez"),
            "overlay text must be WinAnsi-encoded, got {operands:?}"
        );

        // The round-trip guard reads it back as the original text.
        assert!(page_text(&flat).unwrap().contains("José Núñez"));
    }

    #[test]
    fn flatten_rejects_a_value_outside_win_ansi_loudly() {
        // A glyph WinAnsi cannot carry must fail the flatten, never
        // silently garble a packet on its way to a government office.
        let blank = blank_acroform(&["name"]);
        let filled = fill_acroform(&blank, &fields(&[("name", "日本商事")])).unwrap();
        let err = flatten(&filled).unwrap_err();
        assert!(
            matches!(err, PdfError::UnencodableChar { ch: '日', .. }),
            "expected UnencodableChar, got {err:?}"
        );
    }

    #[test]
    fn flatten_is_idempotent() {
        let blank = blank_acroform(&["x"]);
        let filled = fill_acroform(&blank, &fields(&[("x", "hello")])).unwrap();
        let once = flatten(&filled).unwrap();
        let twice = flatten(&once).unwrap();
        assert!(field_names(&twice).unwrap().is_empty());
        assert!(
            page_text(&twice).unwrap().contains("hello"),
            "re-flattening a flat form neither errors nor drops content"
        );
    }

    #[test]
    fn flatten_without_an_acroform_is_a_loud_error() {
        // A Typst-rendered PDF has no AcroForm to flatten.
        let plain = crate::render("Nothing to flatten here.").unwrap();
        assert!(matches!(flatten(&plain).unwrap_err(), PdfError::NoAcroForm));
    }

    #[test]
    fn flatten_stamps_a_checked_box_appearance_onto_the_page() {
        // A checkbox whose on-state carries a real appearance stream: after
        // flattening, the field is gone but its appearance is drawn onto the
        // page (a `Do` of the stamped XObject), so the visible check survives.
        let blank = blank_acroform_with(&[FieldSpec::Checkbox {
            name: "box".into(),
            on_state: "Yes".into(),
        }]);
        let mut doc = Document::load_mem(&blank).unwrap();
        let mut sdict = Dictionary::new();
        sdict.set("Type", Object::Name(b"XObject".to_vec()));
        sdict.set("Subtype", Object::Name(b"Form".to_vec()));
        sdict.set(
            "BBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(14),
                Object::Integer(14),
            ]),
        );
        let ap_id = doc.add_object(Object::Stream(Stream::new(
            sdict,
            b"q 0 0 0 rg 2 2 10 10 re f Q".to_vec(),
        )));
        // Point the checkbox's /AP /N /Yes at the real stream.
        let ids: Vec<_> = doc.objects.keys().copied().collect();
        for id in ids {
            if let Some(Object::Dictionary(d)) = doc.objects.get_mut(&id) {
                if d.get(b"T").and_then(Object::as_str).ok() == Some(b"box".as_slice()) {
                    if let Ok(Object::Dictionary(ap)) = d.get_mut(b"AP") {
                        if let Ok(Object::Dictionary(n)) = ap.get_mut(b"N") {
                            n.set("Yes", Object::Reference(ap_id));
                        }
                    }
                }
            }
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();

        let filled = fill_acroform(&bytes, &fields(&[("box", "Yes")])).unwrap();
        assert_eq!(
            read_widget_appearance_state(&filled, "box", None).as_deref(),
            Some("Yes")
        );

        let flat = flatten(&filled).unwrap();
        assert!(field_names(&flat).unwrap().is_empty());
        let out = Document::load_mem(&flat).unwrap();
        let stamped = out
            .page_iter()
            .any(|pid| String::from_utf8_lossy(&out.get_page_content(pid)).contains("NavFlatX0"));
        assert!(
            stamped,
            "the checked box's appearance must be stamped onto the page as an XObject"
        );
    }

    fn spec_renaming(pairs: &[(&str, &str)]) -> ReauthorSpec {
        ReauthorSpec {
            renames: fields(pairs),
            ..ReauthorSpec::default()
        }
    }

    #[test]
    fn reauthor_renames_a_field_in_place() {
        let blank = blank_acroform(&["1 Name of Entity"]);
        let out = reauthor(
            &blank,
            &spec_renaming(&[("1 Name of Entity", "entity__company.name")]),
        )
        .expect("reauthor succeeds");
        assert_eq!(field_names(&out).unwrap(), vec!["entity__company.name"]);
        let filled = fill_acroform(&out, &fields(&[("entity__company.name", "Neon LLC")])).unwrap();
        assert_eq!(
            read_field_value(&filled, "entity__company.name").as_deref(),
            Some("Neon LLC")
        );
    }

    #[test]
    fn reauthor_merges_same_target_renames_into_one_multiwidget_field() {
        // The NV packets restate the same person on several pages; two
        // olds → one target must become one field with two kid widgets.
        let blank = blank_acroform(&["Name3", "organizer name"]);
        let out = reauthor(
            &blank,
            &spec_renaming(&[
                ("Name3", "people__managing_members.0.name"),
                ("organizer name", "people__managing_members.0.name"),
            ]),
        )
        .expect("reauthor succeeds");
        assert_eq!(
            field_names(&out).unwrap(),
            vec!["people__managing_members.0.name"]
        );
        // Both widgets survive, and one fill paints in both places.
        assert_eq!(widget_annotation_count(&out).unwrap(), 2);
        let filled = fill_acroform(
            &out,
            &fields(&[("people__managing_members.0.name", "Ada Organizer")]),
        )
        .unwrap();
        let flat = flatten(&filled).unwrap();
        assert_eq!(widget_annotation_count(&flat).unwrap(), 0);
        let text = page_text(&flat).unwrap();
        assert_eq!(text.matches("Ada Organizer").count(), 2, "{text}");
    }

    #[test]
    fn reauthor_merges_a_checkbox_pair_into_a_radio_group() {
        let blank = blank_acroform_with(&[
            FieldSpec::Checkbox {
                name: "managers_a".into(),
                on_state: "managers".into(),
            },
            FieldSpec::Checkbox {
                name: "managers_b".into(),
                on_state: "members".into(),
            },
        ]);
        let out = reauthor(
            &blank,
            &ReauthorSpec {
                radios: vec![RadioMergeSpec {
                    name: "custom_single_choice__management_structure".into(),
                    members: radio_members(&[
                        ("managers_a", "managers"),
                        ("managers_b", "members"),
                    ]),
                }],
                ..ReauthorSpec::default()
            },
        )
        .expect("reauthor succeeds");
        assert_eq!(
            field_names(&out).unwrap(),
            vec!["custom_single_choice__management_structure"]
        );
        let filled = fill_acroform(
            &out,
            &fields(&[("custom_single_choice__management_structure", "members")]),
        )
        .expect("radio fill succeeds");
        assert_eq!(
            read_widget_appearance_state(
                &filled,
                "custom_single_choice__management_structure",
                Some(1)
            )
            .as_deref(),
            Some("members")
        );
        assert_eq!(
            read_widget_appearance_state(
                &filled,
                "custom_single_choice__management_structure",
                Some(0)
            )
            .as_deref(),
            Some("Off")
        );
    }

    #[test]
    fn reauthor_can_rename_radio_exports_to_answer_keys() {
        let blank = blank_acroform_with(&[
            FieldSpec::Checkbox {
                name: "five_year_box".into(),
                on_state: "A".into(),
            },
            FieldSpec::Checkbox {
                name: "three_year_box".into(),
                on_state: "B".into(),
            },
        ]);
        let blank = with_down_and_rollover_appearances(&blank, "five_year_box", "A");
        let out = reauthor(
            &blank,
            &ReauthorSpec {
                radios: vec![RadioMergeSpec {
                    name: "custom_single_choice__eligibility_basis".into(),
                    members: vec![
                        radio_member("five_year_box", "A", "five_year"),
                        radio_member("three_year_box", "B", "three_year_marriage"),
                    ],
                }],
                ..ReauthorSpec::default()
            },
        )
        .expect("reauthor succeeds");

        let filled = fill_acroform(
            &out,
            &fields(&[("custom_single_choice__eligibility_basis", "five_year")]),
        )
        .expect("radio fill succeeds");
        assert_eq!(
            read_widget_appearance_state(
                &filled,
                "custom_single_choice__eligibility_basis",
                Some(0)
            )
            .as_deref(),
            Some("five_year")
        );
        for key in [b"N".as_slice(), b"D".as_slice(), b"R".as_slice()] {
            let states = radio_kid_appearance_states(
                &out,
                "custom_single_choice__eligibility_basis",
                0,
                key,
            );
            assert!(
                states.iter().any(|state| state == "five_year"),
                "{states:?}"
            );
            assert!(!states.iter().any(|state| state == "A"), "{states:?}");
        }
    }

    #[test]
    fn reauthor_refuses_duplicate_radio_final_exports() {
        let blank = blank_acroform_with(&[
            FieldSpec::Checkbox {
                name: "yes_a".into(),
                on_state: "A".into(),
            },
            FieldSpec::Checkbox {
                name: "yes_b".into(),
                on_state: "B".into(),
            },
        ]);

        let err = reauthor(
            &blank,
            &ReauthorSpec {
                radios: vec![RadioMergeSpec {
                    name: "custom_single_choice__x".into(),
                    members: vec![
                        radio_member("yes_a", "A", "yes"),
                        radio_member("yes_b", "B", "yes"),
                    ],
                }],
                ..ReauthorSpec::default()
            },
        )
        .expect_err("duplicate final exports must be refused");
        assert!(
            matches!(err, PdfError::Reauthor(ref msg) if msg.contains("duplicate final export")),
            "{err:?}"
        );
    }

    #[test]
    fn reauthor_preprints_literals_as_static_content() {
        let blank = blank_acroform(&["formation_1", "entity_name"]);
        let out = reauthor(
            &blank,
            &ReauthorSpec {
                renames: fields(&[("entity_name", "entity__company.name")]),
                literals: fields(&[("formation_1", "NRS 86")]),
                ..ReauthorSpec::default()
            },
        )
        .expect("reauthor succeeds");
        // The literal left the interactive layer; the renamed field stays.
        assert_eq!(field_names(&out).unwrap(), vec!["entity__company.name"]);
        assert_eq!(widget_annotation_count(&out).unwrap(), 1);
        assert!(page_text(&out).unwrap().contains("NRS 86"));
        // The output is still a fillable blank.
        let filled = fill_acroform(&out, &fields(&[("entity__company.name", "Neon LLC")])).unwrap();
        let flat = flatten(&filled).unwrap();
        let text = page_text(&flat).unwrap();
        assert!(text.contains("NRS 86"), "{text}");
        assert!(text.contains("Neon LLC"), "{text}");
    }

    #[test]
    fn reauthor_refuses_an_unaccounted_field() {
        let blank = blank_acroform(&["mapped", "forgotten"]);
        let err = reauthor(
            &blank,
            &spec_renaming(&[("mapped", "entity__company.name")]),
        )
        .expect_err("must refuse");
        assert!(
            matches!(err, PdfError::UnaccountedField(ref n) if n == "forgotten"),
            "{err:?}"
        );
    }

    #[test]
    fn reauthor_refuses_a_spec_entry_naming_no_field() {
        let blank = blank_acroform(&["real"]);
        let err = reauthor(&blank, &spec_renaming(&[("real", "a"), ("ghost", "b")]))
            .expect_err("must refuse");
        assert!(
            matches!(err, PdfError::UnmatchedField(ref n) if n == "ghost"),
            "{err:?}"
        );
    }

    #[test]
    fn reauthor_refuses_a_rename_target_colliding_with_a_radio_name() {
        // A map that both checks a box for a choice and prints the same
        // choice as text can emit this collision: two final fields named
        // alike, of which `fill_acroform`'s by-name index fills only one
        // — a silent blank box on a filed form.
        let blank = blank_acroform_with(&[
            FieldSpec::Text {
                name: "choice as text".into(),
            },
            FieldSpec::Checkbox {
                name: "managers_a".into(),
                on_state: "managers".into(),
            },
            FieldSpec::Checkbox {
                name: "managers_b".into(),
                on_state: "members".into(),
            },
        ]);
        let err = reauthor(
            &blank,
            &ReauthorSpec {
                renames: fields(&[(
                    "choice as text",
                    "custom_single_choice__management_structure",
                )]),
                radios: vec![RadioMergeSpec {
                    name: "custom_single_choice__management_structure".into(),
                    members: radio_members(&[
                        ("managers_a", "managers"),
                        ("managers_b", "members"),
                    ]),
                }],
                ..ReauthorSpec::default()
            },
        )
        .expect_err("must refuse the duplicate final name");
        assert!(
            matches!(err, PdfError::Reauthor(ref msg) if msg.contains("duplicate final")),
            "{err:?}"
        );
    }

    #[test]
    fn reauthor_refuses_two_radio_groups_sharing_a_name() {
        let blank = blank_acroform_with(&[
            FieldSpec::Checkbox {
                name: "a".into(),
                on_state: "yes".into(),
            },
            FieldSpec::Checkbox {
                name: "b".into(),
                on_state: "no".into(),
            },
        ]);
        let err = reauthor(
            &blank,
            &ReauthorSpec {
                radios: vec![
                    RadioMergeSpec {
                        name: "custom_single_choice__x".into(),
                        members: radio_members(&[("a", "yes")]),
                    },
                    RadioMergeSpec {
                        name: "custom_single_choice__x".into(),
                        members: radio_members(&[("b", "no")]),
                    },
                ],
                ..ReauthorSpec::default()
            },
        )
        .expect_err("must refuse the duplicate final name");
        assert!(
            matches!(err, PdfError::Reauthor(ref msg) if msg.contains("duplicate final")),
            "{err:?}"
        );
    }
}
