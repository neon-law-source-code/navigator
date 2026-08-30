# On-chain attestation — Neon Law Node (Solana)

**Status: scaffolded, not shipped.** The durable local record and the workflow seam are implemented and tested; the
on-chain write itself is deferred behind a trait. The Node product page says so plainly, and the code cannot lie about
it — see [Why it can't claim a false record](#why-it-cant-claim-a-false-record).

Neon Law Node is an attorney attestation recorded on-chain: a licensed attorney confirms a fact from the records a
client provides, and a hash of the signed attestation is written to Solana, binding the firm's wallet, the client's
wallet, and that hash. Solana is the chain because the workspace is Rust top-to-bottom — Solana programs are written in
Rust and [Anchor](https://www.anchor-lang.com/) is a framework of Rust macros — so the same workspace speaks to the
chain natively. The binding engagement letter is `templates/neon_law/shared/onboarding_letter.md`.

## What is built

The vertical is implemented end-to-end except the chain write, mirroring how `cloud::StorageService` keeps GCS out of
the generic layer:

- **The local record is the system of record.** Every attestation writes one row to the `attestation` table in
  SurrealDB (`store::attestations`) — one row per notation by convention, since the Surreal schema carries no unique
  index on `notation_id`. The row carries the document SHA-256 plus identifiers (wallets, PDA, transaction signature)
  and a `status` of `pending` / `recorded` / `failed` — **never client** content, the same trust boundary telemetry
  observes. SurrealDB is snapshotted by the scheduled `surreal-archive` CronJob (see
  [`surreal-archives.md`](surreal-archives.md)), so the attestation inherits the firm's ten-year retention for free.
- **The chain is isolated behind a trait.** `workflows::attest::Attestor` has one method, `record`, returning a
  `RecordedTx` or `None`. `NullAttestor` is the default (records nothing); `attestor_from_env` reads
  `NAVIGATOR_ONCHAIN_BACKEND` (`null` default, `solana` reserved and currently erroring by design). Selecting a chain —
  or a second chain later — is a new `impl Attestor`, never a workflow edit.
- **The step is provider-neutral.** `StepKind::OnChainRecord` binds the `onchain__` prefix; `dispatch_onchain_record`
  hashes the document at the payload's `storage_key`, calls the attestor, and writes the row inside the worker's
  `ctx.run` (replay-idempotent). The attestor rides on `StepDeps` via `with_attestor`, so a step reached without one
  errors clearly rather than silently skipping.

### Why it can't claim a false record

The honesty is enforced by code, not copy. `dispatch_onchain_record` sets `status = recorded` **only** when the attestor
returns a real `RecordedTx`; `NullAttestor` returns `None`, so the row stays `pending` with no transaction. And
`attestor_from_env` makes the `solana` backend *error at startup* until the real implementation lands — a deployer can
never silently believe attestations are going on-chain when they are not. This is why the step is deliberately **not yet
wired into the binding `onboarding__letter` workflow**: a binding retainer must not route through a step that, in its
current default, records nothing.

## What is deferred

Two pieces of code and four decisions. The code is straightforward; the decisions are the real gate and want a council
before any keypair touches production.

### The code

**1. `SolanaAttestor`** — an `impl Attestor` holding an RPC client, the program id, and the firm signer. It builds the
`record_attestation` instruction, submits it, waits for the chosen commitment, and returns the signature + PDA.

**2. The Anchor program** — one instruction, `record_attestation`, that initializes a Program Derived Address seeded by
the notation id (`init, seeds = [b"attestation", notation_id], bump, payer = firm`), storing:

```rust
struct Attestation { firm: Pubkey, client: Pubkey, sha256: [u8; 32], recorded_at: i64 }
```

The PDA is also the **exactly-once key**: a replayed submit hits "account already in use", which the attestor treats as
success — the chain itself dedupes, complementing the journaled `ctx.run`.

**3. The workflow edge** — the one-line YAML change in `templates/neon_law/shared/onboarding_letter.md`, routing the
signature into the new step:

```yaml
sent_for_signature__pending:
  signature_received: onchain__record_attestation
  signature_declined: END
onchain__record_attestation:
  attestation_recorded: END
  attestation_failed: lawyer_review
```

This ripples into the retainer / e-signature test suite (the tests that assert `signature_received → END`), so it lands
*with* the `SolanaAttestor` and its test updates, not before.

### The decisions (not code)

"It's written in Rust" chose the SDK; it did not answer any of these. Each gates production:

- **Firm key custody.** The `SolanaAttestor` signs and pays fees from a firm wallet. That keypair belongs in KMS /
  Secret Manager with rotation — never `SOLANA_SIGNER_SECRET` as a path on disk. `.env.example` holds a *reference*, not
  the key.
- **Client wallet.** The retainer promises "the client's wallet." Do we collect a client public key at intake (a new
  questionnaire field — none exists today) or mint a custodial one? This is a product decision with a UX cost.
- **Public-chain confidentiality.** Only the hash + two public keys + a timestamp go on-chain, never content — but a
  hash and two wallets are world-readable forever. The retainer's RPC 1.6 clause already discloses this; confirm the
  client's informed consent covers a permanent public record.
- **Finality and cost.** Transition to `attestation_recorded` only at `finalized` commitment. Fees are SOL, passed
  through "at cost" per the retainer, so the firm needs a funded treasury and a devnet → mainnet switch.

## Configuration

`NAVIGATOR_ONCHAIN_BACKEND` selects the backend (`null` default). When the `SolanaAttestor` ships it reads
`SOLANA_RPC_URL`, `SOLANA_PROGRAM_ID`, and a KMS reference for the signer. See `.env.example` for the committed
contract.

## Pointers

- Seam and dispatch: `workflows::attest` (the `Attestor` trait, `NullAttestor`, `dispatch_onchain_record`).
- Local record: `store::attestations` + the `attestation` table in `store/src/schema/navigator.surql`.
- Step kind / status table: `workflows::step` and [`docs/notation-authoring.md`](notation-authoring.md) (the `onchain__`
  row).
- Engagement surface: `templates/neon_law/shared/onboarding_letter.md` (the binding engagement letter).
