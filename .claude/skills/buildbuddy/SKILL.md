---
name: buildbuddy
description: >
  Configure and troubleshoot BuildBuddy authentication, Buck2 remote execution,
  remote caching, bb login, and missing BUILDBUDDY_API_KEY errors in Sleek.
  Use whenever BuildBuddy, RBE, remote cache, bb login, buildbuddy.api-key,
  BUILDBUDDY_API_KEY, or remote.buildbuddy.io is mentioned.
---

# BuildBuddy in Sleek

## Credential location

BuildBuddy's `bb login` does **not** normally store the API key in a system
BuildBuddy YAML file. It stores the key in the current repository's local Git
configuration:

```text
<git-dir>/config
```

under:

```ini
[buildbuddy]
    api-key = ...
```

Read it without displaying the secret:

```bash
test -n "$(git config --local --get buildbuddy.api-key)" &&
  echo "BuildBuddy key is configured for this repository"
```

Load it for a process that only understands `BUILDBUDDY_API_KEY`:

```bash
BUILDBUDDY_API_KEY="$(git config --local --get buildbuddy.api-key)" \
  ./buck2 build :sleek
```

Never print the value or copy it into a tracked file.

The BuildBuddy CLI checks credentials in this order:

1. `BUILDBUDDY_API_KEY`
2. `buildbuddy.api-key` in the repository-local `.git/config`
3. interactive `bb login`, which writes option 2

`BUILDBUDDY_CONFIG_DIR` defaults to the platform user config directory plus
`buildbuddy` (normally `~/.config/buildbuddy` on Linux), but that directory is
for other CLI configuration. It is not the default API-key storage used by
`bb login`.

## Shared developer credential via SecretSpec

Sleek declares `BUILDBUDDY_API_KEY` in `secretspec.toml`. Devenv resolves it
from the current OS user's keyring and exports it into the development shell.
Because SecretSpec addresses the entry by project (`sleek`), profile
(`default`), and key name, all Sleek checkouts and worktrees for the same OS
user can resolve the same keyring entry.

Initialize it once per user/machine:

```bash
devenv shell
secretspec set BUILDBUDDY_API_KEY --provider keyring
```

If the key already exists only in the current checkout's Git config, run the
interactive `secretspec set` command above and paste the value at its masked
prompt. Do not put the value in shell history or a tracked file.

Check resolution without revealing the value:

```bash
secretspec check --provider keyring
```

This shares the credential across worktrees and clones on one machine. It does
not copy the secret to another machine, OS user, CI runner, or remote Delta
agent; configure that environment's provider separately.

## Sleek Buck2 integration

- `./buck2` is the repository wrapper; prefer it over a system `buck2`.
- The wrapper resolves credentials in this order: existing environment or
  `.env`, SecretSpec's `sleek/default/BUILDBUDDY_API_KEY` keyring entry, then
  the current clone's `buildbuddy.api-key` Git config. If none resolve, it
  disables BuildBuddy and builds locally.
- `.buckconfig` references `$BUILDBUDDY_API_KEY` in
  `[buck2_re_client] http_headers`.
- `platforms/defs.bzl` gates both `remote_enabled` and
  `remote_cache_enabled` with `[buildbuddy] enabled`.
- The wrapper uses a credential-fingerprinted `sleek-buildbuddy-*` isolation
  directory for authenticated RBE and `sleek-local` when BuildBuddy is
  disabled. Buck daemons retain their startup environment; fingerprinting
  prevents a newly configured or rotated key from reconnecting to a daemon
  that still has missing/old authentication.
- A repository `.env` is optional and git-ignored. Do not create another copy
  of a key if `bb login` already stored it in local Git config; export that
  existing value instead.

## Diagnosis

Check whether a key exists without revealing it:

```bash
if [[ -n "${BUILDBUDDY_API_KEY:-}" ]]; then
  echo "key: environment"
elif git config --local --get buildbuddy.api-key >/dev/null; then
  echo "key: repository Git config"
else
  echo "key: missing; run bb login"
fi
```

If Buck reports:

```text
Error substituting `$BUILDBUDDY_API_KEY`: environment variable not found
```

the key may still be present in `.git/config`; Buck2 does not read the
BuildBuddy CLI's Git setting itself. Export it into the Buck process, or make
the repository wrapper perform that bridge.

Confirm RBE is active by checking that build actions report remote execution
rather than only `local_execute`. Do not treat a successful local fallback as
proof that BuildBuddy authentication worked.

## Related project skill

Use `.claude/skills/analyze-build/SKILL.md` for inspecting hosted BuildBuddy
invocations triggered by the proxy/Tangled artifact pipeline.
