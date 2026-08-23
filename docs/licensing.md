# Licensing

Navigator is free software. Root [`LICENSE`](../LICENSE) is the licence of record, it is the only licence file in the
tree, and it covers everything the Firm is able to license:

| What | Licence | Why |
| --- | --- | --- |
| Workspace, CLI, build and deploy tooling | `AGPL-3.0-only` | Copyleft with a network clause, which is how it is run |
| Notation bodies under `templates/` | `AGPL-3.0-only` | Same grant; one tree, one answer |
| Blank government PDFs in `templates/forms/` | None — not ours to license | A Nevada state form belongs to Nevada |

One grant and one file is the whole design. A reader never has to work out which instrument governs the file in front of
them, and a fork never has to reconcile two sets of obligations across a directory boundary.

## Two files, and which one is the instrument

[`LICENSE`](../LICENSE) is the Free Software Foundation's text, unaltered: it opens on the licence's own first line,
ends on its last, and carries nothing of ours in between. [`NOTICE`](../NOTICE) beside it carries everything we have to
say — the copyright line, the publication right the Foundation holds, the SPDX tag, § 13 in its own voice, the
government forms nobody here can license, the marks the Firm reserves, and the terms a contribution arrives under.

The split is what every other project does, and there is a mechanical reason for it. A licence file is read by machines
as well as people: GitHub's repository page, `cargo deny`, SBOM generators, and a corporate review team's scanner all
decide *which* licence they are looking at by comparing the file to the canonical text. Each of them wants a near-exact
match. Add a page of our own prose in front of the grant and the comparison drops under the threshold — the sidebar
stops saying `AGPL-3.0`, the scanner reports an unidentified licence, and a reader who wanted one glance instead gets
700 lines to read. The most useful thing a licence file can do is be recognised on sight, which costs exactly one
discipline: nothing of ours goes in it.

`NOTICE` narrows nothing. It is where this work meets that text, not a second instrument, and
[`cli/tests/license_of_record.rs`](../cli/tests/license_of_record.rs) holds both files to that shape — `LICENSE` bounded
at both ends by the FSF's own lines, so there is nowhere in it for an added clause to sit.

## Who holds what

Three facts, two organizations, and the one a fork actually needs to get right is the second row.

| Held | By | Which is why |
| --- | --- | --- |
| Copyright in this repository | **Shook Law PLLC**, the law firm | Only the holder can grant the whole work |
| An irrevocable right to publish it under the AGPL | **Neon Law Foundation** | It outlives the Firm's owners |
| **NEON LAW**, U.S. Reg. No. 6,325,650 | **Shook Law PLLC**, the law firm | The mark is not licensed here at all |

**Why the Firm holds it.** The Firm writes this software, engages the people who write the rest of it, and operates a
legal practice on it under the NEON LAW mark. A mark on legal services is how a client identifies who is accountable for
their legal work, and that accountability belongs to the entity holding the bar licence — so the mark was always going
to be the Firm's, and putting the copyright in the same hands is what lets one signature grant the whole work.

**Why the second row is load-bearing.** A copyright holder can normally stop publishing under a licence whenever it
likes. Every copy already given keeps its rights — a licence granted cannot be revoked — but nothing obliges the holder
to offer another, which is how a project gets relicensed out from under the people building on it. The right the **Neon
Law Foundation** holds removes that possibility: it is perpetual, irrevocable, non-exclusive, and royalty-free; it binds
the Firm's successors; and it survives any change of the Firm's control. So a buyer of the Firm inherits a work somebody
else is entitled to keep publishing under `AGPL-3.0-only`, and can be made to.

That is a stronger promise than putting the copyright in a charity would be. A foundation you control can relicense too.
A right held by a different organization is enforceable by that organization.

**The practical consequence** is unchanged, and it is the sentence to read if you read only one: copy it, change it,
sell it, run it for other people — none of that needs anyone's permission, though § 13 attaches an obligation to the
last one. Calling the result "Neon Law" needs the Firm's permission, and the Firm does not give it.

## Chain of title

