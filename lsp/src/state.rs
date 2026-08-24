//! In-memory document cache + JSON-RPC dispatcher. Keeping I/O out
//! of this module so unit tests can drive request/response purely
//! over `Server::handle_message`.

// `lsp_types::Uri` contains an `UnsafeCell` for percent-decode
// memoization; clippy can't see that the cell is never mutated
// through our `HashMap` keys (we own them by value and never
// reach inside). Allow the lint at module scope.
#![allow(clippy::mutable_key_type)]

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CompletionItem, CompletionItemKind,
    Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    Hover, HoverContents, MarkupContent, MarkupKind, OneOf, PublishDiagnosticsParams,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkspaceEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostics::{lint_buffer, violation_to_diagnostic};
use crate::position::range_to_lsp_range;

/// In-memory state of the language server: one entry per open
/// document. The cache mirrors what the editor has on screen —
/// `didChange` notifications replace it; `didClose` evicts it.
#[derive(Debug, Default)]
pub struct Server {
    documents: HashMap<Uri, String>,
}

/// A single outgoing message produced by the server in response to
/// some incoming traffic. Either a JSON-RPC response (paired with
/// an `id`) or a server-originated notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    Response { id: Value, result: Value },
    Notification { method: String, params: Value },
}

impl Server {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Static capability advertisement returned from `initialize`.
    /// Kept as a free function so tests can assert the shape without
    /// constructing a `Server`.
    #[must_use]
    pub fn capabilities() -> ServerCapabilities {
        ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            code_action_provider: Some(lsp_types::CodeActionProviderCapability::Options(
                lsp_types::CodeActionOptions {
                    code_action_kinds: Some(vec![
                        CodeActionKind::QUICKFIX,
                        CodeActionKind::SOURCE_FIX_ALL,
                    ]),
                    resolve_provider: Some(false),
                    work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
                },
            )),
            hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
            completion_provider: Some(lsp_types::CompletionOptions::default()),
            ..ServerCapabilities::default()
        }
    }

    /// Completion at `position`: inside a template's `questionnaire:` block,
    /// offer the registered `<type>` vocabulary (the Slice-4 registry,
    /// mirrored in `rules`) so authors compose `<type>__<role>` states from
    /// real types. Empty everywhere else.
    #[must_use]
    pub fn completion(&self, uri: &Uri, position: lsp_types::Position) -> Vec<CompletionItem> {
        let Some(text) = self.documents.get(uri) else {
            return Vec::new();
        };
        let Some(byte) = position_to_byte(text, position) else {
            return Vec::new();
        };
        // On the `kind:` line, offer the document kinds a Markdown file
        // may declare. Asset-lane-only values (`transcript`,
        // `unclassified`) classify a filed byte artifact and would be
        // nonsense in frontmatter, so the completion filters by lane
        // rather than mirroring the whole enum.
        if on_top_level_key_line(text, byte, "kind") {
            return rules::Kind::ALL
                .iter()
                .filter(|k| k.valid_for(rules::kind::Lane::Template))
                .map(|k| CompletionItem {
                    label: k.as_str().to_string(),
                    kind: Some(CompletionItemKind::VALUE),
                    detail: Some(k.describe().to_string()),
                    ..CompletionItem::default()
                })
                .collect();
        }
        if !in_frontmatter_block(text, byte, "questionnaire:") {
            return Vec::new();
        }
        rules::REGISTERED_QUESTION_TYPES
            .iter()
            .map(|ty| CompletionItem {
                label: (*ty).to_string(),
                kind: Some(CompletionItemKind::VALUE),
                detail: rules::describe_question_type(ty),
                ..CompletionItem::default()
            })
            .collect()
    }

    /// Open a document and return the diagnostics-publish
    /// notification an LSP server is expected to emit.
    pub fn did_open(&mut self, params: DidOpenTextDocumentParams) -> Outgoing {
        let uri = params.text_document.uri.clone();
        self.documents
            .insert(uri.clone(), params.text_document.text);
        self.publish_diagnostics(&uri)
    }

    /// Replace a document's content (FULL sync) and re-publish.
    pub fn did_change(&mut self, params: DidChangeTextDocumentParams) -> Outgoing {
        let uri = params.text_document.uri.clone();
        if let Some(change) = params.content_changes.into_iter().next() {
            self.documents.insert(uri.clone(), change.text);
        }
        self.publish_diagnostics(&uri)
    }

    pub fn did_close(&mut self, params: DidCloseTextDocumentParams) {
        self.documents.remove(&params.text_document.uri);
    }

    fn publish_diagnostics(&self, uri: &Uri) -> Outgoing {
        let text = self.documents.get(uri).cloned().unwrap_or_default();
        let diagnostics = self.diagnostics_for(uri, &text);
        let params = PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics,
            version: None,
        };
        Outgoing::Notification {
            method: "textDocument/publishDiagnostics".to_string(),
            params: serde_json::to_value(params).expect("serialize publish diagnostics"),
        }
    }

    fn diagnostics_for(&self, uri: &Uri, text: &str) -> Vec<Diagnostic> {
        let path = uri_to_path(uri);
        let (_file, violations) = lint_buffer(path, text.to_string());
        violations
            .iter()
            .map(|v| violation_to_diagnostic(text, v))
            .collect()
    }

    /// Compute every quick-fix code action that intersects the
    /// requested range, plus a single `source.fixAll` action that
    /// applies every safe-by-construction autofix.
    pub fn code_actions(&self, uri: &Uri, range: lsp_types::Range) -> Vec<CodeActionOrCommand> {
        let Some(text) = self.documents.get(uri) else {
            return Vec::new();
        };
        let path = uri_to_path(uri);
        let (file, violations) = lint_buffer(path, text.clone());
        let rule_set: Vec<Box<dyn rules::Rule>> = rules::navigator_classified_rules(&file);
        let mut actions = Vec::new();
        let mut all_edits: Vec<(rules::TextEdit, &'static str)> = Vec::new();
        for v in &violations {
            let rule = rule_set.iter().find(|r| r.code() == v.code);
            let Some(rule) = rule else { continue };
            let Some(edit) = rule.fix(&file, v) else {
                continue;
            };
            let lsp_range = range_to_lsp_range(text, &v.range);
            if intersects(&range, &lsp_range) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("{}: fix", v.code),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![violation_to_diagnostic(text, v)]),
                    edit: Some(workspace_edit(uri, text, std::slice::from_ref(&edit))),
                    command: None,
                    is_preferred: Some(true),
                    disabled: None,
                    data: None,
                }));
            }
            all_edits.push((edit, v.code));
        }
        if !all_edits.is_empty() {
            // Sort by start asc + code asc; drop overlaps keeping the
            // lower-coded edit; this mirrors `cli validate --fix`.
            all_edits.sort_by(|a, b| a.0.range.start.cmp(&b.0.range.start).then(a.1.cmp(b.1)));
            let mut kept: Vec<rules::TextEdit> = Vec::with_capacity(all_edits.len());
            for (edit, _) in all_edits {
                if let Some(prev) = kept.last() {
                    if edit.range.start < prev.range.end {
                        continue;
                    }
                }
                kept.push(edit);
            }
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "Neon Law Navigator: fix all auto-fixable problems".to_string(),
                kind: Some(CodeActionKind::SOURCE_FIX_ALL),
                diagnostics: None,
                edit: Some(workspace_edit(uri, text, &kept)),
                command: None,
                is_preferred: None,
                disabled: None,
                data: None,
            }));
        }
        actions
    }

    /// Hover at `position`. Complementary sources, tried in order:
    ///
    /// 1. **Workflow step doc** — when the cursor is on a state name in a
    ///    template's `workflow:` block, show what that step actually does
    ///    (from the `rules::workflow_steps` catalog), the way hovering a
    ///    function shows its doc.
    /// 2. **Questionnaire state doc** — the effective prompt a respondent
    ///    sees and where its wording comes from.
    /// 3. **Change-surface doc** — on a choice key under a GitHub
    ///    notation's `change_surface` question, what that surface means and
    ///    which gate it implies.
    /// 4. **Document-kind doc** — on the `kind:` value, what family the
    ///    file belongs to.
    /// 5. **Violation** — if any violation's range covers the byte, show
    ///    the rule description + message.
    ///
    /// When a doc and a violation both apply (e.g. hovering `lawyer_review`,
    /// which has both a catalog entry and an N112 advisory), the doc comes
    /// first, then the violation below a rule.
    pub fn hover(&self, uri: &Uri, position: lsp_types::Position) -> Option<Hover> {
        let text = self.documents.get(uri)?;
        let path = uri_to_path(uri);
        let byte = position_to_byte(text, position)?;

        let step = workflow_step_hover(text, byte)
            .or_else(|| question_type_hover(text, byte))
            .or_else(|| change_surface_hover(text, byte))
            .or_else(|| kind_value_hover(text, byte));
        let (_file, violations) = lint_buffer(path, text.clone());
        let violation = violations
            .iter()
            .find(|v| v.range.start <= byte && byte <= v.range.end);

        let (range, body) = match (step, violation) {
            (Some((range, doc)), Some(v)) => (
                range,
                format!(
                    "{doc}\n\n---\n\n**{}** — {}\n\n{}",
                    v.code,
                    rules::description_for_code(v.code),
                    v.message,
                ),
            ),
            (Some((range, doc)), None) => (range, doc),
            (None, Some(v)) => (
                v.range.clone(),
                format!(
                    "**{}** — {}\n\n{}",
                    v.code,
                    rules::description_for_code(v.code),
                    v.message,
                ),
            ),
            (None, None) => return None,
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: body,
            }),
            range: Some(range_to_lsp_range(text, &range)),
        })
    }
}

