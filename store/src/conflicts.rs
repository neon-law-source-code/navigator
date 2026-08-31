//! Pre-matter conflict check — the graph traversal that runs *before* a
//! Project is created.
//!
//! # The shape of the problem
//!
//! A law firm may not open a matter that is adverse to, or improperly
//! entangled with, a client it already serves (Model Rules 1.7 / 1.9,
//! imputed firm-wide by 1.10). Answering "would this new matter
//! conflict?" is a **graph reachability** question: start from the
//! proposed client and the proposed entity, walk the relationships, and
//! see whether you arrive at another party the firm already represents —
//! especially across an `adverse_to` edge.
//!
//! # Where the graph lives
//!
//! In the store. `entity_role` and `relationship` are Surreal-resident
//! tables, and this module traverses them on the deployment's own
//! connection — one source of truth, one schema file. A small firm's
//! whole relationship graph is a few thousand edges, so a dedicated graph
//! database beside the store would only be a second copy to keep in sync.
//!
//! Both are gone. What survives is the part that was always the point:
//! the traversal below is the same SurrealQL it was when it ran against
//! the projection, because the projection was deliberately written in
//! the shape the persistent store would hold.
//!
//! # This module only reads
//!
//! The traversal is `LET` and `SELECT` and nothing else. That mattered
//! less when the graph was a throwaway database — a stray write went
//! nowhere — and matters more now that the statements run against the
//! live store. [`tests::the_check_writes_nothing`] pins it.
//!
//! # What the engine does and what Rust does
//!
//! The engine walks: one bounded query collects every edge within
//! [`MAX_HOPS`] of the two anchors. Rust scores: the confidence-product
//! along a path, the floors, and the shortest-path-preferring revisit
//! rule stay here, because they decide *which* paths are worth
//! following and that is a judgment about conflicts, not about graphs.
//!
//! # Who may see what
//!
//! A conflict check reads **across matters, unscoped** — the parties it
//! looks for are by definition on someone else's matter, which is what
//! imputed firm-wide checking under Model Rule 1.10 means. The engine
//! does not narrow this and is not asked to: Navigator's authorization
//! lives above the database (#1145), and the containment is that only
//! firm-side create paths call in here. See
//! [`docs/access-model.md`](https://github.com/neon-law-source-code/navigator/blob/main/docs/access-model.md).
//!
//! # What feeds the graph
//!
//! - `entity_role` — structural ties (a person manages / owns / is a
//!   member of an entity). Always present, always confidence 100.
//! - `relationship` — the supplemental typed edges: adversity,
//!   related-party ties, and edges an LLM parsed out of a Relationship
//!   Log's detail. Each carries its own confidence and provenance.
//!
//! Findings are **advisory to clear, authoritative to block**: a
//! confident, direct `adverse_to` link to an existing client is a hard
//! block; everything else is surfaced for a human to adjudicate (the
//! firm's standing `@cleared` discipline). The graph can *raise* a
//! conflict; only a person can *clear* one — because the graph is never
//! known to be complete.

use std::collections::{HashMap, HashSet, VecDeque};

use surrealdb::types::SurrealValue;
use uuid::Uuid;
use String;

use crate::relationships::{Endpoint, KIND_ADVERSE_TO};
use crate::surreal::SurrealDb;

/// How many relationship hops out from the proposed matter the check
/// explores. Three reaches "my counterparty's affiliate's owner" — deep
/// enough for imputation without drowning lawyer in distant noise.
const MAX_HOPS: usize = 3;

/// A path weaker than this (after multiplying edge confidences) is
/// dropped — a chain of low-confidence guesses should not raise a
/// finding on its own.
const REVIEW_FLOOR_PCT: i32 = 25;

/// A `Block` requires at least this much confidence. Below it, even an
/// adverse link is downgraded to `Review` rather than hard-stopping the
/// open on a shaky edge.
const BLOCK_FLOOR_PCT: i32 = 80;

/// A `Block` requires the adverse counterparty to be this close — a
/// direct or one-removed adversity. Distant adversity is a review item.
const BLOCK_MAX_HOPS: usize = 2;

/// A node in the conflict graph: a typed reference to one `person` or
/// `entity` row.
///
/// The kind is [`Endpoint`], which `store::relationships` owns because
/// the engine enforces it there — the relation's `FROM person|entity TO
/// person|entity` refuses anything else at write time, so this module
/// no longer re-checks an endpoint kind it read back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeRef {
    pub kind: Endpoint,
    pub id: Uuid,
}

impl NodeRef {
    /// A `person` node.
    #[must_use]
    pub fn person(id: Uuid) -> Self {
        Self {
            kind: Endpoint::Person,
            id,
        }
    }

    /// An `entity` node.
    #[must_use]
    pub fn entity(id: Uuid) -> Self {
        Self {
            kind: Endpoint::Entity,
            id,
        }
    }
}

/// How serious a finding is. `Block` hard-stops the automated open;
/// `Review` surfaces the finding but lets authorized lawyers proceed after
/// acknowledging it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Review,
    Block,
}

/// Why a finding fired — the legal shape of the concern.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    /// The path crosses an `adverse_to` edge to a party the firm serves.
    Adverse,
    /// The proposed parties share an entity / party with another matter
    /// the firm already runs for a different client.
    SharedParty,
    /// A recorded `disclosures` row (conflict / related-party) touches a
    /// node in the proposed matter's neighborhood.
    Disclosure,
}

