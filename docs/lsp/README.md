# Navigator editor integration

`navigator-lsp` brings Navigator's Markdown and Notation diagnostics to any editor that supports the Language Server
Protocol. It provides the same rule results and safe fixes as `navigator validate`, over local JSON-RPC with no
telemetry.

It serves attorneys and engineers who review or edit repository Markdown outside the web editor. The integration is
necessary so local feedback, pull-request review, and CI enforce one rulebook rather than discovering structural errors
at different stages.

Install the `navigator-lsp` binary, then configure the editor to launch it for Markdown files. The server contract lives
in the [`lsp` crate](../../lsp/README.md); editor-specific configuration belongs in the editor or extension.

## Where to get the binary

Every tagged release attaches a `navigator-lsp-<tag>-<platform>` archive to its [GitHub
Release](https://github.com/neon-law-source-code/navigator/releases), alongside the `navigator` CLI archives —
`navigator-lsp-<tag>-linux.tar.gz`, `-macos.tar.gz`, and `-windows.zip`. Each carries the executable beside `LICENSE`.
This is the source an editor extension should resolve against, since it is a per-version asset on an immutable tag;
`navigator ops lsp publish` also mirrors the binaries to the site's public assets bucket at a "latest" key for the
site's own use, which is not a stable target for an extension pinned to a release.

`navigator-lsp` is BUSL-1.1, like the rest of the workspace — the same licence the `navigator` CLI archives already
carry — but the licence restricts production *use*, not distribution, so the archives are downloadable by anyone. An
editor extension repository (a Zed marketplace listing, for example) needs its own accepted licence only for the
extension's own code; the language server it downloads and runs is not held to that requirement.
