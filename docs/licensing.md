# Licensing

Navigator is **source-available software, not open source**. Root [`LICENSE`](../LICENSE) is the licence of record, it
is the only licence file in the tree, and it covers everything the Firm is able to license:

| What | Licence | Why |
| --- | --- | --- |
| Workspace, CLI, build and deploy tooling | `BUSL-1.1` | Non-production use is free; production use is sold |
| Notation bodies under `templates/` | `BUSL-1.1` | Same grant; one tree, one answer |
| Blank government PDFs in `templates/forms/` | None — not ours to license | A Nevada state form belongs to Nevada |

One grant and one file is the whole design. A reader never has to work out which instrument governs the file in front of
them, and a fork never has to reconcile two sets of obligations across a directory boundary.

## The four parameters

BUSL is a template. Its terms are invariant — its fourth Covenant of Licensor forbids modifying them — so the entire
deal lives in the parameters block at the top of [`LICENSE`](../LICENSE), and reading the terms without the parameters
tells you almost nothing.

| Parameter | Value | What it decides |
| --- | --- | --- |
| Licensor | **Shook Law PLLC** | Who sells a production licence, and who may set every other parameter |
| Licensed Work | Neon Law Navigator | What is licensed |
| Additional Use Grant | Evaluate, develop, test, demonstrate — anywhere, so long as nothing relies on it | What you may run without buying anything |
| Change Date | Four years from each version's publication | When the restriction ends, per version |
| Change License | `AGPL-3.0-only` | What each version becomes |

**The Additional Use Grant pins the free zone.** BUSL's base grant already permits copying, modification,
redistribution, and *non-production use*; the Additional Use Grant is the slot a licensor uses to permit some **limited
production** use on top of that. This parameter was `None` until 2026 — not a restriction, just the absence of the extra
permission — and `None` left BUSL's undefined term, "production use", carrying the entire commercial boundary. An
undefined term in a licence is read against the party that drafted it, and that party is the Firm. So the slot now
states affirmatively what you may run: Navigator, on infrastructure you control or rent, to evaluate, develop against,
test, or demonstrate it, for so long as it performs no work anybody relies on.

**It is written as a grant and not as a definition, deliberately.** BUSL's second covenant allows only two things in
that slot: a grant that imposes no additional restriction on the base grant, or the literal word `None`. A clause
*defining* "production use" broadly would narrow the base grant's non-production use and risk being exactly such a
restriction — which would breach the covenant the Firm's permission to use BUSL is conditioned on. A clause that only
adds permission cannot. So the free zone is operative text; what lies outside it remains BUSL's own undefined term, and
the Firm's reading of that lives in [`NOTICE`](../NOTICE) and below, where it binds nobody.

The same paragraph opens the grant in `navigator-ux` and in the Homebrew tap. A legal review that has cleared one of
the three has cleared all three.

**The Change Date runs per version.** Each published version carries its own four-year clock, so a version published
today converts four years from today whatever happens to the ones after it. That is BUSL's own rule, not a choice: its
terms say the licence "applies separately for each version" and that the change happens on the Change Date or the fourth
anniversary of that version's first public distribution, whichever comes first.

**Why `AGPL-3.0-only` is a permitted Change License.** BUSL's first covenant obliges the Change License to be GPL-2.0 or
any later version, or something compatible with GPL-2.0 or a later version — where compatible means code under the
Change License can be included in a program with GPL-licensed code. AGPL-3.0 is not GPL-2.0-compatible, but it does not
need to be: GPL-3.0 is "a later version", and GPL-3.0 § 13 expressly permits combining a GPL-3.0 work with an AGPL-3.0
work. So AGPL-3.0-only code can be included in a program with GPL-3.0 code, which is what the covenant asks.

## Two files, and which one is the instrument

