---
name: pr-image-upload
description: >
  Embed a local screenshot or GIF into a `github.com` PR (or issue) body so it actually RENDERS, driven from the
  CLI — no drag-drop, no committing the file, no image-hosting branch, no release/tag. One `curl` uploads the `/tmp`
  capture to the tenant's `user-attachments` store using nothing but `gh auth token`, and returns a real
  `https://github.com/user-attachments/assets/…` URL to drop into the body. Trigger as the embed half of
  [[create-pr]] Step 6 (after [[web-preview]] captures the visual), when a reviewer comment on a PR asks for a "live
  walkthrough"/screenshot during [[review-pr]], or any time you have a `/tmp` image that must appear in a PR/issue body
  or comment. Capture lives in [[web-preview]] — a walkthrough defaults to a GIF of the real interaction (§5), a still
  (§3) only for a genuinely static change; this skill only hosts + embeds it.
---

# Embedding screenshots in a PR body from the CLI

An `<img src="/tmp/…">` in a `gh`-created body renders **broken** (the host resolves it to
`https://github.com/tmp/…` → 404), and the clean hosting options are all off the table per `CLAUDE.md`: don't
commit the capture to the tree, don't push an image-hosting branch, don't cut a release/tag just to host a PNG.

The path that satisfies all of that is the tenant's own **user-attachments** store, reached with a single authenticated
request. It needs an OAuth token with push access to the target repository — exactly what `gh auth token` returns.

## Do not reach for the `gh-image` extension

`drogers0/gh-image` is a **dead end on this host** and no amount of flags fixes it. It hardcodes github.com throughout —
its own error string is `no github.com user_session cookie found in any supported browser` — and it has no host
override. Its whole purpose is to replay a 3-step *browser-cookie* upload flow, which the token route below skips
entirely. Symptoms, if someone tries anyway:

| Invocation | Failure |
| --- | --- |
| no flag | `could not parse GitHub owner/repo from remote URL: https://github.com/…` |
| `--repo <a github.com repo>` | `failed to look up repo ID …` — there is no github.com `gh` auth here |
| `--repo neon-law-source-code/navigator` | `step 0 (get upload token): repo page returned 404` — not a github.com repo |

## The recipe

```bash
# 1. Capture to /tmp first — never into the repo tree. Default to a GIF of the real interaction
#    (web-preview §5); a still (§3) only for a genuinely static change.
# 2. Look at it yourself: Read the PNG/GIF so it renders inline, and confirm it shows the change.
# 3. Upload. Keep the token out of `ps` by passing it through a curl config file, not `-H` in argv.
REPO_ID=$(gh api repos/neon-law-source-code/navigator --jq .id)
umask 077
gh auth token | sed 's/.*/header = "Authorization: Bearer &"/' > /tmp/.curlcfg
URL=$(curl -sS -K /tmp/.curlcfg -X POST \
  -H "Accept: application/json" -H "Content-Type: image/gif" \
  --data-binary @/tmp/navigator-screenshots/walkthrough.gif \
  "https://uploads.github.com/user-attachments/assets?name=walkthrough.gif&content_type=image%2Fgif&repository_id=$REPO_ID" \
  | jq -r .url)
rm -f /tmp/.curlcfg

# 4a. New PR: reference $URL in the body you pass to `gh pr create` (an <img> tag or ![alt]($URL)).
# 4b. Existing PR: splice it into the current body and update.
gh pr view <N> --json body --jq .body > /tmp/body.md
printf '\n\n## Walkthrough\n\n<img alt="…" src="%s" />\n' "$URL" >> /tmp/body.md
gh pr edit <N> --body-file /tmp/body.md
```

A `201` returns `{"url":"https://github.com/user-attachments/assets/<uuid>"}` — that is the only field.

Upload **all** of a PR's images in one pass, then do a single `gh pr edit`, so the body isn't rewritten N times.

## Verify the render — not the asset URL

**Do not verify by curling the asset URL with the token.** An OAuth token is not a web session, so the request follows a
redirect and hands back ~39 KB of `text/html`. That looks like a broken upload and is not one.

Ask the host's own renderer instead. A working asset resolves to an `<img>` on `objects-origin.github.com` with a
signed `X-Amz-Signature` and `response-content-type=image/…`:

```bash
gh api /markdown -X POST -f mode=gfm -f context=neon-law-source-code/navigator \
  -f text="![probe]($URL)" | head -3
```

## Rules and caveats

- **Images and video only.** Anything else returns `422 content_type is not included in the list of allowed content
  types`. That file needs the 3-step browser-cookie flow, which has **no token equivalent** on this host — deliver it to
  the user with `SendUserFile` and say why it isn't embedded.
- **URL-encode `content_type`.** A bare `+` in `image/svg+xml` arrives as a space and is rejected.
- **The uploads host is `uploads.github.com`** — the tenant subdomain, not `uploads.github.com`. It resolves to
  the same tenant address as the web host.
- **Assets inherit repository visibility.** `neon-law-source-code/navigator` is `internal`, so the image renders for tenant members
  and 302s for an anonymous fetch. That 302 is correct behaviour, not a failure.
- The token needs **push access** to `repository_id`; a `404` from the upload endpoint means the token lacks it (or the
  parameter is missing), not that the URL is wrong.
- **Never** commit the capture, push an image-hosting branch, or create a release/tag to host it. (See [[web-preview]]
  §6 and `CLAUDE.md`.)

## When a review comment asks for a walkthrough

A common [[review-pr]] finding (e.g. "missing live walkthrough artifact") is satisfied here: capture the changed states
([[web-preview]]), embed them in the PR body with this skill, then **reply to the thread and resolve it** ([[review-pr]]
Step 8) noting what the capture shows. Embedding the image is the fix; the reply + resolve closes it.