/// One conflict the check surfaced, with enough context for a lawyer to
/// adjudicate it rather than trust it blindly.
#[derive(Clone, Debug)]
pub struct ConflictFinding {
    pub severity: Severity,
    pub reason: Reason,
    /// Human label of the party the proposed matter collides with.
    pub counterparty: String,
    /// Full sentence including the relationship path that produced it.
    pub explanation: String,
    /// Confidence the path is real, 0–100.
    pub confidence_pct: i32,
}

/// The result of a pre-matter conflict check.
#[derive(Clone, Debug, Default)]
pub struct ConflictReport {
    pub findings: Vec<ConflictFinding>,
}

impl ConflictReport {
    /// No conflicts at all — the matter may open without lawyer review.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.findings.is_empty()
    }

    /// At least one `Block`-severity finding — the automated open is
    /// hard-stopped and cannot be overridden from the create form.
    #[must_use]
    pub fn has_blocking(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Block)
    }

    /// One human-readable line per finding, for the create form and for
    /// the Relationship Log audit entry when a lawyer overrides a review.
    #[must_use]
    pub fn summary_lines(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|f| {
                let tag = match f.severity {
                    Severity::Block => "BLOCK",
                    Severity::Review => "REVIEW",
                };
                format!(
                    "[{tag}] {} ({}% confidence)",
                    f.explanation, f.confidence_pct
                )
            })
            .collect()
    }
}

/// An anchor handed to the traversal, resolved to a record id by the
/// query rather than spelled into it.
#[derive(SurrealValue)]
struct AnchorRow {
    table: &'static str,
    id: String,
}

/// One edge the traversal returned, normalized across the two edge
/// tables: `entity_role`'s `role` and its implicit confidence 100 read
/// back in the same shape as `relationship`'s own fields.
///
/// The endpoint *names* travel with the edge. They used to be loaded
/// separately, because half of them lived in the other engine; now that
/// a link dereferences to a real row, asking for `in.name` in the same
/// round trip is both cheaper and the only way a name can be wrong in
/// exactly the way the edge is.
#[derive(SurrealValue)]
struct TraversedEdge {
    edge_table: String,
    from_table: String,
    from_id: String,
    from_name: Option<String>,
    to_table: String,
    to_id: String,
    to_name: Option<String>,
    kind: String,
    confidence_pct: i32,
}

/// One step out of a node: the neighbor and the edge that reaches it.
struct Adjacent {
    node: NodeRef,
    kind: String,
    confidence_pct: i32,
}

/// What each reached node is called, for the sentence a finding prints.
type Labels = HashMap<NodeRef, String>;

/// The conflict graph's lookups, built once per check by [`build_graph`].
///
/// The graph itself is not in here — it is the store, and `graph` is a
/// handle to it. What this holds is the "who does the firm already
/// serve" side of the question, which is a matter of participations and
/// disclosures rather than of edges.
pub struct ConflictGraph {
    /// The store the traversal runs against.
    graph: SurrealDb,
    /// Entity → the distinct client DRIs of its non-archived projects.
    entity_clients: HashMap<Uuid, HashSet<Uuid>>,
    /// Persons who are the client DRI of some non-archived project.
    client_persons: HashSet<Uuid>,
    /// Entity → its conflict / related-party disclosure summaries.
    entity_disclosures: HashMap<Uuid, Vec<String>>,
}