/// Hover doc for a workflow step, when `byte` falls on a state name
/// inside a template's `workflow:` block. Returns the token's byte range
/// and the markdown body (`**prefix** — status\n\nsummary`), or `None`
/// when the cursor isn't on a known step in that block.
fn workflow_step_hover(text: &str, byte: usize) -> Option<(std::ops::Range<usize>, String)> {
    if !in_frontmatter_block(text, byte, "workflow:") {
        return None;
    }
    let range = identifier_at(text, byte)?;
    let step = rules::lookup_workflow_step(&text[range.clone()])?;
    let body = format!(
        "**{}** — {}\n\n{}",
        step.prefix,
        step.status.label(),
        step.summary,
    );
    Some((range, body))
}

/// Hover doc for a questionnaire state, when `byte` falls on a state name
/// inside a template's `questionnaire:` block. Shows the effective prompt the
/// respondent will see and where its wording comes from (custom / overridden
/// / inherited from the question bank), plus the `<type>` registry
/// description — the way hovering a workflow step shows what it does. Falls
/// back to the bare type description for an unregistered or bare state.
fn question_type_hover(text: &str, byte: usize) -> Option<(std::ops::Range<usize>, String)> {
    if !in_frontmatter_block(text, byte, "questionnaire:") {
        return None;
    }
    let range = identifier_at(text, byte)?;
    let state = &text[range.clone()];
    if let Some(body) = rules::questionnaire_prompt_hover(text, state) {
        return Some((range, body));
    }
    let ty = state.split_once("__").map_or(state, |(t, _)| t);
    let body = rules::describe_question_type(ty)?;
    Some((range, body))
}

