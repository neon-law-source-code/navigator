# Client trust accounting — the IOLTA mirror

How Neon Law Navigator tracks client funds the firm holds in trust: the money a client pays **in advance** of the work,
held until it is earned, and refunded if the engagement ends early. **Xero is the books.** The firm's bookkeeper records
every deposit, refund, and withdrawal there, against a real bank account; Navigator holds a read-only mirror so a client
can see their own funds and a lawyer can see a matter's position without opening Xero. See
[`xero-billing.md`](xero-billing.md) for the invoice-out side of the same integration.

The modules are [`store/src/trust.rs`](../store/src/trust.rs) (the per-matter ledger),
[`store/src/iolta_accounts.rs`](../store/src/iolta_accounts.rs) (one pooled account per state), and
[`store/src/iolta_withdrawals.rs`](../store/src/iolta_withdrawals.rs) (one withdrawal, many invoice allocations).

## Why this exists, and why it is not the bank

A matter's fee terms are agreed per engagement and stated in the retainer's clauses, so a retainer may collect an
advance against them. Under NRPC 1.5 and California Rule 1.15, an advance fee is **not earned on receipt**: it is held
in the client trust account, earned as the work is performed, and any unearned remainder is **refunded** when the matter
closes early. That earned/unearned split and the refund are a bar-rules obligation — no bank, payment processor, or
accounting package computes them for us. So the firm-specific logic lives in `store::trust` as pure, provider- and
asset-agnostic Rust.

Navigator **does not custody funds** — the invariant from [`xero-billing.md`](xero-billing.md) holds. It opens no
account, moves no money, raises no withdrawal, and writes nothing back to Xero. What it owns is the **legal-meaning
overlay**: which matter a movement belongs to, what it means, and the running trust position per matter.

## What a trust ledger has to do (IOLTA)

Trust accounting is regulated bookkeeping. A compliant setup keeps three balances that must always agree — the **monthly
three-way reconciliation**:

1. **The bank statement** — what the bank says is in the pooled trust account.
2. **The trust master ledger** — the firm's own book balance for that account.
3. **The per-matter balances** — how much of the pooled money belongs to each client/matter, summing to the master.

Xero supplies balances 1 and 2. Navigator mirrors the account balance Xero reports (`iolta_account.balance_cents`) and
folds balance 3 from the matter ledgers. Because Navigator holds no funds, it cannot *break* reconciliation; it exists
to make the per-matter side auditable at any time, and to show a client their own share of it.

## One pooled account per US state

IOLTA is pooled **by jurisdiction**: every client whose matter is governed by Nevada law shares one Nevada trust account
at the bank. `iolta_account` mirrors one Xero bank account per `jurisdiction` of type `state`, UNIQUE on the
jurisdiction — a second Xero account claiming Nevada is a refused write, not a silent second master balance.

A matter reaches its pool through **`project.jurisdiction_id`**, the matter's general governing jurisdiction. The pooled
account is derived from it rather than recorded a second time, so there is one authoritative jurisdiction per matter and
no second mapping to drift. A matter that names no jurisdiction, or names a country, has no pooled state account: the
nightly mirror **counts** it and moves on. Nothing infers a trust account from the client entity or the owning firm.

Xero has no field for "which US state does this account pool for", so the firm declares it in the **account name** —
`IOLTA NV — Trust` — the twin of the `Matter <code>` convention invoices use. The firm's operating and payroll accounts
come back on the same Xero read and are counted as unscoped, never turned into a trust pool.

## The model: immutable double-entry postings

Each trust movement is a **double-entry posting** — value flows from one account to another — recorded as one immutable
event. There are three kinds:

| Kind | Flow | Mirrored from |
| --- | --- | --- |
| `deposit` | client → `trust:<project>` | a `RECEIVE` bank transaction on the state's pooled account |
| `earned_draw` | `trust:<project>` → `operating` | this matter's share of a pooled withdrawal |
| `refund` | `trust:<project>` → client | a `SPEND` whose lines settle no invoice |

