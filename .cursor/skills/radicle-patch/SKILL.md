---
name: radicle-patch
description: >
  Open a Radicle patch for sleek. Cloud / GitHub agents must use the
  boxci-github-patch skill (POST boxci /api/patches/from-github) — not the
  radicle MCP and not local rad auth. Use this skill only as a pointer to
  boxci-github-patch, or when a machine already has RADICLE_SECRET_KEY and
  a working rad remote for a direct git push rad.
---

# Radicle patches (sleek)

## Cloud / Cursor agents (default)

**Do not use the radicle MCP** (`create_patch` / `issue_device_key` on
`mcp.boxd.sh`). Hosted MCP cannot see the agent git checkout.

**Do not** run `rad auth` to mint a device DID in the cloud VM.

Follow **`.cursor/skills/boxci-github-patch/`** instead:

1. Push the commit to GitHub (`origin`).
2. `POST https://boxci.boxd.sh/api/patches/from-github` with sleek’s RID,
   `https://github.com/codegod100/sleek.git`, and the commit SHA.
3. Poll the boxci run and surface `https://boxci.boxd.sh/runs/<run_id>` plus
   `patch_id` from the step output.

Sleek defaults:

| Field | Value |
|-------|--------|
| `repo` | `rad:z9mjPzpVK472QXaaP1picc5U9xBR` |
| `github_repo_url` | `https://github.com/codegod100/sleek.git` |
| `branch` | `main` |

## Local / CI exception

Only when `RADICLE_SECRET_KEY` (or an existing `$RAD_HOME`) is already loaded
and `git remote get-url rad` works — e.g. a CI job that pulled the key from
OpenBao `secret/data/radicle` — you may open a patch with:

```bash
git push rad HEAD:refs/patches \
  -o patch.message="Title" \
  -o patch.message="Body paragraph"
```

Never commit key material under `.radicle/` or elsewhere.
