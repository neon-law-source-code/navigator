---
kind: post
title: Going All-In on Rust
description: Why a general practice firm builds its own tools in one language for litigation and transactional work.
---

![Ferris trains at sunrise on stone steps.](img/going-all-in-on-rust/ferris-training-hero.png)

_Nick is a partner at Neon Law._

Most of our work arrives the way yours probably does: on a Tuesday, from someone who did not plan their week around it.
A founder forwards a contract at eleven at night because the customer wants it signed by Thursday. A client gets served
and reads the caption four times before calling anyone. That is the job. We are a general practice firm — we litigate,
and we do transactional work quickly — and both halves live or die on how fast a careful person can turn something
around.

So we build our own tools. Not because we are trying to be a software company, but because the alternative is waiting on
someone else's roadmap to fix the thing that is slowing down a client's Thursday.

For Neon Law Navigator, the tool we build, the language is Rust.

The reason is simple: Rust can work almost anywhere. The same language can power a Windows laptop, a Linux server, a
Mac, a cloud container, a local dev box, a command-line validator, a web service, and whatever comes next. We are a
small firm. One stack means fewer handoffs and fewer ways for a small team to lose the thread — which matters more than
fashion when the person waiting on you is a client.

Rust also gives us a governance story we trust. The language is stewarded by an independent nonprofit, not by one vendor
whose incentives may change later. Its community made the boring, generous choice early: permissive open-source
licensing, independent stewardship, and a culture that treats the ecosystem as bigger than any one company. When you are
going to build the tools your practice depends on, who controls the foundation underneath them is a real question, and
we would rather answer it once.

Navigator now sits the same way, and we would rather say so plainly. The firm holds the copyright, operates the
software, and publishes it. It is source-available under the Business Source License 1.1, templates included: read it,
build it, fork it, and use it outside production with no permission to ask for. Running it to deliver legal services to
other people needs a commercial licence from us, and every version we publish converts to the GNU Affero General Public
License v3 four years later.

Navigator ships as a command-line tool, an editor plugin, and a web service. That is deliberate. You should be able to
try the whole system locally without a cloud subscription. You should be able to draft legal documents in an editor,
with the same diagnostics, squiggly lines, and fixable errors a developer gets. A legal workflow should be something a
person can run, inspect, and improve without needing five runtimes or a perfect Internet connection.

Here is the part that shows up on the client's side. Our transactional practice commits to returning a standard master
services agreement — the one contract that sets payment, liability, IP ownership, and confidentiality between you and a
customer once, so the next ten deals are short order forms instead of fresh negotiations — within four business hours of
a complete intake. That is a commitment about our own turnaround. It is not a promise about how fast your counterparty
moves, and it is not a promise about whether the deal closes. We can make it because the drafting, the validation, and
the attorney review all run in one system we control, and because a compiler catches a whole category of mistakes before
a person has to.

Rust can be tough to learn and tough to write. It fights us regularly, and we do not think that is a reason to avoid it,
especially in a world where agents can write more of the first draft. The better question is what the human should
review. We would rather review types, data structures, tests, and compiler errors than sift through a pile of code that
only works because the happy path happened to run once. On a document that becomes binding, "it ran once" is not a
standard.

Linus Torvalds put the data point sharply in a [2006 Git mailing-list post mirrored by
LWN](https://lwn.net/Articles/193245/): "Bad programmers worry about the code. Good programmers worry about data
structures and their relationships." Rich Hickey, the creator of Clojure, makes the complementary point in Clojure's
official essay on [values, identity, and state](https://clojure.org/about/state): an identity is a stable logical thing
associated with different values over time. A matter is exactly that — the same file, different facts, month over month.

Start with the data. Understand how it changes over time. Write the behavior down as tests first, in plain English
before anything else. Then let Rust carry those steps into the system that drafts documents, validates them, routes them
through attorney review, and produces something a person can actually use.

That is the bet. Divorce, caregiving, debt, a contract dispute, forming the company, being sued over it: people reach
for a lawyer when life is already heavy. The software should open quickly, explain itself, and keep working close to the
machine in front of them — because the person on the other end of it is having a worse week than we are.

We feel great about Rust because we feel great about the ecosystem around Rust. We talk about this work at the Rust NYC
meetup, and about a larger hope: that lawyers who understand software can build and maintain more of their own tools.
Not because software engineers do not matter, but because the Rust ecosystem now carries enough of the serious machinery
that the old split between "lawyer" and "software developer" can get a little less rigid.

Anyone can vibe code these days. Going all-in on Rust is how we make that power inspectable, local-first, and durable
enough for legal work — and how a small firm keeps its promises about turnaround.