The accounts are `client:<project>` (the client's own funds), `trust:<project>` (this matter's individual client
ledger), and `operating` (the firm's earned revenue). The trust balance for a matter is everything that flowed **into**
`trust:<project>` minus everything that flowed **out**. Because earned money is drawn out, whatever remains in trust is
by definition still **unearned** — so `held == unearned == refund-on-close`, and `store::trust::Position` exposes all
three off the same fold. `held` is always derived, never a stored column that could disagree with the postings.

A posting is mirrored only if it holds together three ways: the transaction's `Matter <code>` reference resolves to a
live Project, that Project's jurisdiction has a mirrored pooled account, and the money moved through **that** account. A
Nevada deposit landing against a California-governed matter is a bookkeeping error; it is counted for a human rather
than posted somewhere plausible. Every posting carries the Xero `BankTransactionID` in `Movement::external_ref`, which
is what makes a re-read of the same night idempotent.

### One withdrawal, many invoices

The firm draws earned fees out of trust in **one bank transfer**, which may settle several matters' invoices at once.
The bank sees one movement; each client is entitled to see their own share and nothing else. So the transfer is mirrored
as one `iolta_withdrawal` plus one `iolta_allocation` per invoice it settles, and each matter draws once for the total
of its own lines.

A line names an **invoice**, never a matter: the matter is read off the invoice it settles, so a line cannot claim an
invoice belongs to someone else. Four things make a withdrawal refuse, and it refuses **whole** — no row, no line, no
draw, not even for the matters that would have been fine:

* the lines do not sum to exactly the transfer;
* a line names an invoice the mirror does not carry;
* a matter is governed by another state, or by none;
* a matter's lines exceed what that matter holds.

Overdraw is never clipped to what fits. Drawing more than a client has in trust is the error trust accounting exists to
prevent, and clipping would hide it where a refusal puts it in front of a human. The refusal is *reported*, not fatal —
one bad transfer does not stop the rest of the night's states from reconciling.

### What each audience sees

A **client** sees their own matter's position — paid in, still held, earned and drawn, refunded — and their own
allocation lines: how much of the firm's draw settled which of *their* invoices, and when. They never see the pooled
transfer's total, another matter's share, or any Xero id. A **lawyer** on the matter sees the same client-safe numbers;
both surfaces render through one constructor so they cannot drift apart. The firm-side read of a whole pooled withdrawal
(`iolta_withdrawals::lines_for`) is not on any client surface.

### Proration

Where the fee runs monthly, the first partial month due at signing is `monthly_fee × (days_remaining ÷ days_in_month)`,
where `days_remaining` equals `days_in_month − day_of_month` — the balance of the signing month **after** the signing
date. Any signing on **July 8** prorates to `(31 − 8) / 31` of the fee, collected at signing; a signing on the last day
of the month prorates to `0`, so the first full month bills at the next period boundary. The fee may **flex by phase**:
a litigation matter stepping from pleadings to discovery to trial prorates the **new** phase's monthly fee the same way
from its effective date, tagged with `Movement::phase`. `prorate_first_month` rounds to the nearest cent (half up) and
is exercised against a table of signing-date cases — month lengths, both February variants, and the boundaries.

### Storage — an append-only journal, no new table

A trust movement is recorded as an immutable JSON event on the existing `notation_events` journal under the
`trust_ledger` machine-kind, anchored to a notation of the matter it belongs to — the matter's earliest, which is the
engagement's own. A matter with no notation has nothing to anchor a posting to; the mirror counts it rather than
anchoring to something invented.

That journal is append-only **at its command seam**: `store::notation_events` exports an append and reads, and no update
or delete function at all, pinned by a covering test. There is no database trigger — the module's public shape is the
invariant, and a correction is a new reversing posting. Reusing the journal means the per-matter ledger adds **no schema
of its own**; the two IOLTA tables above hold the pooled account and the withdrawal split, which are facts about the
bank rather than about a matter's postings.

## Assets: USD and crypto

A movement records the **asset actually moved** (`asset` + `amount`) as a decimal *string* — never a float, and never
assuming USD cents (a wei-denominated ETH amount overflows `i64`). The **fee obligation is denominated in USD**, so the
accounting math runs on the common denominator `usd_value_cents` — the USD value credited at receipt — while `asset` /
`amount` / `rail` / `external_ref` preserve the in-kind record for audit.

The legal frame differs by asset. Pooled **USD** held for clients is the classic **IOLTA** concern, and is what the Xero
mirror covers. Client **crypto** is not IOLTA at all — it is **safekeeping of client property** under RPC 1.15's
property branch, tracked in kind like a held stock certificate rather than swept into a pooled interest-bearing account.
The schema does not bake in "cents in a USD bank account," so both fit the same postings.

## Related

* [`xero-billing.md`](xero-billing.md) — the receivables side: who raises an invoice, the `Matter` reference, and the
  nightly ingest.
* [`third-party-integrations.md`](third-party-integrations.md) — the vendor-account convention. Mercury appears there
  as a People and Entities integration; it is not a money rail for Navigator.
