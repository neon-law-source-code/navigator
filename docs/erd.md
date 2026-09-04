# Entity-relationship diagram

The workspace ships two ERD artifacts, both rendered from the same live `INFO FOR DB` introspection in `cli/src/erd.rs`:

- `docs/erd.md` (this file) — the Mermaid `erDiagram` block under [Schema](#schema). GitHub renders Mermaid natively,
  so this is the "view in the repo" artifact.
- `docs/erd.svg` — a standalone SVG from `navigator db erd --format svg`. The renderer is **deterministic by
  construction** (alphabetical `BTreeMap` iteration, integer-only arithmetic, no timestamps, no random IDs): same schema
  in → byte-identical SVG out. Use it anywhere Mermaid won't render (slides, design docs, screenshots, links shared
  outside the repo). Unlike Mermaid's own SVG (text in `<foreignObject>`, invisible in many viewers), this renderer uses
  native `<text>` elements, so it opens in GNOME Image Viewer, `feh`, browsers — anything.

## Regenerating

Both artifacts come from one command; `--format` selects the renderer. The command reads the `NAVIGATOR_SURREAL_*`
connection and **applies the schema idempotently before introspecting**, so a database no one has prepared yields the
current shape rather than an empty diagram:

```bash
set -a && source .devx/env && set +a   # NAVIGATOR_SURREAL_* for the KIND store

# Mermaid erDiagram block → paste into the fenced block under Schema in docs/erd.md
cargo run -p cli -- db erd --format mermaid

# Deterministic SVG → overwrite the committed file
cargo run -p cli -- db erd --format svg > docs/erd.svg
```

- `--format mermaid` (default) — the GitHub-renderable `erDiagram` block on stdout.
- `--format svg` — a standalone SVG document on stdout.

After any change to `store/src/schema/navigator.surql`, refresh **both** so the picture and the schema stay in sync.

## The `docs/erd.svg` idempotency test

`cli/tests/erd_svg.rs` prepares a server-mode store (`store::test_support::server_surreal`), renders the SVG through the
`navigator` binary, and asserts the output byte-matches the committed `docs/erd.svg`. That binary is a subprocess and
cannot reach an in-process engine, so the test skips when no endpoint is configured and `NAV_REQUIRE_SURREAL=1` turns
that skip into a failure. The failure message prints the byte sizes, the first divergent line, and the exact refresh
command. The intended flow: edit `navigator.surql` → `cargo test -p cli` → see the drift → regenerate → commit the
schema change and the refreshed SVG together on a topic branch (never on `main`).

## Verifying against production

Every boot applies the schema, so local and prod should match as soon as a deploy lands. To diff (this *applies* the
schema to the target, so it is safe only against a deployment already on the same build):

```bash
NAVIGATOR_SURREAL_ENDPOINT=wss://your-deployment.example \
NAVIGATOR_SURREAL_NAMESPACE=navigator \
NAVIGATOR_SURREAL_DATABASE=navigator \
  cargo run -p cli -- db erd --format mermaid > /tmp/prod_mermaid.txt
diff <(cargo run -p cli -- db erd --format mermaid) /tmp/prod_mermaid.txt   # no output = identical
```

See [`test-database.md`](test-database.md) for the local store connection story.

## Opening the SVG

```bash
firefox docs/erd.svg                              # or google-chrome, or any image viewer
xdg-mime default firefox.desktop image/svg+xml    # optional: make Firefox the default SVG handler
xdg-open docs/erd.svg
```

## Layout tuning

The layout is intentionally simple — a 4-column alphabetical row-major grid, straight-line edges, no crossing avoidance;
the priority is *byte-stable, readable enough*. The constants live at the top of `cli/src/erd.rs`: `CHAR_WIDTH` /
`ROW_HEIGHT` / `TITLE_HEIGHT` (text dimensions), `CELL_PAD` / `CELL_GAP_X` / `CELL_GAP_Y` (spacing), `GRID_COLS`
(columns wide), and `MARGIN` / `FONT_SIZE` (outer margin, base font). Any layout change trips the idempotency test until
`docs/erd.svg` is refreshed — a feature, not a bug.

## Other lenses on the schema