/// Hover doc for a change-surface choice key, when `byte` falls on one of
/// the tokens under a GitHub notation's
/// `custom_questions.change_surface.choices`.
///
/// The surface is the field that decides which gate the work has to clear,
/// so an author picking a value sees the consequence rather than guessing
/// from a two-word label. Scoped to the `choices:` mapping of that one
/// question, so the word `web` elsewhere in the frontmatter stays inert.
fn change_surface_hover(text: &str, byte: usize) -> Option<(std::ops::Range<usize>, String)> {
    if !in_frontmatter_block(text, byte, "custom_questions:") {
        return None;
    }
    let range = identifier_at(text, byte)?;
    if !under_change_surface_choices(text, range.start) {
        return None;
    }
    rules::describe_change_surface(&text[range.clone()]).map(|doc| (range, doc))
}

/// True when the line holding `byte` is an entry under
/// `custom_questions.<change-surface role>.choices:`.
///
/// Walks back through progressively shallower parent keys the way a YAML
/// reader would, so it recognizes the nesting rather than pattern-matching
/// a fixed indentation.
fn under_change_surface_choices(text: &str, byte: usize) -> bool {
    let Some(role) = rules::CHANGE_SURFACE_STATE.split_once("__").map(|(_, r)| r) else {
        return false;
    };
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let mut indent = leading_width(line_at(text, line_start));
    let mut parent_is_choices = false;
    for line in text[..line_start].lines().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let parent_indent = leading_width(line);
        if parent_indent >= indent {
            continue;
        }
        indent = parent_indent;
        let key = line.trim().trim_end_matches(':');
        if parent_is_choices {
            // The grandparent must be the change-surface question itself.
            return key == role;
        }
        if key != "choices" {
            return false;
        }
        parent_is_choices = true;
    }
    false
}

/// Hover doc for the declared `kind:` value — what family the file belongs
/// to, and therefore which rules it is held to. The same one-liner the
/// `kind:` value completion offers.
fn kind_value_hover(text: &str, byte: usize) -> Option<(std::ops::Range<usize>, String)> {
    if !on_top_level_key_line(text, byte, "kind") {
        return None;
    }
    let range = identifier_at(text, byte)?;
    let token = &text[range.clone()];
    // The key shares the line with its value; only the value has a doc.
    if token == "kind" {
        return None;
    }
    let kind = rules::Kind::parse(token)?;
    let mut body = format!("**`kind: {token}`** — {}", kind.describe());
    // The GitHub shelf is closed, so naming both files and what each opens
    // answers the next question the author has.
    if kind == rules::Kind::Github {
        for (stem, _) in rules::GITHUB_NOTATIONS {
            if let Some(doc) = rules::describe_github_notation(stem) {
                body.push_str("\n\n- ");
                body.push_str(&doc);
            }
        }
    }
    Some((range, body))
}

/// The line beginning at byte offset `start`, without its newline.
fn line_at(text: &str, start: usize) -> &str {
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    &text[start..end]
}