Who wrote Navigator, and how the copyright reached the organization that holds it. This section exists because the
answer used to be asserted rather than recorded: a dozen places in this tree agreed that a different organization owned
the work, and the guard test proved they agreed with each other. Agreement is not a chain.

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

Two, doing different jobs. Neither is reproduced here and neither is quoted: what this section records is that they
exist and between whom. Their dates are held with the executed originals rather than published, though a recordation
under § 205 puts the assignment's date on the public record in its own right.

| Instrument | Parties | Status |
| --- | --- | --- |
| Assignment of copyright, 17 U.S.C. § 204(a) | Nicholas Shook and Shook Law PLLC, to Shook Law PLLC | Executed |
| Licence to publish under `AGPL-3.0-only` | Shook Law PLLC to the Neon Law Foundation | Executed |

Both assignors join the first instrument, each conveying whatever interest it holds. That is deliberate and costs one
signature block: whether the work was already the Firm's turns on an employment question nobody needs to answer, and it
stops mattering once both have conveyed.

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
this section gains a row rather than being rewritten, and the Foundation's right to publish survives it: that right
binds the Firm's successors, which is what the clause is for.

So read the holder named here as the holder of record today, and read this section — rather than any single sentence
elsewhere — as the place the answer lives.

## The grant: AGPL-3.0-only

Every line of code and every line of drafted legal prose is licensed under version 3 of the GNU Affero General Public
License ([`LICENSE`](../LICENSE)). Cargo and npm manifests declare `AGPL-3.0-only`.

**`-only`, never `-or-later`.** The terms this repository publishes under are the terms in its own licence file. A later
FSF revision may be an improvement, but a law practice does not hand a third party the ability to change the obligations
attached to the software it runs its matters on.

`deny.toml`'s allowlist is a different question. It governs what this workspace is willing to *consume*, which has
nothing to do with how the workspace is licensed out: every licence on that allowlist may be distributed inside an AGPL
work, which is the only property an inbound policy has to have.

### Section 13 is the reason

§ 13 is what makes this the Affero licence rather than the ordinary GPL, and it is the clause to read before deploying
rather than before forking. Modify Navigator, let users interact with your version remotely over a network, and you must
offer those users the corresponding source of what they are actually using. The obligation attaches to **operating** the
software, not only to shipping a copy of it.

That is not an incidental fit. Nobody downloads a legal-services platform to run it on their own desk; they run it as a
portal for clients, which is the exact act § 13 attaches to. So the clause lands on the way this software is actually
used rather than on an edge case, and it makes the exchange symmetric: anyone may run a practice on Navigator, and a
client of that practice can see the software their matter is being handled by. A firm that improves it while operating
it for clients owes those clients the source of the version they are using, on the same terms it received.

Two things it does **not** do:

- **It does not reach an unmodified deployment.** Running the software as published carries no § 13 source obligation,
  because the corresponding source is already here.
- **It does not reach client data.** § 13 obliges you to publish *your modified software*. A matter, a document, and a
  client's facts are not the software, and nothing in the licence asks for them.

### What a fork owes, in order

1. **Keep the notices.** § 4 conditions the permission to convey on handing every recipient this License along with the
   work, and on keeping the copyright notices intact — which is `LICENSE` and `NOTICE`, travelling together.
2. **Publish your changes when you convey the work.** § 5 covers conveying modified source; § 6 covers conveying a
   built binary, which must be accompanied by the corresponding source.
