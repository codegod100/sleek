---
name: boxci-github-patch
description: >-
  Ask boxci to open a Radicle patch from a GitHub commit (cherry-pick +
  git push rad HEAD:refs/patches). Use when a Cursor cloud agent (or any
  agent) should publish GitHub work onto Radicle via boxci, when opening or
  updating a code review proposal (PR/MR/patch) for this repo, or when the
  user mentions boxci from-github, github-commit → patch, or
  /api/patches/from-github. Prefer this over radicle MCP create_patch or
  local git push rad on cloud VMs.
---

# boxci: GitHub commit → Radicle patch

Cloud agents finish work on a **GitHub** clone. To land that commit on
**Radicle**, call boxci — do **not** try to `git push rad` from the cloud VM
unless Radicle is already set up there, and do **not** use radicle MCP
`create_patch` as the primary path for sleek.

Base URL (default): `https://boxci.boxd.sh`

Sleek defaults:

| Field | Value |
|-------|--------|
| RID | `rad:z9mjPzpVK472QXaaP1picc5U9xBR` |
| Garden git | `https://nandi.radicle.garden/z9mjPzpVK472QXaaP1picc5U9xBR.git` |
| GitHub | `https://github.com/codegod100/sleek.git` |

## When to use

- User (or task) has a GitHub commit SHA that should become a Radicle patch
- Opening a code review proposal for sleek (prefer Radicle patch via boxci)
- User mentions boxci from-github / `/api/patches/from-github`

## Prerequisites

1. Push the branch/commit to GitHub (`origin`) first so boxci can fetch it.
2. Know the commit SHA (`git rev-parse HEAD`).

## Call

`POST /api/patches/from-github` with JSON:

```json
{
  "repo": "rad:z9mjPzpVK472QXaaP1picc5U9xBR",
  "github_repo_url": "https://github.com/codegod100/sleek.git",
  "github_commit": "FULL_OR_PREFIX_SHA",
  "branch": "main",
  "title": "optional",
  "description": "optional body"
}
```

| Field | Required | Notes |
|-------|----------|--------|
| `repo` or `repo_url` | yes | `rad:…` / RID, or Garden HTTPS URL |
| `github_repo_url` | yes | `https://github.com/codegod100/sleek.git` |
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
SHA="$(git rev-parse HEAD)"
curl -sS -X POST https://boxci.boxd.sh/api/patches/from-github \
  -H 'Content-Type: application/json' \
  -d "{
    \"repo\": \"rad:z9mjPzpVK472QXaaP1picc5U9xBR\",
    \"github_repo_url\": \"https://github.com/codegod100/sleek.git\",
    \"github_commit\": \"${SHA}\",
    \"title\": \"Make nix run enter the flake devShell before cargo\",
    \"description\": \"Opened by Cursor cloud agent via boxci-github-patch\"
  }"
```

### Example (python)

```python
import json, subprocess, urllib.request

sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
payload = {
    "repo": "rad:z9mjPzpVK472QXaaP1picc5U9xBR",
    "github_repo_url": "https://github.com/codegod100/sleek.git",
    "github_commit": sha,
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

## Poll for `patch_id`

Default response is **async** (`202`, `status: "running"`). Poll until finished:

```bash
curl -sS "https://boxci.boxd.sh/api/runs/<run_id>"
```

On success, the `github-patch` step `output_tail` contains a line:

```text
patch_id=<40-char-id>
```

Surface that id and a Garden link:

`https://nandi.radicle.garden/rad:z9mjPzpVK472QXaaP1picc5U9xBR/patches/<patch_id>`

## What boxci does

1. Checks out the Radicle (Garden) repo at `branch`
2. Bootstraps the CI Radicle identity + `rad` remote
3. `git fetch` the GitHub SHA, `cherry-pick` onto `github/<shortsha>`
4. `git push rad HEAD:refs/patches` with title/body

Conflicts or missing commit parents fail the run — fix on GitHub and retry.

## Do not

- Do not invent a different boxci URL unless the user gives one
- Do not put GitHub tokens in the JSON body; private fetches use VM `GITHUB_TOKEN`
- Do not confuse this with the issue → cursor-agent flow (`trigger: issue`)
- Do not prefer radicle MCP `create_patch` or local `rad auth` for cloud-agent patches on sleek