When you don't need a picture: `INFO FOR DB` (the table list), `INFO FOR TABLE <table>` (fields, types, indexes), or
`cargo run -p cli -- db erd --format mermaid` (plain text you can grep).

## Schema

```mermaid
erDiagram
    address {
        record id PK
        string city
        string country
        option_record_entity entity_id FK
        datetime inserted_at
        string line1
        option_string line2
        option_record_person person_id FK
        string postal_code
        string region
        datetime updated_at
    }
    answer {
        record id PK
        option_record_person authored_by_person_id FK
        datetime inserted_at
        option_record_notation notation_id FK
        record_person person_id FK
        record_question question_id FK
        string source
        option_string state_name
        datetime updated_at
        any value
    }
    asset {
        record id PK
        int byte_size
        string content_type
        option_string description
        option_string filename
        datetime inserted_at
        option_string kind
        any metadata
        option_record_project project_id FK
        option_string published_at
        option_string received_at
        option_string secondary_storage_key
        string sha256_hex
        option_string slug
        option_string source
        string storage_key
        datetime updated_at
        string visibility
    }
    attestation {
        record id PK
        string chain
        option_string client_wallet
        option_string firm_wallet
        datetime inserted_at
        record_notation notation_id FK
        option_string pda
        option_string recorded_at
        string sha256
        string status
        option_string tx_signature
        datetime updated_at
    }
    authority {
        record id PK
        option_record_asset archived_asset_id FK
        option_string canonical_url
        option_string checked_on
        string citation
        string class
        datetime inserted_at
        option_string issued_on
        option_string publisher
        option_string short_cite
        string title
        datetime updated_at
    }
    authority_use {
        record id PK
        record_authority authority_id FK
        string disposition
        datetime inserted_at
        string position
        record_project project_id FK
        option_string role
        datetime updated_at
    }
    case {
        record id PK
        string caption
        option_string docket_number
        option_string forum
        datetime inserted_at
        option_string judge
        option_string jurisdiction
        string posture
        record_project project_id FK
        string status
        datetime updated_at
    }
    case_docket_entry {
        record id PK
        record_case case_id FK
        option_record_asset document_asset_id FK
        string entry_number
        option_string filed_or_served_on
        datetime inserted_at
        string kind
        option_record_notation notation_id FK
        option_string party
        string title
        datetime updated_at
    }
    citation {
        record id PK
        record_authority_use authority_use_id FK
        any draft_pin
        datetime inserted_at
        string quote
        any source_pin
        datetime updated_at
        string why
    }
    communication {
        record id PK
        option_record_asset asset_id FK
        option_record_person author_person_id FK
        string body
        string channel
        option_string counterparty
        string direction
        datetime inserted_at
        string occurred_at
        record_project project_id FK
        option_string source_ref
        option_string subject
        datetime updated_at
    }
    contract_review {
        record id PK
        option_record_asset asset_id FK
        any findings
        datetime inserted_at
        record_notation notation_id FK
        record_playbook playbook_id FK
        option_string risk_summary
        string status
        datetime updated_at
    }
    credential {
        record id PK
        datetime inserted_at
        record_jurisdiction jurisdiction_id FK
        string license_number
        record_person person_id FK
        datetime updated_at
    }
    disclosure {
        record id PK
        option_record_entity entity_id FK
        datetime inserted_at
        string kind
        option_record_project project_id FK
        string summary
        datetime updated_at
    }
    discovery_item {
        record id PK
        record_discovery_request discovery_request_id FK
        datetime inserted_at
        int item_number
        option_string objections
        string request_text
        option_string response_text
        datetime updated_at
    }
    discovery_request {
        record id PK
        record_case case_id FK
        string device
        option_record_case_docket_entry docket_entry_id FK
        datetime inserted_at
        string propounding_party
        string responding_party
        option_string responses_due_on
        option_string served_on
        int set_number
        string status
        datetime updated_at
    }
    document_comment {
        record id PK
        int anchor_end
        int anchor_start
        string body
        option_uuid communication_id
        datetime inserted_at
        record_person person_id FK
        string quoted_text
        bool resolved
        record_review_document review_document_id FK
        datetime updated_at
    }
    email_conversation {
        record id PK
        string external_email
        option_string external_name
        datetime inserted_at
        option_record_notation notation_id FK
        option_record_person person_id FK
        string status
        string subject
        string token
        datetime updated_at
    }
    email_conversation_message {
        record id PK
        string body_text
        option_string command_payload
        record_email_conversation conversation_id FK
        string direction
        string from_addr
        option_string in_reply_to
        datetime inserted_at
        option_string provider_message_id
        option_string raw_storage_key
        string subject
        string to_addr
        datetime updated_at
    }
    email_token {
        record id PK
        string email
        datetime expires_at
        datetime inserted_at
        record_person person_id FK
        string purpose
        string token_hash
        datetime updated_at
        option_datetime used_at
    }
    entity {
        record id PK
        record_entity_type entity_type_id FK
        option_string firm_anchor_key
        datetime inserted_at
        record_jurisdiction jurisdiction_id FK
        string name
        option_string phone
        datetime updated_at
        option_string url
    }
    entity_role {
        record id PK
        record_person in FK
        datetime inserted_at
        record_entity out FK
        string role
        datetime updated_at
    }
    entity_type {
        record id PK
        datetime inserted_at
        string name
        datetime updated_at
    }
    expunge_record {
        record id PK
        record_person authorized_by_person_id FK
        string category
        option_string head_after
        option_string head_before
        datetime inserted_at
        option_string note
        string path
        record_project project_id FK
        datetime updated_at
    }
    expunge_request {
        record id PK
        record_asset asset_id FK
        option_record_expunge_record expunge_record_id FK
        datetime inserted_at
        option_string note
        record_project project_id FK
        record_person requested_by_person_id FK
        option_record_person resolved_by_person_id FK
        string status
        datetime updated_at
    }
    filing {
        record id PK
        datetime inserted_at
        string kind
        record_notation notation_id FK
        string office
        option_string reference
        string submitted_at
        string summary
        datetime updated_at
    }
    firm_anchor {
        record id PK
        record_entity entity_id FK
    }
    git_access_token {
        record id PK
        string expires_at
        string inserted_at
        record_person person_id FK
        option_record_project project_id FK
        string scope
        string token_hash
        string updated_at
    }
    git_repository {
        record id PK
        datetime inserted_at
        string last_commit_sha
        string remote_hash
        datetime updated_at
    }
    glossary_term {
        record id PK
        string body
        datetime inserted_at
        string slug
        string title
        datetime updated_at
    }
    jurisdiction {
        record id PK
        string code
        datetime inserted_at
        string jurisdiction_type
        string name
        datetime updated_at
    }
    letter {
        record id PK
        string direction
        datetime inserted_at
        record_mailroom mailroom_id FK
        string recipient
        string sender
        string summary
        datetime updated_at
    }
    mailroom {
        record id PK
        record_address address_id FK
        datetime inserted_at
        string name
        datetime updated_at
    }
    notarization {
        record id PK
        option_record_asset asset_id FK
        datetime inserted_at
        option_string notarized_at
        option_record_person notary_person_id FK
        record_notation notation_id FK
        string provider
        string provider_id
        datetime updated_at
    }
    notation {
        record id PK
        string delivery
        option_record_entity entity_id FK
        option_string git_commit_sha
        datetime inserted_at
        record_person person_id FK
        record_project project_id FK
        any questionnaire_snapshot
        string state
        record_template template_id FK
        datetime updated_at
    }
    notation_clause {
        record id PK
        option_record_person authored_by_person_id FK
        string body_markdown
        datetime inserted_at
        record_notation notation_id FK
        int position
        datetime updated_at
    }
    notation_event {
        record id PK
        record_person acting_person_id FK
        string condition
        string from_state
        datetime inserted_at
        string machine_kind
        record_notation notation_id FK
        option_string payload
        string recorded_at
        record_template template_version_id FK
        string to_state
        datetime updated_at
    }
    person {
        record id PK
        string email
        bool email_confirmed
        string email_lower
        option_string family_name
        option_string given_name
        datetime inserted_at
        bool is_admitted
        option_string linkedin_url
        option_string middle_name
        string name
        option_string oidc_subject
        option_string phone
        option_string profile_image_url
        string role
        option_string title
        datetime updated_at
        option_string xero_contact_id
    }
    person_external_identity {
        record id PK
        string external_id
        option_string handle
        datetime inserted_at
        record_person person_id FK
        string system
        datetime updated_at
    }
    person_mailbox {
        record id PK
        record_person person_id FK
    }
    person_project_role {
        record id PK
        string inserted_at
        bool is_client_dri
        bool is_lawyer_dri
        string participation
        record_person person_id FK
        record_project project_id FK
        string updated_at
    }
    playbook {
        record id PK
        bool active
        record_entity entity_id FK
        datetime inserted_at
        string name
        any positions
        datetime updated_at
    }
    project {
        record id PK
        option_string brand
        option_string closed_at
        string code
        option_string description
        option_string drive_folder_id
        record_entity entity_id FK
        option_string external_slack_channel_url
        option_string forge_provisioned_at
        option_string git_initialized_at
        string inserted_at
        option_string internal_slack_channel_id
        option_string internal_slack_channel_url
        string name
        option_string private_notion_page_url
        option_string repository_url
        option_string shared_notion_page_url
        string status
        string updated_at
    }
    project_module {
        record id PK
        string enabled_at
        option_record_person enabled_by_person_id FK
        string inserted_at
        string module
        record_project project_id FK
        string updated_at
    }
    question {
        record id PK
        string answer_type
        string audience
        string code
        datetime inserted_at
        string prompt
        datetime updated_at
    }
    relationship {
        record id PK
        int confidence_pct
        option_string detail
        record_person_entity in FK
        datetime inserted_at
        string kind
        record_person_entity out FK
        option_record_relationship_log_disclosure source_id FK
        string source_kind
        datetime updated_at
    }
    relationship_log {
        record id PK
        string action
        option_record_person actor_person_id FK
        string detail
        datetime inserted_at
        uuid subject_id
        string subject_type
        datetime updated_at
    }
    review_document {
        record id PK
        string body_html
        datetime inserted_at
        string kind
        record_notation notation_id FK
        string status
        string title
        datetime updated_at
    }
    schema_version {
        record id PK
        datetime applied_at
        int version
    }
    sent_email {
        record id PK
        string body
        datetime inserted_at
        string outcome
        string recipient
        string sender
        datetime sent_at
        option_string sg_message_id
        string subject
        option_string template_slug
        datetime updated_at
    }
    signature {
        record id PK
        option_string field
        datetime inserted_at
        record_notation notation_id FK
        string provider
        string provider_id
        option_string signed_at
        option_record_person signer_person_id FK
        datetime updated_at
    }
    statutory_deadline {
        record id PK
        string due_on
        string inserted_at
        string kind
        record_project project_id FK
        string source
        string status
        string statute
        string trigger_on
        string updated_at
    }
    template {
        record id PK
        option_record_asset asset_id FK
        string code
        option_string current_key
        option_string form_code
        datetime inserted_at
        bool is_current
        option_string kind
        option_record_project project_id FK
        string respondent_type
        option_string source_commit_sha
        string title
        datetime updated_at
    }
    testimonial {
        record id PK
        option_string attribution_label
        option_string consented_at
        int display_order
        string inserted_at
        record_person person_id FK
        record_project project_id FK
        option_string published_at
        string quote
        string updated_at
    }
    verification {
        record id PK
        record_citation citation_id FK
        datetime inserted_at
        string revision_sha
        string status_citation
        string status_proposition
        string status_quote
        datetime updated_at
        record_person verifier_person_id FK
    }
    visitor_route_count {
        record id PK
        datetime bucket_date
        string country_code
        datetime inserted_at
        string locale
        string route_pattern
        string source
        string status_class
        datetime updated_at
        int visits
    }
    xero_invoice {
        record id PK
        int amount_cents
        int amount_paid_cents
        string currency
        datetime inserted_at
        record_project project_id FK
        string reference
        string status
        datetime updated_at
        string xero_invoice_id
    }
    entity ||--o{ address : "entity_id"
    person ||--o{ address : "person_id"
    person ||--o{ answer : "authored_by_person_id"
    notation ||--o{ answer : "notation_id"
    person ||--o{ answer : "person_id"
    question ||--o{ answer : "question_id"
    project ||--o{ asset : "project_id"
    notation ||--o{ attestation : "notation_id"
    asset ||--o{ authority : "archived_asset_id"
    authority ||--o{ authority_use : "authority_id"
    project ||--o{ authority_use : "project_id"
    project ||--o{ case : "project_id"
    case ||--o{ case_docket_entry : "case_id"
    asset ||--o{ case_docket_entry : "document_asset_id"
    notation ||--o{ case_docket_entry : "notation_id"
    authority_use ||--o{ citation : "authority_use_id"
    asset ||--o{ communication : "asset_id"
    person ||--o{ communication : "author_person_id"
    project ||--o{ communication : "project_id"
    asset ||--o{ contract_review : "asset_id"
    notation ||--o{ contract_review : "notation_id"
    playbook ||--o{ contract_review : "playbook_id"
    jurisdiction ||--o{ credential : "jurisdiction_id"
    person ||--o{ credential : "person_id"
    entity ||--o{ disclosure : "entity_id"
    project ||--o{ disclosure : "project_id"
    discovery_request ||--o{ discovery_item : "discovery_request_id"
    case ||--o{ discovery_request : "case_id"
    case_docket_entry ||--o{ discovery_request : "docket_entry_id"
    person ||--o{ document_comment : "person_id"
    review_document ||--o{ document_comment : "review_document_id"
    notation ||--o{ email_conversation : "notation_id"
    person ||--o{ email_conversation : "person_id"
    email_conversation ||--o{ email_conversation_message : "conversation_id"
    person ||--o{ email_token : "person_id"
    entity_type ||--o{ entity : "entity_type_id"
    jurisdiction ||--o{ entity : "jurisdiction_id"
    person ||--o{ entity_role : "in"
    entity ||--o{ entity_role : "out"
    person ||--o{ expunge_record : "authorized_by_person_id"
    project ||--o{ expunge_record : "project_id"
    asset ||--o{ expunge_request : "asset_id"
    expunge_record ||--o{ expunge_request : "expunge_record_id"
    project ||--o{ expunge_request : "project_id"
    person ||--o{ expunge_request : "requested_by_person_id"
    person ||--o{ expunge_request : "resolved_by_person_id"
    notation ||--o{ filing : "notation_id"
    entity ||--o{ firm_anchor : "entity_id"
    person ||--o{ git_access_token : "person_id"
    project ||--o{ git_access_token : "project_id"
    mailroom ||--o{ letter : "mailroom_id"
    address ||--o{ mailroom : "address_id"
    asset ||--o{ notarization : "asset_id"
    person ||--o{ notarization : "notary_person_id"
    notation ||--o{ notarization : "notation_id"
    entity ||--o{ notation : "entity_id"
    person ||--o{ notation : "person_id"
    project ||--o{ notation : "project_id"
    template ||--o{ notation : "template_id"
    person ||--o{ notation_clause : "authored_by_person_id"
    notation ||--o{ notation_clause : "notation_id"
    person ||--o{ notation_event : "acting_person_id"
    notation ||--o{ notation_event : "notation_id"
    template ||--o{ notation_event : "template_version_id"
    person ||--o{ person_external_identity : "person_id"
    person ||--o{ person_mailbox : "person_id"
    person ||--o{ person_project_role : "person_id"
    project ||--o{ person_project_role : "project_id"
    entity ||--o{ playbook : "entity_id"
    entity ||--o{ project : "entity_id"
    person ||--o{ project_module : "enabled_by_person_id"
    project ||--o{ project_module : "project_id"
    person ||--o{ relationship : "in"
    entity ||--o{ relationship : "in"
    person ||--o{ relationship : "out"
    entity ||--o{ relationship : "out"
    relationship_log ||--o{ relationship : "source_id"
    disclosure ||--o{ relationship : "source_id"
    person ||--o{ relationship_log : "actor_person_id"
    notation ||--o{ review_document : "notation_id"
    notation ||--o{ signature : "notation_id"
    person ||--o{ signature : "signer_person_id"
    project ||--o{ statutory_deadline : "project_id"
    asset ||--o{ template : "asset_id"
    project ||--o{ template : "project_id"
    person ||--o{ testimonial : "person_id"
    project ||--o{ testimonial : "project_id"
    citation ||--o{ verification : "citation_id"
    person ||--o{ verification : "verifier_person_id"
    project ||--o{ xero_invoice : "project_id"
```