[`LICENSE`](../LICENSE) is the licence text and its parameters, and nothing else: it opens on the licence's own title,
ends on its last covenant, and carries no prose of ours in between. The parameters block is part of the instrument —
BUSL is filled in by its licensor — so the Firm's own name appears there legitimately, where under the FSF's text it
never could. [`NOTICE`](../NOTICE) beside it carries everything we have to *say* — the copyright line, what each
parameter means, where the production boundary falls, the SPDX tag, the government forms nobody here can license, the
marks the Firm reserves, and the terms a contribution arrives under.

The split is what every other project does, and there is a mechanical reason for it. A licence file is read by machines
as well as people: GitHub's repository page, `cargo deny`, SBOM generators, and a corporate review team's scanner all
decide *which* licence they are looking at by comparing the file to the canonical text. Each of them wants a near-exact
match. Add a page of our own prose in front of the grant and the comparison drops under the threshold — the sidebar
stops saying `BUSL-1.1`, the scanner reports an unidentified licence, and a reader who wanted one glance instead gets a
file to read. The most useful thing a licence file can do is be recognised on sight, which costs exactly one discipline:
nothing of ours goes in it beyond the parameters it asks for.

`NOTICE` neither widens nor narrows the grant. It is where this work meets that text, not a second instrument, and
[`cli/tests/license_of_record.rs`](../cli/tests/license_of_record.rs) holds both files to that shape — `LICENSE` bounded
at both ends by the licence's own lines, its five parameter values asserted individually, so there is nowhere in it for
an added clause to sit and no way to move the deal without failing a test.

## Who holds what

Two facts, one organization.

| Held | By | Which is why |
| --- | --- | --- |
| Copyright in this repository | **Shook Law PLLC**, the law firm | Only the holder can grant the whole work |
| **NEON LAW**, U.S. Reg. No. 6,325,650 | **Shook Law PLLC**, the law firm | The mark is not licensed here at all |

**Why the Firm holds it.** The Firm writes this software, engages the people who write the rest of it, and operates a
legal practice on it under the NEON LAW mark. A mark on legal services is how a client identifies who is accountable for
their legal work, and that accountability belongs to the entity holding the bar licence — so the mark was always going
to be the Firm's, and putting the copyright in the same hands is what lets one signature grant the whole work.

**The Firm is the sole Licensor**, and no other party is entitled to publish this work. That is worth stating rather
than leaving to inference, because the alternative — a second organization holding its own right to publish — would make
a stated licence untrue while the repository looked exactly the same.

**What that costs, stated plainly.** A sole Licensor may stop publishing, or change these parameters, whenever it
chooses; nothing here promises otherwise. What a reader can rely on is narrower and does not depend on the Firm's later
goodwill, or on the Firm still existing: every copy already distributed keeps the terms it came with, and every version
published under BUSL converts to `AGPL-3.0-only` on its own Change Date, because that conversion is a term of the
licence each of those copies already carries.

**The practical consequence** is the sentence to read if you read only one: read it, build it, fork it, change it, and
redistribute it — none of that needs anyone's permission. Running it to deliver legal services to other people is
production use and needs a commercial licence. Calling the result "Neon Law" needs the Firm's permission, and the Firm
does not give it.

## Chain of title

Who wrote Navigator, and how the copyright reached the organization that holds it. A reader working out who can license
them anything follows the route recorded here, rather than a conclusion repeated elsewhere in the tree.

### The authors

Two people have written Navigator.

| Author | What they wrote |
| --- | --- |
| Nicholas Shook | The work before publication, and most of it since |
| Jaskaran Singh | Contributions across the rule engine, the CLI, the store, the portal, telemetry, and the build |

**The published Git history is not the authorship record**, and should not be read as one. This repository's root commit
is a *publication* event that lands the whole tree at once, so a date of creation inferred from it is the day the work
was published rather than any day it was written.

### The instruments

One instrument, and it is neither reproduced nor quoted here: what this section records is that it exists and between
whom. Its date is held with the executed original rather than published, though a recordation under § 205 puts that date
on the public record in its own right.

