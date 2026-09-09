# GKE ship — the roll-only model

CI (`.github/workflows/deploy.yml`) builds every image and publishes it to **GHCR** at `ghcr.io/neon-law-source-code`
(`cli::devx::registry::DEFAULT_REGISTRY`; a fork overrides it with `NAVIGATOR_IMAGE_REGISTRY`), tagged with the release
version — `YY.M.D`, optionally carrying a prerelease such as `-rc.N` or `-hotfix.N` — plus `latest`. `ship` only **rolls
the cluster** onto an already-published image.

The reconcile/render mechanics (the CLI-embedded manifest tree, the unconditional `kubectl apply -k`, the
placeholder→coordinate table) are owned by [`gke-prod.md`](../gke-prod.md#manifest-delivery); the operational roll
recipe (tag resolution, the Secret-invariant check, the concurrent rollout, the Restate re-registration, and the
secret-rotation "no-rebuild push") is owned by [`cloud-operations.md`](../cloud-operations.md). This page is only a
short worked example.

## The new model in one breath

CI (`deploy.yml`) publishes release-versioned images to GHCR under `ghcr.io/neon-law-source-code`; `ship` **rolls** the
cluster onto an already-published tag and builds nothing. It takes a **required** `--deployment` naming a directory
under `deployments/` and a **required** `--tag` naming a published release version exactly — `26.9.8`, or a prerelease
such as `26.9.9-rc.1` or `26.9.8-hotfix.1`. The tag is semver, so the legacy four-component `YY.M.D.H` spelling is
refused (`registry::validate_release_tag`); a second release on one day is a `-hotfix.N` prerelease, per
[`gitops.md`](../gitops.md#releasing-twice-in-one-day). `ship` confirms the deployment's Secret satisfies the new
binary's boot invariants, and pins `navigator-web` and `workflows-service` to that one tag together — never roll one
alone. Verify the roll with `GET https://www.<your-domain>/version`, whose `release` field is the version now live. See
[`gke-prod.md`](../gke-prod.md#manifest-delivery) for why the reconcile is unconditional and how the manifest tree is
rendered.

## The fast path

```bash
# Roll one deployment onto a named published YY.M.D image (service deployments, together).
# --deployment and --tag are required; ship never guesses either.
navigator ops ship --deployment <row> --deployments-dir . --tag 26.6.23

# Print every command, run nothing.
navigator ops ship --deployment <row> --deployments-dir . --tag 26.6.23 --dry-run

# No-rebuild push: restart service deployments so they re-read a rotated Secret value (no --tag needed).
navigator ops ship --deployment <row> --deployments-dir . --restart-only

# Under a no-IAM-changes rule: assert the web GSA's self-signing binding rather than granting it. An
# absent binding stops the roll and prints the `gcloud` command for whoever holds `setIamPolicy`.
navigator ops ship --deployment <row> --deployments-dir . --tag 26.6.23 --assert-signing-iam
```

Configuration is read from the repository's `deployments/<name>/config.toml` — the GCP project / region / cluster for
the kubectl context, the public host for the smoke check, the registry hub for the image references. Nothing is
hard-coded, and nothing comes from the process environment: a stale shell cannot select the wrong deployment. See
[`deployment-secrets.md`](../deployment-secrets.md) for the tree and [`cloud-operations.md`](../cloud-operations.md) for
the manual `kubectl` fallback.
