# GKE ship — the roll-only model

CI (`.github/workflows/deploy.yml`) builds every image and publishes it to the **private** Google Artifact Registry
`YOUR_GCP_REGION-docker.pkg.dev/YOUR_IMAGES_PROJECT_ID/navigator/navigator-*`, tagged `YY.M.D` (the release date) plus
`latest`. `ship` only **rolls the cluster** onto an already-published image.

The reconcile/render mechanics (the CLI-embedded manifest tree, the unconditional `kubectl apply -k`, the
placeholder→coordinate table) are owned by [`gke-prod.md`](../gke-prod.md#manifest-delivery); the operational roll
recipe (tag resolution, the Secret-invariant check, the concurrent rollout, the Restate re-registration, and the
secret-rotation "no-rebuild push") is owned by [`cloud-operations.md`](../cloud-operations.md). This page is only a
short worked example.

## The new model in one breath

CI (`deploy.yml`) publishes dated `YY.M.D` images to the private Google Artifact Registry
`YOUR_GCP_REGION-docker.pkg.dev/YOUR_IMAGES_PROJECT_ID/navigator/navigator-*`; `ship` **rolls** the cluster onto an
already-published tag and builds nothing. It takes a **required** `--deployment` naming a directory under `deployments/`
and a **required** `--tag` (with an optional `.H` suffix — e.g. `26.6.25.14` — for an ad-hoc same-day release), confirms
the deployment's Secret satisfies the new binary's boot invariants, and pins `navigator-web` and `workflows-service` to
that one tag together — never roll one alone. Verify the roll with `GET https://www.<your-domain>/version`, whose
`release` field is the `YY.M.D` tag now live. See [`gke-prod.md`](../gke-prod.md#manifest-delivery) for why the
reconcile is unconditional and how the manifest tree is rendered.

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