3. **Offer your source to the users you operate it for.** § 13, above.
4. **Rename it.** Not a copyright obligation at all — see [Trademarks](#trademarks). The brand manifest
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
workflow definitions in the same files. They are licensed `AGPL-3.0-only` with everything else, because in this tree a
template is not a document sitting near a program — it is an input to one. A notation body is parsed by the workflow
engine, validated against the `N`-family rules, and rendered; the prose, the prompts, and the state machine are the same
file, and a rule change and a clause change arrive through the same review. A licence boundary drawn inside that file
would ask a contributor to work out which half of a line they were editing, and a fork to track two obligations through
one file.

Attribution, which is what drafted prose actually needs, is already a subset of what § 4 and § 5 require: a conveyed
copy keeps its notices and a modified one says what changed.

## Why open

Neon Law charges published flat fees for consumer legal work. That economics only holds if routine matters cost very
little to run, and this software is what makes them cost little. Publishing it is the same argument as publishing the
prices: a legal system where only the well-resourced can afford counsel is not fixed by one firm being efficient in
private.

Three consequences follow, and they are the trade being made:

- **No trade-secret protection.** Anything published cannot be un-published, so no mechanism in this tree is a secret.
- **The confidentiality boundary is procedural.** A publication path exists, so the no-client-data rule is enforced by
  a load-bearing test on every pull request — see [`agent-workflows.md`](agent-workflows.md#no-client-data-in-the-repo).
- **Forks are expected, and each one answers to its own users.** Another firm running this software is the point, and
  § 13 attaches exactly there: a firm that improves Navigator while operating it for clients owes those clients the
  source of the version they are using. It owes this project nothing — no fork has to send anything here, and none is
  asked to. The brand manifest (`views::brand_bundle`) exists so a fork renames itself without patching sources.

The trademark reservation below protects the thing that actually distinguishes the practice, which is why the software
itself does not need protecting.

## Commercial licensing

The AGPL is granted by the copyright holder, so only the copyright holder can relieve anyone of it. That holder is
**Shook Law PLLC**, and this section says what follows from that — including the two things the Foundation cannot do.

**What is on offer is relief from § 13, and nothing else.** A firm that modifies Navigator and operates it for clients
owes those clients the corresponding source of its version. A firm that would rather keep its modifications to itself
can be licensed out of that obligation. Nothing else about the grant is for sale, because nothing else needs to be:
running, forking, modifying, and redistributing Navigator are already free to everyone, and no exception is required for
any of them.

**Only the Firm can grant it.** This is not a policy choice that could have gone another way. A proprietary exception is
a permission carved out of the copyright, and a permission can only be given by whoever holds the right — so the
Foundation, holding a licence to publish rather than the copyright itself, has nothing to carve from. Being
non-exclusive, its right to publish does not narrow what the Firm may separately license; being a licence rather than
title, it does not widen what the Foundation may.

**The Foundation may sublicense to legal aid organizations at cost**, including relief from § 13. That is the one
sublicensing power it has, it is bounded by who receives it and by what it may charge, and it exists because a legal aid
office running a modified Navigator for its clients should not have to choose between the source obligation and its
capacity to meet it. At cost means what it says: the programme recovers what it spends and is not a revenue line.

**The Foundation may not grant commercial exceptions.** Keeping that with the Firm is what keeps the two relationships
legible. A charity whose controlling insider's firm holds the exclusive right to monetize the charity's principal asset
is a hard arrangement to explain and a harder one to price; a charity that publishes the work and serves legal aid with
it is not making that argument at all.

**No price is published here or anywhere on the website.** A deployment's scope is not knowable in advance, so a figure
would be a floor dressed as a fee — the same reason litigation and fractional general counsel carry none while the
consumer flat fees are published in full. Write to [contact@neonlaw.org](mailto:contact@neonlaw.org).

One thing worth stating plainly, because the section invites the opposite reading: **none of this is a restriction on
the public grant.** Every right `AGPL-3.0-only` gives you, you already have, and no copy already taken can be reached by
anything in this section. A commercial licence is an *additional* permission somebody may want; the absence of one takes
nothing away.

## Trademarks

**NEON LAW** is a registered trademark, U.S. Reg. No. 6,325,650, owned by Shook Law PLLC. The licence grants rights in
copyright, not in trademarks, and [`NOTICE`](../NOTICE) says so explicitly — a reader deciding whether they may ship a
fork called "Neon Law" reads the terms that shipped with the code, so the answer has to be there rather than only in a
doc. It is also the reason `NOTICE` travels in every archive and image: the reservation is the one thing the grant does
not hand a fork.

The registrant and the copyright holder are the same organization — see [Who holds what](#who-holds-what) — and that
changes nothing, which is the point worth stating rather than assuming. A copyright licence conveys rights in copyright.
It does not reach a mark, so the Firm granting you everything it can grant under `AGPL-3.0-only` still leaves you
without the name. The same answer used to follow from the two sitting in different hands; it now follows from what a
copyright licence is, which is the sturdier reason.

This is the one reservation this project genuinely needs. A client identifies who is accountable for their legal work by
the name on the door, so a fork trading as Neon Law would misdirect the person least able to check. Anyone may run,
fork, and redistribute the software, and may say their work is built on Neon Law Navigator; nobody may present their
deployment as Neon Law.

The Neon Law Foundation uses the mark for its charitable, pro bono, and public-education work under separate written
permission from the Firm.

## Contributions

**Outside contributions are closed right now**, and anyone is welcome to write to
[contact@neonlaw.org](mailto:contact@neonlaw.org) instead. That is a capacity decision about pull requests and nothing
more: it revokes nothing, because a licence already given cannot be taken back, and every copy already cloned keeps its
rights whatever the contribution policy says.

The terms are stated anyway, so they are knowable in advance and a fork's own authors know where they stand. **A
contribution assigns to the Firm.** Every author in this repository signed a written agreement to that effect before
their first commit, so the work is the Firm's on creation, and reopening to outside contributions means a contributor
agreement on the same footing.

Assignment is the mechanism and it changes nothing about what you receive: the work reaches you under `AGPL-3.0-only`,
the same terms the project ships under, wherever in the tree it lands. What it buys is a single holder able to grant the
whole work — which is what the Foundation's irrevocable right to publish rests on, because a grant assembled from many
holders is one nobody can reliably renew, and what one contributor could not be found to re-sign would be a hole in the
public grant rather than in a private one. See [`CONTRIBUTING.md`](../CONTRIBUTING.md).

Two boundaries survive the opening, because this repository runs a live practice: **no client data ever enters the
tree**, and **a change to `templates/` gets attorney review** before it merges, however mechanical the diff looks.

## What the binary carries

An installed `navigator` is one executable that may sit far from anything it shipped beside, so its terms are compiled
into it as well as staged in the archive. The AGPL requires this rather than merely inviting it: § 4 conditions the
permission to convey on handing every recipient a copy of this License along with the work. A bare executable someone
was given is a copy — and under § 13 its holder may owe the source onward in turn, which nobody honours from terms they
were never shown.

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

- `LABEL org.opencontainers.image.licenses="AGPL-3.0-only"`, which Artifact Registry and GHCR read for the package
  page — what a reader sees *before* pulling.
- `LICENSE` and `NOTICE` staged at `/app` beside the binary — what a running container can be made to show.

`Containerfile.runner` is exempt: it is the CI runner image rather than a published artifact of the software.

### Where the images are published

Every product image goes to `ghcr.io/neon-law-foundation`, from `publish-service` and `publish-triggers`. There is no
toggle and no second registry: the push is unconditional, and `cli/tests/license_of_record.rs` asserts that no condition
guards it. A registry the release depends on must not be switchable by a repository variable, whose absence is a
settings edit that touches no file, passes every gate, and yields a release that looks fine until someone checks the
registry days later.

No credential to create. The repository and its Actions live on github.com, whose own registry is `ghcr.io`, so the push
authenticates with `GITHUB_TOKEN` and the `packages: write` scope. That scope is granted on those two jobs rather than
at the top of the workflow — every other job in the file checks out and builds release code, and none of them has any
business writing packages.

**A GHCR package inherits its linked repository's visibility.** `neon-law-foundation/navigator` is public, so these are
public packages — anyone can pull the same digests the deployments run, which is the point of publishing them.

## Releases

Release archives carry `LICENSE` and `NOTICE` so an unpacked archive states its own terms without the repository tree,
and the binary prints both, plus the third-party notices, itself. Someone who ends up with nothing but the executable
can still read the terms they are running under and the attributions it is obliged to carry.