/// The number of leading whitespace bytes on `line` — its YAML nesting depth.
fn leading_width(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The maximal `[A-Za-z0-9_]` run covering `byte` (a state name is one
/// such token). `None` if the byte isn't on a word character.
fn identifier_at(text: &str, byte: usize) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = byte.min(bytes.len());
    let mut end = start;
    while end < bytes.len() && is_word(bytes[end]) {
        end += 1;
    }
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    (start != end).then_some(start..end)
}

/// True when `byte` lies on an indented line inside the frontmatter's
/// top-level `block_key` block (e.g. `workflow:` or `questionnaire:`) — so a
/// body word or a word under a different key doesn't pick up that block's
/// hover.
fn in_frontmatter_block(text: &str, byte: usize, block_key: &str) -> bool {
    if !text.starts_with("---\n") {
        return false;
    }
    let fm_start = 4;
    let Some(rel_end) = text[fm_start..].find("\n---") else {
        return false;
    };
    let fm_end = fm_start + rel_end;
    if byte < fm_start || byte >= fm_end {
        return false;
    }
    let mut in_block = false;
    let mut offset = fm_start;
    for line in text[fm_start..fm_end].split_inclusive('\n') {
        let line_end = offset + line.len();
        let indented = line.starts_with([' ', '\t']);
        if !indented && line.trim_end().ends_with(':') {
            // A top-level key resets the block: we're in it only under
            // `block_key` itself.
            in_block = line.trim_end() == block_key;
        }
        if byte >= offset && byte < line_end {
            return in_block && indented;
        }
        offset = line_end;
    }
    false
}

/// True when `byte` lies on a top-level (non-indented) frontmatter line
/// whose key is `key` (e.g. `kind`) — so value completion for a scalar
/// key fires on that line and nowhere else.
fn on_top_level_key_line(text: &str, byte: usize, key: &str) -> bool {
    if !text.starts_with("---\n") {
        return false;
    }
    let fm_start = 4;
    let Some(rel_end) = text[fm_start..].find("\n---") else {
        return false;
    };
    // `fm_end` is the newline that begins the `\n---` closer — i.e. the
    // end of the last frontmatter content line. A cursor at end-of-line
    // there (`byte == fm_end`) is still on that line, so include it.
    let fm_end = fm_start + rel_end;
    if byte < fm_start || byte > fm_end {
        return false;
    }
    // Isolate the line containing `byte` and test it directly, so a cursor
    // at the very end of the last frontmatter line still resolves.
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[byte..].find('\n').map_or(text.len(), |i| byte + i);
    let line = &text[line_start..line_end];
    !line.starts_with([' ', '\t']) && line.trim_start().starts_with(&format!("{key}:"))
}

fn intersects(a: &lsp_types::Range, b: &lsp_types::Range) -> bool {
    !(b.end < a.start || a.end < b.start)
}

/// Best-effort `file:///…` URI → local path. Falls back to a
/// placeholder so the rules engine has something to record in
/// `Violation.path`.
fn uri_to_path(uri: &Uri) -> PathBuf {
    let s = uri.as_str();
    if let Some(rest) = s.strip_prefix("file://") {
        let decoded = percent_decode(rest);
        return PathBuf::from(decoded);
    }
    PathBuf::from(s)
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                #[allow(clippy::cast_possible_truncation)]
                out.push(char::from((h * 16 + l) as u8));
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn workspace_edit(uri: &Uri, text: &str, edits: &[rules::TextEdit]) -> WorkspaceEdit {
    let lsp_edits: Vec<OneOf<lsp_types::TextEdit, lsp_types::AnnotatedTextEdit>> = edits
        .iter()
        .map(|e| {
            OneOf::Left(lsp_types::TextEdit {
                range: range_to_lsp_range(text, &e.range),
                new_text: e.new_text.clone(),
            })
        })
        .collect();
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        lsp_edits
            .into_iter()
            .map(|e| match e {
                OneOf::Left(te) => te,
                OneOf::Right(_) => unreachable!("only constructed Left variants"),
            })
            .collect(),
    );
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

fn position_to_byte(text: &str, position: lsp_types::Position) -> Option<usize> {
    let mut line: u32 = 0;
    let mut line_start = 0usize;
    for (i, ch) in text.char_indices() {
        if line == position.line {
            line_start = i;
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = i + ch.len_utf8();
        }
    }
    if line != position.line && !text.is_empty() {
        return None;
    }
    let mut character: u32 = 0;
    let mut byte = line_start;
    while byte < text.len() && character < position.character {
        let ch = text[byte..].chars().next()?;
        character += u32::try_from(ch.len_utf16()).ok()?;
        byte += ch.len_utf8();
        if ch == '\n' {
            break;
        }
    }
    Some(byte)
}

/// Shape of an `initialize` response payload — exposed so tests can
/// snapshot it without a real connection.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
}

impl InitializeResult {
    #[must_use]
    pub fn default_payload() -> Self {
        Self {
            capabilities: Server::capabilities(),
            server_info: Some(ServerInfo {
                name: "navigator-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InitializeResult, Outgoing, Server};
    use lsp_types::{
        DidOpenTextDocumentParams, Position, Range, TextDocumentItem, Uri,
        VersionedTextDocumentIdentifier,
    };
    use std::str::FromStr;

    fn open(server: &mut Server, uri: &str, text: &str) {
        let uri = Uri::from_str(uri).unwrap();
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: "markdown".to_string(),
                version: 0,
                text: text.to_string(),
            },
        };
        let out = server.did_open(params);
        if let Outgoing::Notification { method, .. } = &out {
            assert_eq!(method, "textDocument/publishDiagnostics");
        } else {
            panic!("expected notification, got {out:?}");
        }
    }

    #[test]
    fn capabilities_advertise_full_sync_quickfix_and_hover() {
        let caps = Server::capabilities();
        assert!(matches!(
            caps.text_document_sync,
            Some(lsp_types::TextDocumentSyncCapability::Kind(
                lsp_types::TextDocumentSyncKind::FULL,
            ))
        ));
        match caps.code_action_provider {
            Some(lsp_types::CodeActionProviderCapability::Options(ref o)) => {
                let kinds = o.code_action_kinds.as_ref().unwrap();
                assert!(kinds.contains(&lsp_types::CodeActionKind::QUICKFIX));
                assert!(kinds.contains(&lsp_types::CodeActionKind::SOURCE_FIX_ALL));
            }
            _ => panic!("expected code-action options"),
        }
        assert!(matches!(
            caps.hover_provider,
            Some(lsp_types::HoverProviderCapability::Simple(true))
        ));
    }

    #[test]
    fn initialize_result_default_names_the_server() {
        let r = InitializeResult::default_payload();
        let info = r.server_info.unwrap();
        assert_eq!(info.name, "navigator-lsp");
        assert!(info.version.is_some());
    }

    /// A notation template with a questionnaire + workflow block and one
    /// typed questionnaire state, for the completion/hover/diagnostics tests.
    const TEMPLATE: &str = "---\nkind: retainer\ntitle: T\nquestionnaire:\n  BEGIN:\n    _: entity__company\n  entity__company:\n    _: END\n  END: {}\nworkflow:\n  BEGIN:\n    _: lawyer_review\n  lawyer_review:\n    _: END\n  END: {}\n---\n\nBody.\n";

    #[test]
    fn capabilities_advertise_completion() {
        assert!(Server::capabilities().completion_provider.is_some());
    }

    #[test]
    fn completion_offers_registered_types_inside_the_questionnaire_block() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", TEMPLATE);
        let uri = Uri::from_str("file:///t.md").unwrap();
        // Line 4 (0-based) is `    _: entity__company`, inside questionnaire.
        let items = server.completion(
            &uri,
            Position {
                line: 4,
                character: 6,
            },
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"entity"), "labels: {labels:?}");
        assert!(labels.contains(&"custom_datetime"), "labels: {labels:?}");
        assert!(labels.contains(&"people"), "labels: {labels:?}");
    }

    #[test]
    fn completion_is_empty_outside_the_questionnaire_block() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", TEMPLATE);
        let uri = Uri::from_str("file:///t.md").unwrap();
        // Line 16 is the body line `Body.` — not in the questionnaire block.
        let items = server.completion(
            &uri,
            Position {
                line: 16,
                character: 1,
            },
        );
        assert!(items.is_empty(), "got {} items", items.len());
    }

    #[test]
    fn completion_offers_kind_values_on_a_kind_line() {
        let mut server = Server::new();
        open(
            &mut server,
            "file:///t.md",
            "---\ntitle: T\nkind: \n---\n\nBody.\n",
        );
        let uri = Uri::from_str("file:///t.md").unwrap();
        // Line 2 (0-based) is `kind: `; cursor after the colon.
        let items = server.completion(
            &uri,
            Position {
                line: 2,
                character: 6,
            },
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"retainer"), "labels: {labels:?}");
        assert!(labels.contains(&"letter"), "labels: {labels:?}");
        assert!(labels.contains(&"event"), "labels: {labels:?}");
        // The new instrument + workshop kinds are offered too.
        assert!(labels.contains(&"will"), "labels: {labels:?}");
        assert!(labels.contains(&"workshop"), "labels: {labels:?}");
        // Completion mirrors the *template lane*, in step with the enum.
        assert_eq!(labels, rules::kind::VALID, "labels: {labels:?}");
        // An asset-lane-only kind is never offered: a transcript is filed
        // on a matter, not declared in a Markdown file's frontmatter.
        assert!(!labels.contains(&"transcript"), "labels: {labels:?}");
        assert!(!labels.contains(&"unclassified"), "labels: {labels:?}");
    }

    #[test]
    fn diagnostics_flag_an_unrecognized_kind() {
        let server = Server::new();
        let uri = Uri::from_str("file:///t.md").unwrap();
        let diags = server.diagnostics_for(&uri, "---\ntitle: T\nkind: bogus\n---\n");
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("Invalid `kind:` value")),
            "expected an S103 diagnostic, got {diags:?}",
        );
    }

    /// A GitHub notation at its canonical path, so `N119` is satisfied and
    /// a hover assertion is not competing with a violation body. The
    /// `change_summary` prompt deliberately contains the word "web" outside
    /// the `choices:` mapping — the negative case for the scoping.
    const GITHUB_NOTATION: &str = "---
