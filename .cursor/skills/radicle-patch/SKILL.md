---
name: radicle-patch
description: >
  Open or update a Radicle patch with a stable per-environment device identity.
  Use when publishing a patch via the radicle MCP (create_patch / issue_device_key),
  when an agent would otherwise run rad auth, or when the user asks to reuse the
  same Radicle DID / id for a Cursor environment.
---

# Radicle patches (stable DID per Cursor env)

## Rule

**One Cursor environment → one Radicle device DID.** Never mint a new identity
with `rad auth` for patch publishing unless a dedicated CI key was already
loaded into `$RAD_HOME` from OpenBao/Buildkite.

## Resolve `env_name`

1. Call MCP `cursor-cloud` → `environment-info`.
2. Set `env_name` to `environment.environmentPublicId` (UUID).
3. Pass the same `env_name` on every radicle MCP tool call in this run.

Optional alias (first issue only): `cursor-env-<short-env-name>` (e.g.
`cursor-env-codegod100-sleek`). Do not change the alias on later loads.

## Issue or load the identity

```text
radicle / issue_device_key
  env_name: <environmentPublicId>
  alias: cursor-env-…          # only matters on first create
  start_node: false            # default
  force: false                 # never true unless rotating
```

Expect `created: false` and the same `did` on subsequent runs in this env.

## Open a patch

```text
radicle / create_patch
  env_name: <environmentPublicId>   # required — same as above
  title: …
  body: …                           # full description, not title-only
  branch: …
  commit: …                         # optional: commit then patch
```

`create_patch` auto-issues credentials when needed, but still pass `env_name`
so it does not fall back to an unscoped / ephemeral home.

## Do not

- Run `rad auth --alias …` in the agent VM to “get a key quickly”.
- Omit `env_name` (unscoped home is shared/ambiguous across envs).
- Commit `$RAD_HOME/keys` or paste private keys into the repo/PR.
- Use `force: true` on `issue_device_key` unless the user asked to rotate.

## CI exception

Buildkite issue→agent loads a **dedicated CI** identity from
`RADICLE_SECRET_KEY` / OpenBao `secret/data/radicle` via
`scripts/buildkite/bootstrap.sh`. That path is separate from the Cursor
env-scoped MCP identity. Prefer MCP + `env_name` for interactive/cloud-agent
patches; use the CI key only inside Buildkite bootstrap.
