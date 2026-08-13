---
name: boxci-github-patch
description: >-
  Ask boxci to open a Radicle patch from a GitHub commit (cherry-pick +
  git push rad HEAD:refs/patches). Use when a Cursor cloud agent (or any
  agent) should publish GitHub work onto Radicle via boxci, or when the
  user mentions boxci from-github, github-commit → patch, or
  /api/patches/from-github.
---

# boxci: GitHub commit → Radicle patch

Cloud agents often finish work on a **GitHub** clone. To land that commit on
**Radicle**, call boxci — do **not** try to `git push rad` from the cloud VM
unless Radicle is already set up there. Prefer this over radicle MCP
`create_patch` or local `git push rad` on cloud VMs.

Base URL (default): `https://boxci.boxd.sh`

Sleek defaults:

| Field | Value |
|-------|--------|
| `repo` | `rad:z9mjPzpVK472QXaaP1picc5U9xBR` |
| `github_repo_url` | `https://github.com/codegod100/sleek.git` |

## When to use

- User (or task) has a GitHub commit SHA that should become a Radicle patch
- You know the target Radicle RID (`rad:z…`) or Garden clone URL
- You know the GitHub repo URL for that commit

## Call

`POST /api/patches/from-github` with JSON:

```json
{
  "repo": "rad:z9mjPzpVK472QXaaP1picc5U9xBR",
  "github_repo_url": "https://github.com/ORG/REPO.git",
  "github_commit": "FULL_OR_PREFIX_SHA",
  "branch": "main",
  "title": "optional",
  "description": "optional body"
}
```

| Field | Required | Notes |
|-------|----------|--------|
| `repo` or `repo_url` | yes | `rad:…` / RID, or `https://…radicle.garden/<rid>.git` |
| `github_repo_url` | yes | `https://github.com/org/repo.git` |
| `github_commit` | yes | also accepts `sha` / `commit` |
| `branch` | no | Radicle base branch (default `main`) |
| `title` / `description` | no | defaults from the GitHub commit message |
| `dry_run` | no | cherry-pick only; skip patch push |
| `sync` | no | if true, wait for completion (default is async `202`) |

Auth: if the deployment sets `BOXCI_WEBHOOK_SECRET`, send header
`X-Boxci-Secret: <secret>`. Otherwise no secret.

Equivalent: `POST /api/runs/from-repo` with
`"trigger":"github-commit"` plus the same `github_*` fields and `repo_url`.

### Example (curl)

```bash
curl -sS -X POST https://boxci.boxd.sh/api/patches/from-github \
  -H 'Content-Type: application/json' \
  -d "{
    \"repo\": \"rad:z9mjPzpVK472QXaaP1picc5U9xBR\",
    \"github_repo_url\": \"https://github.com/ORG/REPO.git\",
    \"github_commit\": \"${GITHUB_SHA}\",
    \"title\": \"Import from GitHub\",
    \"description\": \"Opened by Cursor cloud agent\"
  }"
```

### Example (python)

```python
import json, os, urllib.request

payload = {
    "repo": "rad:z9mjPzpVK472QXaaP1picc5U9xBR",
    "github_repo_url": "https://github.com/ORG/REPO.git",
    "github_commit": os.environ["GITHUB_SHA"],
}
req = urllib.request.Request(
    "https://boxci.boxd.sh/api/patches/from-github",
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json"},
    method="POST",
)
# optional: headers["X-Boxci-Secret"] = os.environ["BOXCI_WEBHOOK_SECRET"]
with urllib.request.urlopen(req, timeout=120) as resp:
    run = json.load(resp)
print(run["id"], run["status"])
```

## Poll for `patch_id` / `patch_url`

Default response is **async** (`202`, `status: "running"`). Poll until finished:

```bash
curl -sS "https://boxci.boxd.sh/api/runs/<run_id>"
```

On success the run JSON includes structured fields (preferred):

```json
{
  "id": "<run_id>",
  "status": "passed",
  "patch_id": "<40-char-id>",
  "patch_url": "https://radicle.network/nodes/nandi.radicle.garden/rad:…/patches/<id>"
}
```

If those fields are absent (older boxci), the `github-patch` step `output_tail` still
contains:

```text
patch_id=<40-char-id>
```

Build the explorer URL yourself as:

```text
https://radicle.network/nodes/nandi.radicle.garden/<rid>/patches/<patch_id>
```

(`<rid>` is `RADICLE_RID` / `BOXCI_REPO_ID` from `env`, e.g. `rad:z2QL7…`.)

**Surface to the user (in this order):**

1. **The patch URL** — `patch_url` from the run (or the constructed explorer link).
   This is the review artifact — lead with it.
2. **The boxci run link** — `https://boxci.boxd.sh/runs/<run_id>` (or
   `…/repos/<slug>/runs/<run_id>`) as supporting CI context.

  **Do not** just copy the URL `output_tail` prints after `✓ Synced with N
  seed(s)` — the boxci host's `rad` has a misconfigured explorer template that
  glues the wrong domain onto the `/nodes/<seed>/...` path (e.g.
  `https://nandi.radicle.garden/nodes/rosa.radicle.network/rad:.../patches/...`),
  which 404s. Same RID/patch-id, different (working) host — prefer
  `radicle.network` as above, or the run's `patch_url` field.

## What boxci does

1. Checks out the Radicle (Garden) repo at `branch`
2. Bootstraps the CI Radicle identity + `rad` remote
3. `git fetch` the GitHub SHA, `cherry-pick` onto `github/<shortsha>`
4. `git push rad HEAD:refs/patches` with title/body

Conflicts or missing commit parents fail the run — fix on GitHub and retry, or
open the patch manually with the `rad-patch` skill.

## Do not

- Do not invent a different boxci URL unless the user gives one
- Do not put GitHub tokens in the JSON body; private fetches use VM `GITHUB_TOKEN`
- Do not confuse this with the issue → cursor-agent flow (`trigger: issue`)
