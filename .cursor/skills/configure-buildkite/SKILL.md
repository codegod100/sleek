---
name: configure-buildkite
description: >
  Configure Buildkite for nandi org repos using the baogui reference setup.
  Use when the user asks to set up Buildkite, create a pipeline, add
  .buildkite/pipeline.yml, configure cluster secrets, auth the Buildkite API
  from OpenBao, or mirror baogui CI (check / Flatpak / APK). Also use when
  BUILDKITE_API_KEY, bk CLI, or buildkite-agent secrets are mentioned.
---

# Configure Buildkite (baogui reference)

Known-good Buildkite layout for this org comes from **baogui**
([pipeline](https://buildkite.com/nandi/baogui-aopjch),
`.buildkite/pipeline.yml` in `codegod100/baogui`). Reuse these defaults unless
the user overrides them.

## Org defaults

| Item | Value |
|------|--------|
| Organization | `nandi` |
| Cluster | Default cluster (`4e9dc42a-d344-4956-83bb-9091dfe0127a`) |
| Hosted queue | `auto` (`LINUX_AMD64_2X4`) |
| Reference pipeline | `baogui-aopjch` |
| Secrets UI | https://buildkite.com/organizations/nandi/clusters → Default cluster → Secrets |
| API token in OpenBao | `BUILDKITE_API_KEY` under `secret/data/ai-api-keys` (token description `bao`) |

Auth for agents / scripts:

```bash
./scripts/configure-buildkite-from-openbao.sh
# exports BUILDKITE_API_TOKEN (and BUILDKITE_API_KEY) for this shell
```

Or:

```bash
eval "$(./scripts/fetch-openbao-env.sh --export --keys BUILDKITE_API_KEY)"
export BUILDKITE_API_TOKEN="$BUILDKITE_API_KEY"
```

Never print the token. Prefer REST with `Authorization: Bearer $BUILDKITE_API_TOKEN`.

## Create a pipeline (REST)

Pipeline YAML lives in the repo. The Buildkite pipeline step is only:

```yaml
steps:
  - command: buildkite-agent pipeline upload
```

Create via API (require `write_pipelines`):

```bash
org=nandi
cluster_id=4e9dc42a-d344-4956-83bb-9091dfe0127a
name="my-repo"          # human name / slug base
repository="https://github.com/codegod100/my-repo.git"  # or Radicle garden URL
default_branch=main

payload=$(jq -n \
  --arg name "$name" \
  --arg repository "$repository" \
  --arg cluster_id "$cluster_id" \
  --arg default_branch "$default_branch" '{
  name: $name,
  cluster_id: $cluster_id,
  repository: $repository,
  default_branch: $default_branch,
  configuration: "steps:\n  - command: buildkite-agent pipeline upload\n"
}')

curl -sS --fail-with-body -X POST \
  -H "Authorization: Bearer $BUILDKITE_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d "$payload" \
  "https://api.buildkite.com/v2/organizations/$org/pipelines" \
  | jq '{slug, web_url, repository, cluster_id}'
```

If `bk` is installed: `bk pipeline create "$name" -r "$repository" --cluster-uuid "$cluster_id"`.

baogui and sleek use **Radicle Garden** clone URLs (not GitHub) so Buildkite
pulls the same tip / patches as Radicle:

| Repo | RID | Garden git URL |
|------|-----|----------------|
| baogui | `rad:zWrYATFmb1jp9HN2DFxYG5AopJcH` | `https://nandi.radicle.garden/zWrYATFmb1jp9HN2DFxYG5AopJcH.git` |
| sleek | `rad:z9mjPzpVK472QXaaP1picc5U9xBR` | `https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git` |

Provider is `private` (no GitHub webhooks). Trigger builds via API, schedule, or
the Radicle CI adapter. Agents must also reach sibling path-dep remotes (e.g.
vidya on tangled.org / GitHub).

## Repo files to add

1. **`.buildkite/pipeline.yml`** — real steps (see `references/baogui-patterns.md` and `assets/pipeline.template.yml`).
2. Keep using **`scripts/fetch-openbao-env.sh`** when jobs need OpenBao-backed tokens.
3. Document the pipeline slug + cluster secrets in `AGENTS.md` / README like baogui.

## Cluster secrets

Put secrets on the **cluster**, not in YAML:

| Key | Purpose |
|-----|---------|
| `NIXBUILD_TOKEN` | Preferred for APK / nixbuild remote builds |
| `OPENBAO_TOKEN` | Optional; job can `fetch-openbao-env.sh --keys NIXBUILD_TOKEN` |

Soft-load inside the job (missing secret → skip, do not fail upload):

```bash
if command -v buildkite-agent >/dev/null 2>&1; then
  if [[ -z "${NIXBUILD_TOKEN:-}" ]]; then
    if nt="$(buildkite-agent secret get NIXBUILD_TOKEN 2>/dev/null || true)"; then
      [[ -n "$nt" ]] && export NIXBUILD_TOKEN="$nt"
    fi
  fi
fi
```

Do **not** invent tokens — copy from OpenBao (`secret/data/ai-api-keys`) or nixbuild. Scope secrets to the pipeline slug when possible.

## Pipeline YAML conventions (from baogui)

- Top-level `agents: { queue: auto }`.
- Escape shell vars as `$$VAR` / `$${VAR:-}` so Buildkite does not interpolate at upload time.
- Prefer soft secret get over YAML `secrets:` (missing key fails the job).
- Do **not** upload a naked Linux ELF for GUI apps — Flatpak / nix / APK only.
- Flatpak on hosted agents: privileged Docker image `ghcr.io/flathub-infra/flatpak-github-actions:freedesktop-25.08` (host `bwrap` cannot `pivot_root`).
- APK: configure nixbuild SSH + trusted keys, then `nix build` on `ssh-ng://nixbuild` and `nix copy` back; upload with `buildkite-agent artifact upload`.
- Author email must match a verified Buildkite personal email (or GitHub-linked noreply).

## Checklist when configuring a new repo

1. Auth via OpenBao (`configure-buildkite-from-openbao.sh`).
2. Confirm org/cluster/queue (`GET /organizations/nandi/clusters`).
3. Add `.buildkite/pipeline.yml` adapted from the template.
4. Create pipeline with upload-only `configuration` + `cluster_id`.
5. Ensure cluster secrets exist (`NIXBUILD_TOKEN` and/or `OPENBAO_TOKEN`).
6. Trigger a build and confirm check + optional artifact steps.
7. Link pipeline URL in README / AGENTS.md.

## Related skills

For generic Buildkite YAML/API/CLI depth, install official skills:

```bash
npx skills add buildkite/skills
# or: bk skill add buildkite-pipelines --agent cursor
```

This skill owns **nandi/baogui-shaped** org wiring; official skills own generic Buildkite mechanics.