| Instrument | Parties | Status |
| --- | --- | --- |
| Assignment of copyright, 17 U.S.C. § 204(a) | Nicholas Shook and Shook Law PLLC, to Shook Law PLLC | Executed |

Both assignors join it, each conveying whatever interest it holds. That is deliberate and costs one signature block:
whether the work was already the Firm's turns on an employment question nobody needs to answer, and it stops mattering
once both have conveyed.

Contributions arrive already assigned. Each author signed an employment or contractor agreement with Shook Law PLLC
before their first commit, so no contribution in this tree needs a later transfer — see [Contributions](#contributions).

### Registration and recordation

Neither has happened yet, and this section is where the numbers will land.

| Step | What it is for | Status |
| --- | --- | --- |
| Copyright Office registration | Statutory damages and fees under § 412 | Not yet filed |
| Recordation under 17 U.S.C. § 205 | Constructive notice of the transfer to everyone | Not yet recorded |

The instrument is not filed with the application. The Copyright Office does not interpret a transfer document, so the
application states that the claimant obtained the work by written agreement and the instrument is recorded separately.

### This is not necessarily the last link

Moving Navigator's copyright to a separate entity that does not practise law is under consideration. If that happens
this section gains a row rather than being rewritten. A transferee would take the copyright unencumbered, and would
become the Licensor who sets these parameters — so the transfer is a decision about who controls the licence, not only
about who owns the work.

So read the holder named here as the holder of record today, and read this section — rather than any single sentence
elsewhere — as the place the answer lives.

## The grant: BUSL-1.1

Every line of code and every line of drafted legal prose is licensed under the Business Source License 1.1
([`LICENSE`](../LICENSE)). Cargo and npm manifests declare `BUSL-1.1`.

What the base grant gives you, before any parameter: copy the work, modify it, create derivative works from it,
redistribute it, and make **non-production use** of it. What it does not give you is production use. The Additional Use
Grant adds one thing on top: you may *operate* the work — anywhere, the cloud included — to evaluate, develop against,
test, or demonstrate it, for so long as nothing relies on what it does.

Three conditions ride along with it, and they are the ones a fork actually has to act on:

- **Display the licence.** BUSL requires this License to be displayed conspicuously on every original or modified copy,
  which is `LICENSE` and `NOTICE` travelling together — including inside a container image.
- **The licence follows every copy and every derivative.** Receiving a modified copy from a third party does not change
  your terms; you hold this licence too.
- **Violating it terminates your rights**, automatically, for every version and not only the one you misused.

`deny.toml`'s allowlist is a different question. It governs what this workspace is willing to *consume*, which has
nothing to do with how the workspace is licensed out. The property an inbound licence needs is that it can be
distributed inside this work — and, because every version converts on its Change Date, inside an AGPL work four years
later too.

### Where the production line falls

BUSL does not define "production use". The licence answers the question from the other side instead: the Additional Use
Grant states what you may run, and [that is operative text](#the-four-parameters) rather than commentary. Inside it you
need nothing from us.

**Granted by `LICENSE` itself:** operating Navigator on infrastructure you control or rent — a laptop, a rented VM, CI,
a cloud sandbox — to evaluate, develop against, test, or demonstrate it, for so long as it performs no work that you or
anybody else relies on. Reading the source, building it, running the test suite, standing up the local KIND tier,
benchmarking it, and teaching from it sit inside that comfortably, as do internal experiments and proofs of concept.

**The Firm's reading of what lies outside**, offered as our own account rather than as a term: running Navigator where
it performs work you or anybody else relies on — a portal, a matter, a filing pipeline — is production use, and so is
marketing to customers a product or service that relies on it.

**The test is reliance, not where the software runs.** That choice was deliberate. A test keyed to deployment topology —
"any cloud deployment is production use" — fails in both directions: it reaches the cloud sandbox that owes nothing,
which is most evaluation now, and it misses the competitor serving clients from their own hardware, which is the use
worth reaching. Reliance inverts both.

If your case does not obviously fall on one side, write to `contact@neonlaw.org` and ask before deploying rather than
after. A question costs nothing; a production deployment discovered later is an awkward conversation for both parties.

### Section 13 arrives at the Change Date

**BUSL carries no network clause,** and needs none: the deployment shape such a clause reaches — running a modified
version as a service for other people — is production use, which this licence grants only through a commercial licence.

**When a version reaches its Change Date it becomes `AGPL-3.0-only`, and § 13 attaches to that version in full.** From
then on, anyone who modifies that version and lets users interact with it remotely over a network must offer *those
users* — the people using that operator's own instance — the corresponding source of what they are running. The duty
runs in that one direction only.

Two things § 13 will still not do when it arrives:

- **It does not reach an unmodified deployment.** Running a converted version as published carries no source obligation,
  because the corresponding source is already here.
- **It does not reach client data.** § 13 obliges you to offer *your modified software*. A matter, a document, and a
  client's facts are not the software, and nothing in either licence asks for them.

### What a fork owes, in order

1. **Keep the notices, and display the licence.** BUSL conditions your rights on displaying this License conspicuously
   on every copy — which is `LICENSE` and `NOTICE`, travelling together.
2. **Stay out of production, or buy a licence.** The Additional Use Grant reaches evaluation, development, testing, and
   demonstration and no further, so there is no third option. Moving the deployment off the cloud is not one either:
   the test is reliance, not where it runs.
3. **Pass the licence on.** Every copy and derivative you convey is subject to it, and the recipient holds the same
   terms you do.
4. **Offer your source to the users you operate it for** — once the version you are running has converted, per § 13
   above.
5. **Rename it.** Not a copyright obligation at all — see [Trademarks](#trademarks). The brand manifest
   (`views::brand_bundle`) is the seam.

## Government forms: nobody's to license

The blank government PDFs under `templates/forms/` are works of the issuing state or federal agency. Nobody here claims
a copyright in them or grants one; they are committed so the binary embeds the same bytes the repository carries, and
for no other reason.

This is not a technicality. Claiming a licence over a state's own form would be over-claiming a copyright nobody here
holds, and an over-claim in a terms file published beside a law practice is the kind of error that gets quoted back.
What *is* licensed beside each blank PDF is our own material: the catalog card, the field map, and the workflow that
fills the form in.

## Why the legal prose is under the software licence

The notation bodies under `templates/` carry the documents a client signs, together with the questionnaire prompts and
workflow definitions in the same files. They are licensed `BUSL-1.1` with everything else, because in this tree a
template is not a document sitting near a program — it is an input to one. A notation body is parsed by the workflow
engine, validated against the `N`-family rules, and rendered; the prose, the prompts, and the state machine are the same
file, and a rule change and a clause change arrive through the same review. A licence boundary drawn inside that file
would ask a contributor to work out which half of a line they were editing, and a fork to track two obligations through
one file.

Attribution, which is what drafted prose actually needs, is already a subset of what the licence requires: every copy
displays this License and carries its notices.

## Why the source is public

Neon Law charges published flat fees for consumer legal work. That economics only holds if routine matters cost very
little to run, and this software is what makes them cost little. Publishing the source is the same argument as
publishing the prices: a legal system where only the well-resourced can afford counsel is not fixed by one firm being
efficient in private.

**Source-available is the promise this repository makes, and it is worth naming precisely.** The source is readable by
anyone, nothing in the mechanism is secret, and every published version becomes `AGPL-3.0-only` four years on. The Firm
sells the right to operate it in the meantime, and says so.

Four consequences follow, and they are the trade being made:

- **No trade-secret protection.** Anything published cannot be un-published, so no mechanism in this tree is a secret.
  BUSL restricts use, not reading.
- **The confidentiality boundary is procedural.** A publication path exists, so the no-client-data rule is enforced by
  a load-bearing test on every pull request — see [`agent-workflows.md`](agent-workflows.md#no-client-data-in-the-repo).
- **Forks are expected, and none of them owes this project anything.** Reading, building on, and redistributing
  Navigator stay free. No fork has to send anything here and none is asked to. The brand manifest
  (`views::brand_bundle`) exists so a fork renames itself without patching sources.
- **The restriction has an expiry, per version.** Four years is the whole of it. A reader who thinks that is too long
  is disagreeing about a number rather than about whether the work eventually becomes free software again.

The trademark reservation below protects the thing that actually distinguishes the practice, which is a separate
question from either licence.

## Commercial licensing

**Production use needs a commercial licence, and only the copyright holder can grant one.** That holder is **Shook Law
PLLC**.

**What is on offer is the right to run Navigator in production.** The Additional Use Grant reaches evaluation,
development, testing, and demonstration only, so operating Navigator where somebody relies on what it does takes a
licence from the Firm.

**Non-production use needs no licence.** Reading the source, building it, running the tests, standing up the local tier,
evaluating it, and developing against it are all granted by the licence itself.

**Only the Firm can grant it.** This is not a policy choice that could have gone another way. A production exception is
a permission carved out of the copyright, and a permission can only be given by whoever holds the right. The Firm holds
it, and no other party holds anything to carve from.

**No price is published here or anywhere on the website.** A deployment's scope is not knowable in advance, so a figure
would be a floor dressed as a fee — the same reason litigation and fractional general counsel carry none while the
consumer flat fees are published in full. Write to [contact@neonlaw.org](mailto:contact@neonlaw.org).

**Legal aid and nonprofit deployments should write.** There is no standing programme with published terms, but the Firm
can license a legal aid office directly, and the reason such a programme would exist has not gone away. Ask.

## Trademarks

**NEON LAW** is a registered trademark, U.S. Reg. No. 6,325,650, owned by Shook Law PLLC. The licence grants rights in
copyright, not in trademarks, and [`NOTICE`](../NOTICE) says so explicitly — a reader deciding whether they may ship a
fork called "Neon Law" reads the terms that shipped with the code, so the answer has to be there rather than only in a
doc. It is also the reason `NOTICE` travels in every archive and image: the reservation is the one thing the grant does
not hand a fork.

The registrant and the copyright holder are the same organization — see [Who holds what](#who-holds-what) — and that
changes nothing. A copyright licence conveys rights in copyright. It does not reach a mark, so the Firm granting you
everything it can grant under `BUSL-1.1` still leaves you without the name.

This is the one reservation this project genuinely needs. A client identifies who is accountable for their legal work by
the name on the door, so a fork trading as Neon Law would misdirect the person least able to check. Anyone may run,
fork, and redistribute the software, and may say their work is built on Neon Law Navigator; nobody may present their
deployment as Neon Law.

## Contributions

**Outside contributions are closed right now**, and anyone is welcome to write to
[contact@neonlaw.org](mailto:contact@neonlaw.org) instead. That is a capacity decision about pull requests: every copy
already cloned keeps its rights whatever the contribution policy says.

**A contribution assigns to Shook Law PLLC — all right, title, and interest in it, including every copyright in it,
worldwide and for the full term.** That assignment is the contributor licence agreement, and the terms sit here so they
are knowable before anyone writes a line rather than at a merge. Inside the firm the instrument is the employment or
contractor agreement each author signed before their first commit, so the work is the Firm's on creation; an outside
contributor signs a contributor licence agreement on the same footing before a contribution merges.

Assignment is the mechanism and the grant to you is the result: the work reaches you under `BUSL-1.1`, the same terms
the project ships under, wherever in the tree it lands. What it buys is a single holder able to grant the whole work,
which is what lets one licence cover the tree and one party set its parameters. See
[`CONTRIBUTING.md`](../CONTRIBUTING.md).

Two boundaries survive the opening, because this repository runs a live practice: **no client data ever enters the
tree**, and **a change to `templates/` gets attorney review** before it merges, however mechanical the diff looks.

## What the binary carries

An installed `navigator` is one executable that may sit far from anything it shipped beside, so its terms are compiled
into it as well as staged in the archive. BUSL requires this rather than merely inviting it: the licence conditions the
permission to convey on handing every recipient a copy of this License along with the work. A bare executable someone
was given is a copy — and its parameters are what tell that holder whether their own use needs a commercial licence,
which nobody works out from terms they were never shown.

- `navigator --license` prints [`NOTICE`](../NOTICE) and then [`LICENSE`](../LICENSE), both embedded with
  `include_str!`. The notice comes first: it is the half that names this program, its copyright holder, and the marks.
- `navigator --third-party-notices` prints `THIRD-PARTY-NOTICES.txt`, likewise embedded.

Each release archive carries `LICENSE` and `NOTICE` beside the executable, so an unpacked archive states its own terms
before anyone runs anything.

`THIRD-PARTY-NOTICES.txt` is generated by `navigator ops notices` from `Cargo.lock`. A statically linked Rust binary
carries the compiled form of every crate in its dependency tree, and the terms those crates ship under require their
notices to travel with the distributed work. Each distinct licence text appears once, listing the crates that carry it;
crates that publish no licence file are listed with the SPDX expression their manifest declares. Regenerate and commit
it whenever the dependency tree moves:

```bash
cargo fetch
cargo run -p cli -- ops notices
```

`cargo fetch` is part of that pair, not a convenience. Licence text is read from `$CARGO_HOME/registry/src`, and cargo
unpacks a crate there only when something needs it — a build unpacks the platform it built for, so a machine that has
only ever built for macOS has never unpacked the Linux- and Windows-only crates `Cargo.lock` also names. `cargo fetch`
with no `--target` unpacks every target's graph, which is what makes the generated file the same on any machine.

A crate whose source is absent says nothing about that crate's licence — it says this machine never unpacked it. So the
command refuses to write or to check rather than listing such a crate among the ones that publish no licence file: that
conflation would publish one machine's gap as the crate's, and a permissive licence whose text was never read is a
permissive licence whose notice did not ship.

`navigator ops notices --check` fails when the committed file is stale, which is the gate a release should run. It runs
in the `rust` job of `.github/workflows/ci.yml` on every pull request, and again in the release preflight.

## What the images carry

A container image someone pulled is a copy too, and its holder has neither the repository nor a release archive. Every
published image therefore does both of the things a registry makes possible:

- `LABEL org.opencontainers.image.licenses="BUSL-1.1"`, which Artifact Registry and GHCR read for the package
  page — what a reader sees *before* pulling.
- `LICENSE` and `NOTICE` staged at `/app` beside the binary — what a running container can be made to show.

`Containerfile.runner` is exempt: it is the CI runner image rather than a published artifact of the software.

### Where the images are published

Every product image goes to `ghcr.io/neon-law-source-code`, from `publish-service` and `publish-triggers`. There is no
toggle and no second registry: the push is unconditional, and `cli/tests/license_of_record.rs` asserts that no condition
guards it. A registry the release depends on must not be switchable by a repository variable, whose absence is a
settings edit that touches no file, passes every gate, and yields a release that looks fine until someone checks the
registry days later.

No credential to create. The repository and its Actions live on github.com, whose own registry is `ghcr.io`, so the push
authenticates with `GITHUB_TOKEN` and the `packages: write` scope. That scope is granted on those two jobs rather than
at the top of the workflow — every other job in the file checks out and builds release code, and none of them has any
business writing packages.

**A GHCR package inherits its linked repository's visibility.** `neon-law-source-code/navigator` is public, so these are
public packages — anyone can pull the same digests the deployments run, which is the point of publishing them.

## Releases

Release archives carry `LICENSE` and `NOTICE` so an unpacked archive states its own terms without the repository tree,
and the binary prints both, plus the third-party notices, itself. Someone who ends up with nothing but the executable
can still read the terms they are running under and the attributions it is obliged to carry.