kind: github
title: Create a GitHub Issue
questionnaire:
  BEGIN:
    _: custom_single_choice__change_surface
  custom_single_choice__change_surface:
    _: custom_yes_no__engineering_council
  custom_yes_no__engineering_council:
    _: custom_text__change_summary
  custom_text__change_summary:
    _: END
  END: {}
custom_questions:
  change_surface:
    prompt: What does this change touch?
    choices:
      web: Web feature
      api: API feature
      infrastructure: Infrastructure
      form: Government form
  engineering_council:
    prompt: Should the Engineering Council convene?
  change_summary:
    prompt: Describe the web change.
---

Body.
";

    const GITHUB_URI: &str = "file:///templates/github/create_issue.md";

    fn github_hover(line: u32, character: u32) -> Option<String> {
        let mut server = Server::new();
        open(&mut server, GITHUB_URI, GITHUB_NOTATION);
        let uri = Uri::from_str(GITHUB_URI).unwrap();
        server
            .hover(&uri, Position { line, character })
            .map(|h| match h.contents {
                lsp_types::HoverContents::Markup(m) => m.value,
                other => panic!("expected markup hover, got {other:?}"),
            })
    }

    /// Hovering a change-surface choice explains what that surface means and
    /// which gate it implies — the whole point of the closed option list.
    #[test]
    fn hover_explains_each_change_surface_choice() {
        // Lines 17–20 (0-based) are the four `      <key>: <label>` entries.
        for (line, key, expected) in [
            (17, "web", "browser"),
            (18, "api", "AIDA"),
            (19, "infrastructure", "Kubernetes"),
            (20, "form", "templates/forms/"),
        ] {
            let body = github_hover(line, 7)
                .unwrap_or_else(|| panic!("expected a hover for `{key}` on line {line}"));
            assert!(body.contains(key), "`{key}` hover: {body}");
            assert!(body.contains(expected), "`{key}` hover: {body}");
        }
    }

    /// The scoping is real: the same word under a different question's
    /// `prompt:` is prose, not a change-surface choice.
    #[test]
    fn hover_ignores_a_surface_word_outside_the_choices_mapping() {
        // Line 24 is `    prompt: Describe the web change.` — hover `web`.
        let body = github_hover(24, 26);
        assert!(
            body.as_deref().is_none_or(|b| !b.contains("Web feature")),
            "prose word picked up the change-surface doc: {body:?}",
        );
    }

    /// Hovering the declared `kind:` value says what family the file is in,
    /// and therefore which rules hold it.
    #[test]
    fn hover_explains_the_declared_kind() {
        // Line 1 is `kind: github`; hover inside the value.
        let body = github_hover(1, 8).expect("expected a kind hover");
        assert!(body.contains("kind: github"), "got: {body}");
        assert!(body.contains("GitHub issue or pull request"), "got: {body}");
        // The shelf is closed, so the hover names both files it may hold.
        assert!(body.contains("create_issue"), "got: {body}");
        assert!(body.contains("create_pull_request"), "got: {body}");
    }

    /// The `kind` key shares its line with the value; only the value is
    /// documented, so hovering the key itself yields no kind doc.
    #[test]
    fn hover_on_the_kind_key_itself_has_no_kind_doc() {
        let body = github_hover(1, 1);
        assert!(
            body.as_deref()
                .is_none_or(|b| !b.contains("engineering intake notation")),
            "the key should carry no kind doc: {body:?}",
        );
    }

    /// The canonical GitHub notation fixture is clean: hovering a plain body
    /// word surfaces nothing, which is only true when no rule is flagging it.
    #[test]
    fn the_canonical_github_notation_fixture_raises_no_diagnostics() {
        let server = Server::new();
        let uri = Uri::from_str(GITHUB_URI).unwrap();
        let diags = server.diagnostics_for(&uri, GITHUB_NOTATION);
        let blocking: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
            .collect();
        assert!(blocking.is_empty(), "expected none, got {blocking:?}");
    }

    /// The GitHub contract is enforced in the editor: a third file on the
    /// shelf underlines with N119 rather than waiting for CI.
    #[test]
    fn diagnostics_flag_a_github_notation_with_the_wrong_filename() {
        let server = Server::new();
        let uri = Uri::from_str("file:///templates/github/create_discussion.md").unwrap();
        let diags = server.diagnostics_for(&uri, GITHUB_NOTATION);
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("create_pull_request")),
            "expected an N119 filename diagnostic, got {diags:?}",
        );
    }

    #[test]
    fn hover_shows_the_question_type_over_a_questionnaire_state() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", TEMPLATE);
        let uri = Uri::from_str("file:///t.md").unwrap();
        // Line 6 (0-based) is `  entity__company:`; hover inside `entity`.
        let h = server
            .hover(
                &uri,
                Position {
                    line: 6,
                    character: 4,
                },
            )
            .expect("expected a question-type hover");
        match h.contents {
            lsp_types::HoverContents::Markup(m) => {
                assert!(
                    m.value.contains("registered question type"),
                    "got: {}",
                    m.value
                );
                assert!(m.value.contains("entity"), "got: {}", m.value);
            }
            other => panic!("expected markup hover, got {other:?}"),
        }
    }

    /// A template whose `entity__company` state has no `prompts:` override —
    /// hover should surface the bank's inherited wording and its provenance.
    #[test]
    fn hover_shows_the_effective_prompt_and_provenance() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", TEMPLATE);
        let uri = Uri::from_str("file:///t.md").unwrap();
        // Line 6 (0-based) is `  entity__company:`; hover inside `entity`.
        let h = server
            .hover(
                &uri,
                Position {
                    line: 6,
                    character: 4,
                },
            )
            .expect("expected a prompt hover");
        let lsp_types::HoverContents::Markup(m) = h.contents else {
            panic!("expected markup hover");
        };
        assert!(m.value.contains("**Prompt**"), "got: {}", m.value);
        assert!(
            m.value.contains("inherited from the question bank"),
            "got: {}",
            m.value
        );
        // The bank `entity` prompt with the `company` role substituted in.
        assert!(
            m.value.contains("Which entity is company?"),
            "got: {}",
            m.value
        );
    }

    /// A bare (roleless) registered-type state carries no resolvable prompt,
    /// so hover falls back to the type's registry description.
    #[test]
    fn hover_falls_back_to_the_type_description_for_a_bare_state() {
        let bare = "---\ntitle: T\nquestionnaire:\n  BEGIN:\n    _: person\n  person:\n    _: END\n  END: {}\nworkflow:\n  BEGIN:\n    _: lawyer_review\n  lawyer_review:\n    _: END\n  END: {}\n---\n\nBody.\n";
        let mut server = Server::new();
        open(&mut server, "file:///bare.md", bare);
        let uri = Uri::from_str("file:///bare.md").unwrap();
        // Line 5 (0-based) is `  person:`; hover inside the bare `person`.
        let h = server
            .hover(
                &uri,
                Position {
                    line: 5,
                    character: 4,
                },
            )
            .expect("expected a type-description hover");
        let lsp_types::HoverContents::Markup(m) = h.contents else {
            panic!("expected markup hover");
        };
        assert!(!m.value.contains("**Prompt**"), "got: {}", m.value);
        assert!(
            m.value.contains("registered question type"),
            "got: {}",
            m.value
        );
    }

    #[test]
    fn diagnostics_surface_n113_for_an_unregistered_typed_state() {
        let mut server = Server::new();
        let uri = Uri::from_str("file:///bad.md").unwrap();
        let bad = TEMPLATE.replace("entity__company", "bogus__company");
        let out = server.did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: "markdown".to_string(),
                version: 0,
                text: bad,
            },
        });
        let Outgoing::Notification { params, .. } = out else {
            panic!("expected notification");
        };
        let codes: Vec<String> = params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["code"].as_str().map(str::to_string))
            .collect();
        assert!(
            codes.iter().any(|c| c == "N113"),
            "expected N113 in {codes:?}"
        );
    }

    #[test]
    fn did_open_emits_diagnostics_for_a_buffer_with_a_violation() {
        let mut server = Server::new();
        let uri = Uri::from_str("file:///t.md").unwrap();
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: "markdown".to_string(),
                version: 0,
                text: "ok\n\thard tab\n".to_string(),
            },
        };
        let out = server.did_open(params);
        let Outgoing::Notification { method, params } = out else {
            panic!("expected notification")
        };
        assert_eq!(method, "textDocument/publishDiagnostics");
        let v: lsp_types::PublishDiagnosticsParams = serde_json::from_value(params).unwrap();
        assert_eq!(v.uri, uri);
        assert!(v.diagnostics.iter().any(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(s)) => s == "M010",
            _ => false,
        }));
    }

    #[test]
    fn did_change_replaces_buffer_and_clears_fixed_violations() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", "ok\n\thard tab\n");
        let uri = Uri::from_str("file:///t.md").unwrap();
        let out = server.did_change(lsp_types::DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 1,
            },
            content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "ok\n  hard tab\n".to_string(),
            }],
        });
        let Outgoing::Notification { params, .. } = out else {
            panic!("expected notification")
        };
        let v: lsp_types::PublishDiagnosticsParams = serde_json::from_value(params).unwrap();
        assert!(
            !v.diagnostics.iter().any(|d| matches!(
                &d.code,
                Some(lsp_types::NumberOrString::String(s)) if s == "M010"
            )),
            "M010 should be gone after the fix",
        );
    }

    #[test]
    fn code_actions_include_quickfix_and_source_fix_all() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", "ok\n\thard tab\n");
        let uri = Uri::from_str("file:///t.md").unwrap();
        let actions = server.code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 10,
                },
            },
        );
        let kinds: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                lsp_types::CodeActionOrCommand::CodeAction(ca) => ca.kind.clone(),
                lsp_types::CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(kinds.contains(&lsp_types::CodeActionKind::QUICKFIX));
        assert!(kinds.contains(&lsp_types::CodeActionKind::SOURCE_FIX_ALL));
    }

    #[test]
    fn hover_returns_rule_description_when_over_a_violation() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", "ok\n\thard tab\n");
        let uri = Uri::from_str("file:///t.md").unwrap();
        // Line 2 (0-indexed: 1), character 0 — the tab.
        let h = server
            .hover(
                &uri,
                Position {
                    line: 1,
                    character: 0,
                },
            )
            .expect("expected hover");
        match h.contents {
            lsp_types::HoverContents::Markup(m) => {
                assert!(m.value.contains("M010"), "got: {}", m.value);
                assert!(
                    m.value.contains("Hard tabs"),
                    "should include rule description, got: {}",
                    m.value
                );
            }
            _ => panic!("expected markup hover"),
        }
    }

    #[test]
    fn hover_returns_none_outside_any_violation() {
        let mut server = Server::new();
        open(&mut server, "file:///t.md", "ok\n  no tab\n");
        let uri = Uri::from_str("file:///t.md").unwrap();
        assert!(server
            .hover(
                &uri,
                Position {
                    line: 0,
                    character: 1
                },
            )
            .is_none());
        let _ = uri;
    }

    // A minimal notation template whose `workflow:` block names two
    // steps. Line numbers (0-indexed) used by the hover tests below:
    //   5  `  generate_pdf__pdf:`
    //   7  `  lawyer_review:`
    // `kind:` is declared last so it classifies as a notation template
    // (the N112 advisory hover depends on that) without shifting the
    // line-numbered workflow states the hover tests target above.
    const WORKFLOW_FIXTURE: &str = "---\n\
title: T\n\
workflow:\n  \
  BEGIN:\n    \
    created: generate_pdf__pdf\n  \
  generate_pdf__pdf:\n    \
    persisted: lawyer_review\n  \
  lawyer_review:\n    \
    approved: END\n  \
  END: {}\n\
kind: filing\n\
---\n\nBody.\n";

    fn hover_markup(server: &Server, uri: &Uri, line: u32, character: u32) -> Option<String> {
        match server.hover(uri, Position { line, character })?.contents {
            lsp_types::HoverContents::Markup(m) => Some(m.value),
            _ => panic!("expected markup hover"),
        }
    }

    #[test]
    fn hover_over_a_workflow_step_shows_what_it_does() {
        let mut server = Server::new();
        open(&mut server, "file:///wf.md", WORKFLOW_FIXTURE);
        let uri = Uri::from_str("file:///wf.md").unwrap();
        // Line 5 `  generate_pdf__pdf:`, inside the token.
        let v = hover_markup(&server, &uri, 5, 6).expect("step hover");
        assert!(v.contains("generate_pdf"), "got: {v}");
        assert!(v.contains("Implemented"), "should show status, got: {v}");
        assert!(v.contains("PDF"), "should describe what it does, got: {v}");
    }

    #[test]
    fn hover_over_lawyer_review_shows_step_doc_and_the_n112_advisory() {
        let mut server = Server::new();
        open(&mut server, "file:///wf.md", WORKFLOW_FIXTURE);
        let uri = Uri::from_str("file:///wf.md").unwrap();
        // Line 7 `  lawyer_review:`, inside the token.
        let v = hover_markup(&server, &uri, 7, 5).expect("step hover");
        assert!(v.contains("lawyer_review"), "got: {v}");
        assert!(v.contains("attorney"), "step summary, got: {v}");
        // lawyer_review also carries the yellow N112 advisory — both show.
        assert!(v.contains("N112"), "should also surface N112, got: {v}");
    }

    #[test]
    fn hover_over_a_non_step_token_in_the_workflow_block_is_none() {
        let mut server = Server::new();
        open(&mut server, "file:///wf.md", WORKFLOW_FIXTURE);
        let uri = Uri::from_str("file:///wf.md").unwrap();
        // Line 3 `  BEGIN:` — a marker, not a catalog step, no violation.
        assert!(hover_markup(&server, &uri, 3, 3).is_none());
    }
}