/// Whether the firm has actually screened this person for conflicts.
///
/// The predicate is [`build_graph`]'s own definition of a person the firm
/// serves — a `person_project_role` carrying `is_client_dri` on a matter
/// that is not archived — asked about one person instead of all of them.
/// Two callers, one meaning: a party this returns `true` for is a party
/// the conflict graph already reasons over.
///
/// # Why not "does a `person` row exist"
///
/// Because a row is an identity, not a decision. Rows are minted by
/// `find_or_create` on the intake walk, by bulk import, by an OIDC first
/// sign-in and by seeding, none of which imply anyone ran a check. The
/// intake walk falsifies the shortcut outright: it writes the person
/// *before* it calls [`check_new_matter`], and when the check refuses the
/// intake the compensation removes the participation and the matter but
/// leaves the person — so mere presence is at its least trustworthy on
/// exactly the population the screen exists to catch.
///
/// The client-DRI marker does not have that problem. It is written by
/// `projects::designate_dri_in_surreal` only after the check has returned
/// without blocking, so it sits downstream of the screen by control flow
/// rather than beside it by coincidence.
///
/// Reads only, like the rest of this module.
///
/// # Errors
///
/// Returns a message if the participation or matter lookups fail.
pub async fn is_screened_client(surreal: &SurrealDb, person_id: Uuid) -> Result<bool, String> {
    for role in crate::projects::participations_for_person(surreal, person_id)
        .await
        .map_err(|error| format!("load conflict-screen participations: {error}"))?
        .into_iter()
        .filter(|role| role.is_client_dri)
    {
        let matter = crate::projects::find_by_id(surreal, role.project_id)
            .await
            .map_err(|error| format!("load conflict-screen matter: {error}"))?;
        if matter.is_some_and(|matter| matter.status != "archived") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Load the lookups a conflict check tests its reachable set against.
///
/// The edges are not loaded here: the traversal reads them itself, one
/// bounded query at check time, rather than pulling them into memory
/// first. What this loads is `projects` (who the firm currently acts for)
/// and `disclosures`, both on the same Surreal handle the traversal uses.
///
/// # Errors
///
/// Returns a [`String::Custom`] if any of the matter, participation, or
/// disclosure lookups fail.
pub async fn build_graph(surreal: &SurrealDb) -> Result<ConflictGraph, String> {
    // Who the firm currently acts for, keyed by entity. An under-inclusive
    // set here is a missed conflict, so this stays one bulk join over the
    // client-DRI markers rather than a per-project lookup.
    let mut entity_clients: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
    let mut client_persons: HashSet<Uuid> = HashSet::new();
    let live_project_entities: HashMap<Uuid, Uuid> = crate::projects::all(surreal)
        .await
        .map_err(|error| format!("load conflict-check matters: {error}"))?
        .into_iter()
        .filter(|project| project.status != "archived")
        .map(|proj| (proj.id, proj.entity_id))
        .collect();
    for row in crate::projects::all_participations(surreal)
        .await
        .map_err(|error| format!("load conflict-check participations: {error}"))?
        .into_iter()
        .filter(|role| role.is_client_dri)
    {
        let Some(entity_id) = live_project_entities.get(&row.project_id) else {
            continue;
        };
        entity_clients
            .entry(*entity_id)
            .or_default()
            .insert(row.person_id);
        client_persons.insert(row.person_id);
    }

    let entity_disclosures = crate::disclosures::conflict_summaries_by_entity(surreal)
        .await
        .map_err(|error| format!("load conflict-check disclosures: {error}"))?;

    Ok(ConflictGraph {
        graph: surreal.clone(),
        entity_clients,
        client_persons,
        entity_disclosures,
    })
}

/// How a graph-engine failure reaches a caller that only knows
/// [`String`]. The store's public seam predates the second engine and
/// keeps its signature, so this is the one place the two error
/// vocabularies meet.
fn graph_engine_error(context: &str, source: &dyn std::fmt::Display) -> String {
    format!("{context}: {source}")
}

/// Read one traversal endpoint back into a [`NodeRef`].
///
/// Returns `None` for a table this check does not model or a key that
/// is not a UUID — "skip it rather than guess". The engine refuses to
/// *write* an endpoint outside `person|entity`, so this is a read-side
/// backstop rather than a live filter.
fn node_ref(table: &str, id: &str) -> Option<NodeRef> {
    Some(NodeRef {
        kind: Endpoint::from_table(table)?,
        id: Uuid::parse_str(id).ok()?,
    })
}

/// What a node is called when its row is missing.
fn fallback_label(node: NodeRef) -> String {
    match node.kind {
        Endpoint::Person => format!("person {}", node.id),
        Endpoint::Entity => format!("entity {}", node.id),
    }
}

/// The label a node prints in a finding.
fn label(labels: &Labels, node: NodeRef) -> String {
    labels
        .get(&node)
        .cloned()
        .unwrap_or_else(|| fallback_label(node))
}

/// One node the traversal reached, with how it got there.
struct Reached {
    confidence_pct: i32,
    hops: usize,
    adverse_on_path: bool,
    path: String,
}

/// The fields the traversal projects out of an edge record, normalizing
/// `entity_role` (structural, implicit confidence 100) and
/// `relationship` (typed, its own confidence) into one shape.
const TRAVERSAL_FIELDS: &str = "\
meta::tb(id) AS edge_table,
    meta::tb(in) AS from_table, <string> meta::id(in) AS from_id, in.name AS from_name,
    meta::tb(out) AS to_table, <string> meta::id(out) AS to_id, out.name AS to_name,
    IF meta::tb(id) = 'entity_role' { role } ELSE { kind } AS kind,
    IF meta::tb(id) = 'entity_role' { 100 } ELSE { confidence_pct } AS confidence_pct";

/// The bounded traversal: every edge within [`MAX_HOPS`] of the
/// anchors, in one round trip.
///
/// It expands one hop at a time — edges incident to the frontier, then
/// the endpoints of those edges, then again — because the walk has to
/// stop at the same depth the scoring does. The bound is the query's,
/// not the caller's: an edge further out than [`MAX_HOPS`] never
/// crosses the wire, so the scoring below cannot accidentally follow
/// one. That bound is what keeps this affordable against the live store
/// rather than a per-check copy sized to the whole ledger.
///
/// Returns the query and the index of its final statement — the one
/// whose result is the edge list. Statements span several lines each,
/// so the index is counted as they are built rather than recovered
/// from the text afterwards.
fn traversal_query() -> (String, usize) {
    let incident_edges = |frontier: &str| {
        format!(
            "array::distinct(array::flatten((SELECT VALUE \
             array::flatten([<->entity_role, <->relationship]) FROM {frontier})))"
        )
    };
    let endpoints = |edges: &str| {
        format!("array::distinct(array::flatten((SELECT VALUE [in, out] FROM {edges})))")
    };

    let mut statements =
        vec!["LET $l0 = (SELECT VALUE type::record(table, <uuid> id) FROM $anchors);".to_string()];
    for hop in 1..=MAX_HOPS {
        statements.push(format!(
            "LET $e{hop} = {};",
            incident_edges(&format!("$l{}", hop - 1))
        ));
        // The last hop's endpoints are never expanded, so they are
        // never collected.
        if hop < MAX_HOPS {
            statements.push(format!("LET $l{hop} = {};", endpoints(&format!("$e{hop}"))));
        }
    }

    let collected = (1..=MAX_HOPS)
        .map(|hop| format!("$e{hop}"))
        .collect::<Vec<_>>()
        .join(", ");
    statements.push(format!(
        "RETURN (SELECT {TRAVERSAL_FIELDS} FROM array::distinct(array::flatten([{collected}])));"
    ));
    let last = statements.len() - 1;
    (statements.join("\n"), last)
}

impl ConflictGraph {
    /// Ask the engine for every edge within [`MAX_HOPS`] of `anchors`,
    /// as an undirected step list per node, plus what each reached node
    /// is called.
    ///
    /// The list is ordered — structural ties before supplemental ones,
    /// then by endpoint — so two runs over the same rows produce the
    /// same explanation text, which an engine's edge order does not
    /// guarantee on its own.
    async fn neighborhood(
        &self,
        anchors: &[NodeRef],
    ) -> Result<(HashMap<NodeRef, Vec<Adjacent>>, Labels), String> {
        let (query, last) = traversal_query();
        let anchor_rows: Vec<AnchorRow> = anchors
            .iter()
            .map(|anchor| AnchorRow {
                table: anchor.kind.table(),
                id: anchor.id.to_string(),
            })
            .collect();

        let mut edges: Vec<TraversedEdge> = self
            .graph
            .query(query)
            .bind(("anchors", anchor_rows))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .map_err(|err| graph_engine_error("traverse the conflict-check graph", &err))?
            .take(last)
            .map_err(|err| graph_engine_error("read the conflict-check traversal", &err))?;

        edges.sort_by(|a, b| {
            (
                &a.edge_table,
                &a.from_table,
                &a.from_id,
                &a.to_table,
                &a.to_id,
                &a.kind,
            )
                .cmp(&(
                    &b.edge_table,
                    &b.from_table,
                    &b.from_id,
                    &b.to_table,
                    &b.to_id,
                    &b.kind,
                ))
        });

        let mut adjacency: HashMap<NodeRef, Vec<Adjacent>> = HashMap::new();
        let mut labels: Labels = HashMap::new();
        for edge in edges {
            let (Some(from), Some(to)) = (
                node_ref(&edge.from_table, &edge.from_id),
                node_ref(&edge.to_table, &edge.to_id),
            ) else {
                continue;
            };
            // A node whose row is gone still renders: the label falls
            // back to the id rather than the finding failing to print.
            if let Some(name) = edge.from_name {
                labels.insert(from, name);
            }
            if let Some(name) = edge.to_name {
                labels.insert(to, name);
            }
            // The graph is undirected: adversity runs both ways, and a
            // conflict does not care which end of the edge was typed
            // in first.
            adjacency.entry(from).or_default().push(Adjacent {
                node: to,
                kind: edge.kind.clone(),
                confidence_pct: edge.confidence_pct,
            });
            adjacency.entry(to).or_default().push(Adjacent {
                node: from,
                kind: edge.kind,
                confidence_pct: edge.confidence_pct,
            });
        }
        Ok((adjacency, labels))
    }

    /// Name the anchors themselves.
    ///
    /// The traversal names every node it *returns*, but an anchor with
    /// no edges at all appears in no edge — and it is still the subject
    /// of the report, so it has to be resolvable. Only the anchors
    /// missing from the traversal are read back.
    async fn label_anchors(&self, anchors: &[NodeRef], labels: &mut Labels) -> Result<(), String> {
        let missing: Vec<NodeRef> = anchors
            .iter()
            .copied()
            .filter(|anchor| !labels.contains_key(anchor))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }

        let person_ids: Vec<Uuid> = missing
            .iter()
            .filter(|n| n.kind == Endpoint::Person)
            .map(|n| n.id)
            .collect();
        let entity_ids: Vec<Uuid> = missing
            .iter()
            .filter(|n| n.kind == Endpoint::Entity)
            .map(|n| n.id)
            .collect();

        for person in crate::persons::find_by_ids(&self.graph, &person_ids)
            .await
            .map_err(|err| graph_engine_error("resolve a conflict-check anchor person", &err))?
        {
            labels.insert(NodeRef::person(person.id), person.name);
        }
        for entity in crate::entities::find_by_ids(&self.graph, &entity_ids)
            .await
            .map_err(|err| graph_engine_error("resolve a conflict-check anchor entity", &err))?
        {
            labels.insert(NodeRef::entity(entity.id), entity.name);
        }
        Ok(())
    }

    /// Breadth-first walk from both anchors, keeping the shortest path to
    /// each reachable node along with the multiplied confidence and
    /// whether an `adverse_to` edge lay on the way.
    async fn reach(
        &self,
        anchors: &[NodeRef],
    ) -> Result<(HashMap<NodeRef, Reached>, Labels), String> {
        let (adjacency, mut labels) = self.neighborhood(anchors).await?;
        self.label_anchors(anchors, &mut labels).await?;

        let mut reached: HashMap<NodeRef, Reached> = HashMap::new();
        let mut queue: VecDeque<(NodeRef, i32, usize, bool, String)> = VecDeque::new();

        for &a in anchors {
            reached.insert(
                a,
                Reached {
                    confidence_pct: 100,
                    hops: 0,
                    adverse_on_path: false,
                    path: label(&labels, a),
                },
            );
            queue.push_back((a, 100, 0, false, label(&labels, a)));
        }

        while let Some((cur, conf, hops, adverse, path)) = queue.pop_front() {
            if hops >= MAX_HOPS {
                continue;
            }
            let Some(neighbors) = adjacency.get(&cur) else {
                continue;
            };
            for step in neighbors {
                let next = step.node;
                let next_conf = conf * step.confidence_pct / 100;
                if next_conf < REVIEW_FLOOR_PCT {
                    continue;
                }
                let next_adverse = adverse || step.kind == KIND_ADVERSE_TO;
                let next_path = format!("{path} —{}→ {}", step.kind, label(&labels, next));
                let next_hops = hops + 1;
                let should_update = reached.get(&next).is_none_or(|existing| {
                    next_hops < existing.hops
                        || (next_adverse && !existing.adverse_on_path)
                        || (next_conf > existing.confidence_pct
                            && (next_adverse || !existing.adverse_on_path))
                });
                if should_update {
                    reached.insert(
                        next,
                        Reached {
                            confidence_pct: next_conf,
                            hops: next_hops,
                            adverse_on_path: next_adverse,
                            path: next_path.clone(),
                        },
                    );
                    queue.push_back((next, next_conf, next_hops, next_adverse, next_path));
                }
            }
        }
        Ok((reached, labels))
    }

    /// Run the conflict check for opening a matter for `client_person_id`
    /// against `entity_id`. The anchors are those two nodes; the report
    /// names every distinct firm-served party the proposed matter is
    /// entangled with.
    ///
    /// # Errors
    ///
    /// Returns a [`String::Custom`] if the graph engine cannot answer the
    /// traversal.
    pub async fn check(
        &self,
        client_person_id: Uuid,
        entity_id: Uuid,
    ) -> Result<ConflictReport, String> {
        let anchor_person = NodeRef::person(client_person_id);
        let anchor_entity = NodeRef::entity(entity_id);
        let (reached, labels) = self.reach(&[anchor_person, anchor_entity]).await?;

        let mut findings = Vec::new();
        for (&n, r) in &reached {
            // Adversity / shared-party concerns attach to entity nodes the
            // firm already serves and to *other* client persons.
            let counterparty_client = match n.kind {
                Endpoint::Entity => self
                    .entity_clients
                    .get(&n.id)
                    .is_some_and(|clients| clients.iter().any(|c| *c != client_person_id)),
                Endpoint::Person => n.id != client_person_id && self.client_persons.contains(&n.id),
            };

            if counterparty_client {
                let (severity, reason) = if r.adverse_on_path {
                    let blocking = r.confidence_pct >= BLOCK_FLOOR_PCT && r.hops <= BLOCK_MAX_HOPS;
                    (
                        if blocking {
                            Severity::Block
                        } else {
                            Severity::Review
                        },
                        Reason::Adverse,
                    )
                } else {
                    (Severity::Review, Reason::SharedParty)
                };
                let lead = match reason {
                    Reason::Adverse => "Adverse to a current client",
                    _ => "Shares a party with a current client's matter",
                };
                findings.push(ConflictFinding {
                    severity,
                    reason,
                    counterparty: label(&labels, n),
                    explanation: format!("{lead}: {}", r.path),
                    confidence_pct: r.confidence_pct,
                });
            }

            // Recorded disclosures on any reached entity always surface.
            if n.kind == Endpoint::Entity {
                if let Some(summaries) = self.entity_disclosures.get(&n.id) {
                    for summary in summaries {
                        findings.push(ConflictFinding {
                            severity: Severity::Review,
                            reason: Reason::Disclosure,
                            counterparty: label(&labels, n),
                            explanation: format!(
                                "Disclosure on {}: {summary} (via {})",
                                label(&labels, n),
                                r.path
                            ),
                            confidence_pct: r.confidence_pct,
                        });
                    }
                }
            }
        }

        // Stable order: blocks first, then by descending confidence, so
        // the most serious finding leads the form and the audit log.
        findings.sort_by(|a, b| {
            b.severity
                .eq(&Severity::Block)
                .cmp(&a.severity.eq(&Severity::Block))
                .then(b.confidence_pct.cmp(&a.confidence_pct))
                .then(a.counterparty.cmp(&b.counterparty))
        });
        Ok(ConflictReport { findings })
    }
}

/// Build the lookups and run the pre-matter conflict check in one call.
/// This is the entry point the project-create paths use.
///
/// # Errors
///
/// Returns any `String` from loading the lookups or running the
/// traversal.
pub async fn check_new_matter(
    surreal: &SurrealDb,
    client_person_id: Uuid,
    entity_id: Uuid,
) -> Result<ConflictReport, String> {
    build_graph(surreal)
        .await?
        .check(client_person_id, entity_id)
        .await
}

#[cfg(test)]
mod tests {
    use super::{check_new_matter, is_screened_client, Reason, Severity};
    use crate::entity_roles;
    use crate::persons::{self, NewPerson};
    use crate::relationships::{
        self, Endpoint, NewRelationship, KIND_ADVERSE_TO, KIND_RELATED_PARTY, SOURCE_LLM,
        SOURCE_MANUAL,
    };
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use crate::test_support::{dri_person, seed_entity};
    use uuid::Uuid;

    async fn person_named(surreal: &SurrealDb, name: &str) -> Uuid {
        persons::create(
            surreal,
            &NewPerson::new(name, format!("{}@example.com", Uuid::now_v7())),
        )
        .await
        .unwrap()
        .id
    }

    /// One supplemental edge, at full confidence unless a test says
    /// otherwise. Both endpoints are typed by the [`Endpoint`] enum, so a
    /// `from_type: "persn"` typo cannot be written from Rust at all.
    async fn edge(
        surreal: &SurrealDb,
        from: (Endpoint, Uuid),
        to: (Endpoint, Uuid),
        kind: &str,
        confidence_pct: i32,
    ) {
        relationships::record(
            surreal,
            &NewRelationship {
                from: from.0,
                from_id: from.1,
                to: to.0,
                to_id: to.1,
                kind: kind.into(),
                confidence_pct,
                source_kind: if confidence_pct >= 80 {
                    SOURCE_MANUAL.into()
                } else {
                    SOURCE_LLM.into()
                },
                source_id: None,
                detail: None,
            },
        )
        .await
        .unwrap();
    }

    async fn open_project(surreal: &SurrealDb, entity_id: Uuid, client_id: Uuid) {
        let project = crate::projects::create(
            surreal,
            &crate::projects::NewProject {
                code: format!("existing-matter-{}", Uuid::now_v7()),
                name: "Existing matter".into(),
                status: "open".into(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // The conflict graph reasons over who the firm currently acts for,
        // which it reads from the client-DRI markers.
        crate::projects::designate_dri_in_surreal(
            surreal,
            project.id,
            client_id,
            crate::projects::DriSide::Client,
        )
        .await
        .unwrap();
        crate::projects::designate_dri_in_surreal(
            surreal,
            project.id,
            dri_person(surreal).await,
            crate::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn clean_matter_has_no_findings() {
        let surreal = mem().await;
        let entity_id = seed_entity(&surreal).await;
        let client = person_named(&surreal, "Fresh Client").await;
        let report = check_new_matter(&surreal, client, entity_id).await.unwrap();
        assert!(report.is_clear(), "findings: {:?}", report.summary_lines());
    }

    #[tokio::test]
    async fn repeat_client_on_their_own_entity_is_not_a_conflict() {
        let surreal = mem().await;
        let entity_id = seed_entity(&surreal).await;
        let client = person_named(&surreal, "Returning Client").await;
        // The same client already has an open matter on the same entity —
        // opening another for them is not a conflict with themselves.
        open_project(&surreal, entity_id, client).await;
        let report = check_new_matter(&surreal, client, entity_id).await.unwrap();
        assert!(report.is_clear(), "findings: {:?}", report.summary_lines());
    }

    #[tokio::test]
    async fn shared_entity_with_a_different_client_is_a_review() {
        let surreal = mem().await;
        let entity_id = seed_entity(&surreal).await;
        let existing = person_named(&surreal, "Existing Client").await;
        let proposed = person_named(&surreal, "Proposed Client").await;
        // The firm already runs a matter on this entity for someone else.
        open_project(&surreal, entity_id, existing).await;
        let report = check_new_matter(&surreal, proposed, entity_id)
            .await
            .unwrap();
        assert!(!report.is_clear());
        assert!(!report.has_blocking());
        assert!(report
            .findings
            .iter()
            .any(|f| f.reason == Reason::SharedParty && f.severity == Severity::Review));
    }

    /// The adverse-path fixture the wave-four cut had to preserve
    /// verdict-for-verdict: a direct, confident adversity to a party the
    /// firm already serves is the one finding that hard-stops an open.
    #[tokio::test]
    async fn direct_adverse_edge_to_a_current_client_blocks() {
        let surreal = mem().await;
        let proposed = person_named(&surreal, "New Client").await;
        let opponent = person_named(&surreal, "Opposing Party").await;
        // The opponent is already a client of the firm…
        let opp_entity = seed_entity(&surreal).await;
        open_project(&surreal, opp_entity, opponent).await;
        // …and the proposed client is directly adverse to them.
        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Person, opponent),
            KIND_ADVERSE_TO,
            100,
        )
        .await;

        let new_entity = seed_entity(&surreal).await;
        let report = check_new_matter(&surreal, proposed, new_entity)
            .await
            .unwrap();
        assert!(
            report.has_blocking(),
            "findings: {:?}",
            report.summary_lines()
        );
        assert!(report
            .findings
            .iter()
            .any(|f| f.reason == Reason::Adverse && f.severity == Severity::Block));
    }

    /// The finding has to name the parties, not their ids — which is
    /// only true if the traversal dereferenced the resident `person`
    /// and `entity` rows. Under the retired projection the names were
    /// loaded separately and copied in; nothing would have caught a
    /// traversal that returned edges whose endpoints resolved to
    /// nothing.
    #[tokio::test]
    async fn a_finding_names_the_parties_it_reached_through_the_store() {
        let surreal = mem().await;
        let proposed = person_named(&surreal, "Named Client").await;
        let opponent = person_named(&surreal, "Named Opponent").await;
        let opp_entity = seed_entity(&surreal).await;
        open_project(&surreal, opp_entity, opponent).await;
        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Person, opponent),
            KIND_ADVERSE_TO,
            100,
        )
        .await;

        let report = check_new_matter(&surreal, proposed, seed_entity(&surreal).await)
            .await
            .unwrap();
        let explanation = report
            .findings
            .iter()
            .find(|f| f.reason == Reason::Adverse)
            .map(|f| f.explanation.clone())
            .expect("an adverse finding");

        assert!(
            explanation.contains("Named Client") && explanation.contains("Named Opponent"),
            "the path should read in plain language, got: {explanation}"
        );
        assert!(
            !explanation.contains(&proposed.to_string()),
            "a bare id means the endpoint link did not resolve: {explanation}"
        );
    }

    #[tokio::test]
    async fn low_confidence_adverse_edge_only_warns() {
        let surreal = mem().await;
        let proposed = person_named(&surreal, "Maybe Client").await;
        let opponent = person_named(&surreal, "Maybe Opponent").await;
        let opp_entity = seed_entity(&surreal).await;
        open_project(&surreal, opp_entity, opponent).await;
        // A shaky LLM-parsed adverse edge: below the block floor.
        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Person, opponent),
            KIND_ADVERSE_TO,
            40,
        )
        .await;

        let new_entity = seed_entity(&surreal).await;
        let report = check_new_matter(&surreal, proposed, new_entity)
            .await
            .unwrap();
        assert!(!report.is_clear());
        assert!(
            !report.has_blocking(),
            "a 40% edge should not hard-block: {:?}",
            report.summary_lines()
        );
    }

    #[tokio::test]
    async fn adversity_through_a_managed_entity_is_caught() {
        let surreal = mem().await;
        // Proposed client manages an entity that is adverse to an entity
        // the firm already serves for another client — a two-hop chain.
        let proposed = person_named(&surreal, "Chain Client").await;
        let proposed_entity = seed_entity(&surreal).await;
        entity_roles::grant(&surreal, proposed, proposed_entity, "manages")
            .await
            .unwrap();

        let opp_entity = seed_entity(&surreal).await;
        let existing_client = person_named(&surreal, "Served Client").await;
        open_project(&surreal, opp_entity, existing_client).await;
        edge(
            &surreal,
            (Endpoint::Entity, proposed_entity),
            (Endpoint::Entity, opp_entity),
            KIND_ADVERSE_TO,
            100,
        )
        .await;

        let report = check_new_matter(&surreal, proposed, proposed_entity)
            .await
            .unwrap();
        assert!(
            report.findings.iter().any(|f| f.reason == Reason::Adverse),
            "expected an adverse finding via the managed entity: {:?}",
            report.summary_lines()
        );
    }

    #[tokio::test]
    async fn adverse_edge_between_anchors_blocks_even_with_structural_edge() {
        let surreal = mem().await;
        let proposed = person_named(&surreal, "Anchor Client").await;
        let proposed_entity = seed_entity(&surreal).await;
        let existing_client = person_named(&surreal, "Existing Entity Client").await;
        open_project(&surreal, proposed_entity, existing_client).await;

        // The structural edge sorts before the supplemental one. The
        // adverse edge still has to win; otherwise direct adversity to the
        // proposed entity degrades into an overridable shared-party review.
        entity_roles::grant(&surreal, proposed, proposed_entity, "manages")
            .await
            .unwrap();
        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Entity, proposed_entity),
            KIND_ADVERSE_TO,
            100,
        )
        .await;

        let report = check_new_matter(&surreal, proposed, proposed_entity)
            .await
            .unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.reason == Reason::Adverse && f.severity == Severity::Block),
            "expected direct anchor adversity to block: {:?}",
            report.summary_lines()
        );
    }

    #[tokio::test]
    async fn higher_confidence_structural_path_does_not_clear_prior_adversity() {
        let surreal = mem().await;
        let proposed = person_named(&surreal, "Mixed Path Client").await;
        let bridge = person_named(&surreal, "Mixed Path Bridge").await;
        let proposed_entity = seed_entity(&surreal).await;
        let opposing_entity = seed_entity(&surreal).await;
        let existing_client = person_named(&surreal, "Mixed Path Existing Client").await;
        open_project(&surreal, opposing_entity, existing_client).await;

        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Person, bridge),
            KIND_ADVERSE_TO,
            40,
        )
        .await;
        entity_roles::grant(&surreal, proposed, proposed_entity, "manages")
            .await
            .unwrap();
        entity_roles::grant(&surreal, bridge, proposed_entity, "manages")
            .await
            .unwrap();
        entity_roles::grant(&surreal, bridge, opposing_entity, "manages")
            .await
            .unwrap();

        let report = check_new_matter(&surreal, proposed, proposed_entity)
            .await
            .unwrap();
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.reason == Reason::Adverse && f.severity == Severity::Review),
            "expected adversity to survive a later high-confidence structural update: {:?}",
            report.summary_lines()
        );
    }

    /// #1145, in the shape that would break silently: the check has to
    /// read a matter the proposed client has no part in. That is the
    /// whole point — imputed conflicts live on *other people's*
    /// matters — so this fails the day anything scopes the traversal to
    /// the requester's participations.
    #[tokio::test]
    async fn a_matter_the_proposed_client_does_not_participate_in_is_still_seen() {
        let surreal = mem().await;
        let opponent = person_named(&surreal, "Stranger's Client").await;
        let opp_entity = seed_entity(&surreal).await;
        open_project(&surreal, opp_entity, opponent).await;

        let proposed = person_named(&surreal, "Outside Client").await;
        assert!(
            crate::projects::participations_for_person(&surreal, proposed)
                .await
                .unwrap()
                .is_empty(),
            "the proposed client must be a stranger to every matter for this to prove anything"
        );

        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Person, opponent),
            KIND_ADVERSE_TO,
            100,
        )
        .await;

        let report = check_new_matter(&surreal, proposed, seed_entity(&surreal).await)
            .await
            .unwrap();
        assert!(
            report.has_blocking(),
            "a conflict on a matter the requester is a stranger to must still be raised: {:?}",
            report.summary_lines()
        );
    }

    /// Builds a chain of exactly `hops` related-party edges out of one
    /// person and returns the entity at its far end. Every edge is full
    /// confidence, so nothing here is dropped by the confidence floor
    /// and only the hop bound can end the walk. Entity-to-entity edges
    /// keep one edge per hop, where alternating structural ties would
    /// spend two.
    async fn chain_from(surreal: &SurrealDb, start: Uuid, hops: usize) -> Uuid {
        let mut from = (Endpoint::Person, start);
        let mut far = start;
        for _ in 0..hops {
            far = seed_entity(surreal).await;
            edge(
                surreal,
                from,
                (Endpoint::Entity, far),
                KIND_RELATED_PARTY,
                100,
            )
            .await;
            from = (Endpoint::Entity, far);
        }
        far
    }

    /// `MAX_HOPS` is the *query's* bound, not a loop counter the
    /// scoring happens to respect — and it now bounds a walk over the
    /// live store rather than a per-check copy, which is what keeps the
    /// check affordable. Worth pinning at the boundary rather than one
    /// side of it: three hops out is a finding; four is silence.
    #[tokio::test]
    async fn the_walk_reaches_three_hops_and_stops() {
        for (hops, expect_finding) in [(super::MAX_HOPS, true), (super::MAX_HOPS + 1, false)] {
            let surreal = mem().await;
            let proposed = person_named(&surreal, "Chain Anchor").await;
            let far_entity = chain_from(&surreal, proposed, hops).await;
            open_project(
                &surreal,
                far_entity,
                person_named(&surreal, "Distant Client").await,
            )
            .await;

            // The anchor entity is unrelated, so the chain out of the
            // proposed person is the only way to reach anything.
            let report = check_new_matter(&surreal, proposed, seed_entity(&surreal).await)
                .await
                .unwrap();
            assert_eq!(
                !report.is_clear(),
                expect_finding,
                "{hops} hops: {:?}",
                report.summary_lines()
            );
        }
    }

    /// The three states a `person` row can be in, and the one that counts.
    /// A bare row is what a refused intake leaves behind; a participation
    /// with no client-DRI marker is what an errored conflict traversal
    /// leaves. Neither is a screen — only the marker written after the
    /// check is.
    #[tokio::test]
    async fn only_the_client_dri_marker_reads_as_a_screen() {
        let surreal = mem().await;

        // A row and nothing else — a refused intake's residue.
        let refused = person_named(&surreal, "Refused Intake").await;
        assert!(
            !is_screened_client(&surreal, refused).await.unwrap(),
            "a bare person row is an identity, not a screen"
        );

        // A participation on an open matter, but no marker — the state an
        // errored traversal leaves, since designation happens after it.
        let unscreened = person_named(&surreal, "Undesignated Participant").await;
        let matter = crate::projects::create(
            &surreal,
            &crate::projects::NewProject {
                code: format!("pending-matter-{}", Uuid::now_v7()),
                name: "Pending matter".into(),
                status: "open".into(),
                entity_id: seed_entity(&surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        crate::projects::add_participation(&surreal, matter.id, unscreened, "client")
            .await
            .unwrap();
        assert!(
            !is_screened_client(&surreal, unscreened).await.unwrap(),
            "a participation without the client-DRI marker is not a screen"
        );

        // The marker, written only after a non-blocking check.
        let screened = person_named(&surreal, "Client Of Record").await;
        open_project(&surreal, seed_entity(&surreal).await, screened).await;
        assert!(
            is_screened_client(&surreal, screened).await.unwrap(),
            "a client of record on a live matter is screened"
        );
    }

    /// Archiving the matter withdraws the screen, exactly as it withdraws
    /// the person from [`build_graph`]'s client set.
    #[tokio::test]
    async fn an_archived_matter_no_longer_reads_as_a_screen() {
        let surreal = mem().await;
        let person = person_named(&surreal, "Former Client").await;
        open_project(&surreal, seed_entity(&surreal).await, person).await;
        assert!(is_screened_client(&surreal, person).await.unwrap());

        for matter in crate::projects::all(&surreal).await.unwrap() {
            crate::projects::transition_project(
                &surreal,
                matter.id,
                crate::projects::Transition::Archive,
            )
            .await
            .unwrap();
        }
        assert!(
            !is_screened_client(&surreal, person).await.unwrap(),
            "an archived matter leaves the conflict graph, and the screen with it"
        );
    }

    /// The traversal runs against the deployment's own store now, so
    /// "it only reads" stopped being free. A stray `RELATE`, `CREATE`,
    /// or `DELETE` in the traversal would mutate the ledger a conflict
    /// check is supposed to inspect — and the check runs on the matter
    /// -open path, where a write would be both silent and legally
    /// consequential.
    #[tokio::test]
    async fn the_check_writes_nothing() {
        let surreal = mem().await;
        let proposed = person_named(&surreal, "Read Only Client").await;
        let opponent = person_named(&surreal, "Read Only Opponent").await;
        let opp_entity = seed_entity(&surreal).await;
        open_project(&surreal, opp_entity, opponent).await;
        entity_roles::grant(&surreal, proposed, opp_entity, "manages")
            .await
            .unwrap();
        edge(
            &surreal,
            (Endpoint::Person, proposed),
            (Endpoint::Person, opponent),
            KIND_ADVERSE_TO,
            100,
        )
        .await;

        let census = || async {
            (
                crate::entities::all(&surreal).await.unwrap().len(),
                crate::entity_roles::all(&surreal).await.unwrap().len(),
                crate::relationships::all(&surreal).await.unwrap().len(),
                crate::projects::all(&surreal).await.unwrap().len(),
            )
        };
        let before = census().await;

        let report = check_new_matter(&surreal, proposed, opp_entity)
            .await
            .unwrap();
        assert!(
            !report.is_clear(),
            "the fixture has to actually traverse something for this to prove anything"
        );

        assert_eq!(
            census().await,
            before,
            "the conflict check mutated the store it was only supposed to read"
        );
    }
}
