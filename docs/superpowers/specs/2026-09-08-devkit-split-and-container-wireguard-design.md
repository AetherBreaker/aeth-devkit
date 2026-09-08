# devkit split and container wireguard mode

Date: 2026-09-08. Status: design approved in discussion; each numbered step in section 7 gets its
own implementation plan, written in a separate session.

Companion spec (the consuming project's view):
`ScheduledReportAggregator/docs/superpowers/specs/2026-09-08-wireguard-db-access-design.md`.

## 1. Why

- **The repo is a kitchen sink.** Nine crates, a Python package, a VS Code extension and five
  workflows, releasing three artefact streams (`v*` wheel, `container-vN`, `vscode-extension-vN`)
  off one release event. Every session that edits any part loads all of it. The code is entirely
  LLM-written and the owner does not read Rust, so review happens through behaviour specs and
  tests, which are buried in the volume. Docs already lag code: `TODO.md` lists the sister-project
  Docker migration as pending, and it is done.
- **`devkit-container` is about to become security-sensitive.** It will gain a wireguard
  supervisor: root-phase private-key handling, interface bring-up, and PID 1 supervision of the
  app. Code with that job must be small enough to read top to bottom. The extraction happens
  before that code is written, not after.
- **Template changes should not require a devkit release.** Most template edits are content
  (standards prose, pins, lint rules), not code.

## 2. End state: six repositories

Names are working names; the extraction plan may rename.

| Repo | Contents | Artefact and release | How consumers get it |
|---|---|---|---|
| `aeth-devkit` (slimmed) | `devkit` binary; crates `core`, `setup`, `release`, `pin`, `lock`; Python package (poe task table, two scripts); vendored templates snapshot | maturin `bindings = "bin"` wheel on SFTPyPI via `devkit release`; workflows `ci`, `release` only | dev dependency, `uv sync` |
| `devkit-container` | one dedicated binary, no dispatcher; the Dockerfile template; `[tool.docker]` schema doc; smoke tests | semver tags; assets: static musl binary, Dockerfile template; hand-written workflow (today's `devkit-container.yml` with its trigger changed to this repo's own release) | Dockerfile `ADD` of the pinned release URL |
| `devkit-templates` | every template except the Dockerfile; a template-spec version file | semver tags; CI renders through the latest released devkit | `setup-project` fetches the latest tag; snapshot in the wheel as offline fallback |
| `devkit-vscode` | the extension | own tags; workflow moved from `vscode-extension.yml` | installed by `setup-project` from releases, as today |
| `devkit-claude-hooks` | Rust crate; binary `devkit-hook` | maturin bin wheel on SFTPyPI; devkit-managed, `devkit release` | dev dependency; `.claude/settings.local.json` command lines |
| `devkit-poe-complete` | Rust crate; binary `devkit-complete` | same shape as hooks | dev dependency; the shell shims it installs |

## 3. What stays together, and why

**`setup`, `release`, `pin`, `lock` and `core` remain workspace crates in `aeth-devkit`.**

- *Diamond dependency.* They pass `&dyn Runner` and `&dyn Prompt` from `core` across crate
  boundaries. As separate repos each would pin `core` by git tag, and Cargo treats two tags of a
  git dependency as two unrelated crates: the moment `setup` and `release` pin different tags the
  trait objects are incompatible types and `devkit` does not build. Every `core` change would
  force a lockstep tag across four repos.
- *Co-evolution.* A typical change touches a template, `setup`, and `core` together. One
  workspace makes that one commit.
- *One wheel.* Consumers get one `devkit` on PATH. One workspace is what makes that one build.

**No shared library repo.** The only code the departing crates share with `core` is the
`process::Runner` seam (`complete` and `hooks` use nothing else). A library repo whose export is a
test seam adds a dependency edge to every satellite to save roughly a hundred lines. Each
satellite carries its own copy. Revisit only if `setup` and `release` ever split, which would make
`core` a genuinely shared, evolving model.

**Hooks and completion are standalone binaries, not libraries pulled into `devkit`.** Nobody types
`devkit hook` or `devkit complete`: Claude Code runs hooks, the shell runs completion. Nothing is
gained by keeping them under the `devkit` name, and the library model would cost a two-step
release (tag the library, bump the pin, release devkit) for every change.

## 4. Per-component specifics

### 4.1 `devkit-container`

- The crate moves verbatim: `app-extra`, `readme`, `run` all stay. It already depends on no
  workspace crate (its own small pyproject parser is deliberate).
- The Dockerfile template moves with it, together with the smoke test that builds the template
  around a scratch app. The Dockerfile is the binary's usage contract and they version together;
  the smoke test already asserts "the template changed shape".
- Releases are semver (`vX.Y.Z`), replacing the `container-vN` counter. In `setup`:
  `static_files::DEVKIT_REPO` points at the new repo, `pinned_container_version` parses semver,
  and the template's `ADD` URL changes host. The advancement policy is unchanged: `setup-project`
  fills a missing pin with the newest release and never advances an existing one; advancing is a
  separate command (existing TODO).
- `setup-project` obtains the Dockerfile template from the release asset at the pinned tag,
  cached per tag, not from the templates repo: the Dockerfile must match the binary it fetches.
- The Windows build in the release matrix stays only while a test needs it; the query
  subcommands are the only thing it exercises off Linux.

### 4.2 Wireguard mode

Switch: `[tool.docker].wireguard = true` in `pyproject.toml`. Read by the container binary at
`run` and by `setup-project` for template gating. Off is today's behaviour exactly: `exec`, the
app is PID 1. The smoke test's `pid_1` assertion applies to off mode only.

**Entrypoint, mode on** (root phase, in this order):

1. Read the `WG_*` environment. Render `wg0.conf` to a root-only tmpfs path, mode `0600`.
   Remove `WG_PRIVATE_KEY` and `WG_PEER_PRESHARED_KEY` from the environment that will be handed to
   the app. A failure here names the variable, never the value.
2. Bring up `wg0`: interface, key, address, peer, routes for the peer's allowed IPs. Whether via
   `wg-quick` or `ip` + `wg setconf` is the plan's call; both need `CAP_NET_ADMIN` and
   `/dev/net/tun`.
3. Wait for the first handshake, up to `WG_HANDSHAKE_TIMEOUT_SECS` (default 60). No handshake is
   a refused start, with the endpoint named in the message.
4. The existing steps: mount check, `prepare` (mkdir and chown).
5. Spawn, not exec, `/app/.venv/bin/<run-app-*>` as 999:999 with empty supplementary groups (a
   `pre_exec` doing what `run.rs` does today). The entrypoint stays PID 1 as root because re-upping
   the tunnel needs `NET_ADMIN`. The child's capability sets are empty; the smoke test asserts
   `CapEff`, `CapPrm` and `CapAmb` are zero in the child's `/proc/self/status`.
6. Supervise: forward `SIGTERM`, `SIGINT` and `SIGHUP` to the child; reap zombies; every
   `WG_POLL_SECS` (default 30) read the latest handshake and, if older than `WG_STALE_SECS`
   (default 180), re-up the tunnel and log it; write the status file after every poll.
7. On child exit: bring `wg0` down, exit with the child's code (signal death as 128+n).

**Status file.** `/run/devkit/wireguard.json`, world-readable, rewritten atomically, fields:
`state` (`up` | `stale` | `reupping` | `down`), `latest_handshake_epoch`, `endpoint`, `rx_bytes`,
`tx_bytes`, `reups`, `updated_at`. This is the only tunnel information the app can see; reading it
needs no capabilities. `aeth_ext` reading it and alerting on `stale` is follow-on work in that
project.

**Environment contract** (names are a proposal; the plan fixes them and the schema doc records
them):

| Variable | Meaning |
|---|---|
| `WG_PRIVATE_KEY` | this peer's private key; secret; required |
| `WG_ADDRESS` | this peer's tunnel address, CIDR (`10.8.0.20/32`) |
| `WG_PEER_PUBLIC_KEY` | the hub's public key |
| `WG_PEER_ENDPOINT` | `host:port` of the hub |
| `WG_PEER_ALLOWED_IPS` | comma-separated CIDRs routed through the hub |
| `WG_PEER_PRESHARED_KEY` | optional; secret |
| `WG_PERSISTENT_KEEPALIVE` | seconds; default 25 |
| `WG_HANDSHAKE_TIMEOUT_SECS`, `WG_POLL_SECS`, `WG_STALE_SECS` | defaults 60, 30, 180 |

**Compose and Dockerfile rendering.** Both templates gain an `if-wireguard` block using the
existing `# setup-project: if-<name>` gate, enabled from the new context flag. The compose block on
the app service adds `cap_add: [NET_ADMIN]`, `devices: [/dev/net/tun:/dev/net/tun]`,
`sysctls: net.ipv4.conf.all.src_valid_mark=1`, and the `WG_*` environment lines, every value as
`${NAME:?}` so the compose file is identical across projects and all values live in the deploy
environment (Coolify). The Dockerfile block installs `wireguard-tools` and `iproute2` in the final
stage. The compose rule engine needs no code change: keys the standard does not name are ignored,
and the `EnvKeys` rule already inserts scaffold keys that are missing.

**Host requirements.** A Docker host kernel with the wireguard module (Linux 5.6 or later), and a
deploy platform that passes `cap_add`, `devices` and `sysctls` through. Both are verified on the
first deploy, not assumed.

**Tests.** Unit: config rendering, environment scrubbing, status serialisation, stale-handshake
logic with an injected clock. Smoke, in CI beside the existing one: a hub container with generated
keys and an image built from the template with the mode on, on one Docker network. Asserts:
handshake within the timeout; child uid 999 with empty capability sets; `WG_PRIVATE_KEY` absent
from the child's environment; status file present, readable by 999, `state: up`; `SIGTERM`
reaches the child and its exit code passes through; removing the peer on the hub and restoring it
produces `stale` then `up` with `reups` incremented.

### 4.3 `devkit-templates`

- Contents: everything under `python/aeth_devkit/templates` except `template.Dockerfile`, plus a
  template-spec version file (an integer). The two shell scripts under `scripts/` stay in devkit:
  they are code the poe tasks call, not standards.
- **Template language is versioned.** Templates are not passive text: placeholders, `if-*` gates,
  the compose `service-block` markers and the shapes the merge engines expect are a language that
  `setup` implements. A content change needs no code; a shape change does. `setup-project`
  declares the spec versions it supports and refuses a templates tag outside that range, naming
  the devkit release that supports it. Without this a new placeholder renders as literal braces
  into a project file and nothing notices.
- **Latest tag, never HEAD.** `setup-project` is documented as idempotent and `--check` exits 1 on
  drift, which is what makes it usable in CI. HEAD would make every project drift on every push
  and give different answers on different days with nothing to name. Tagging is the ceremony that
  replaces the devkit release: push freely, tag when the change is meant to flow.
- **The chosen tag is recorded** in `[tool.setup-project]` (key name is the plan's call). Plain
  online runs resolve the latest tag, use it, and record it, so templates flow without a
  per-project step; `--templates-tag` pins explicitly. `--check` uses the recorded tag and never
  advances, so CI stays deterministic between deliberate advances.
- **Offline rule.** Use the recorded tag from the per-user cache if present; otherwise the wheel's
  snapshot if its tag is at least the recorded one and its spec version is supported; otherwise
  stop with a note. Never downgrade silently: a stale snapshot would otherwise offer to walk a
  project's files backwards and the Docker consent flow would present that as an ordinary diff.
- **Fetch.** Shallow clone of the tag into a per-user cache keyed by tag, using the GitHub access
  pattern `vscode/install.rs` already has. Once cached, later runs on that tag need no network.
- **Snapshot.** The devkit release workflow vendors the templates at their latest tag into the
  wheel, where they live today, and records the tag beside them. Whether the vendored copy is
  committed or generated at build time is the plan's call; the requirement is that the wheel
  carries a snapshot that knows its tag.
- **Templates CI.** On every push, render every template through the latest released devkit
  against scratch projects (pure Python, Rust, Docker) and fail on a render error or an
  unresolved placeholder. The compatibility guard lives here, before a tag exists.

### 4.4 `devkit-vscode`

- `vscode-extension/` and its workflow move; the workflow triggers on this repo's own tags.
- The consent protocol (`setup/src/vscode/protocol.rs`) gains an explicit version exchanged in the
  handshake. A mismatch retires the reviewer with a note, which is the existing behaviour for a
  transport error. `install.rs` `REPO` and `TAG_PREFIX` point at the new repo; it already picks the
  newest compatible release.

### 4.5 `devkit-claude-hooks` and `devkit-poe-complete`

- Each crate becomes its own repo with a private process seam: the trait, the system runner, and
  as much of the recording runner as its tests use. No dependency on `core`.
- Each gets a minimal `pyproject.toml` with maturin `bindings = "bin"`, publishing binaries
  `devkit-hook` and `devkit-complete`. That is the shape `aeth-devkit` itself has, so both are
  devkit-managed and released with `devkit release`; no custom workflow.
- Template changes: the dev group gains both packages; the `.claude/settings.local.json` template
  commands become `devkit-hook <name>`; the shims `devkit-complete install` writes call
  `devkit-complete query`. The "no global install" property holds: the venv on PATH supplies the
  right version at Tab time and at hook time.
- Dispatcher: remove the `Complete` and `Hook` variants and the `wants_update_check` special
  cases. Existing projects migrate by running `setup-project` and re-running
  `devkit-complete install` once; `setup-project` prints a note saying so.

### 4.6 `aeth-devkit` after the split

- Crates: `aeth-devkit`, `core`, `setup`, `release`, `pin`, `lock`. Workflows: `ci` (drop the
  container-smoke and extension jobs) and `release`.
- `TODO.md`: drop the entries the split makes moot, including the stale migration entry.
- Optional, decided separately: stop baking the poe task table in `build.rs`. The generated file
  is a plain dict; authoring it directly removes a build-time Python dependency, the
  `poethepoet-tasks` build requirement, and the regeneration test.

## 5. Pin policy

One mechanism (fill a missing pin, record what was chosen) with a stated policy per pin:

| Pin | Written where | Advances |
|---|---|---|
| container release | Dockerfile `ADD` | on request only: it changes runtime behaviour |
| extension release | at install time | newest compatible at install (existing) |
| templates tag | `[tool.setup-project]` | by default on plain runs; never on `--check` |
| hooks and completion wheels | dev group floors | with `uv sync` / `devkit lock`, like every dev dependency |

## 6. Cross-repo contracts

Each has one owner and a version, and the consumer checks the version.

| Contract | Owner | Consumer |
|---|---|---|
| Dockerfile template and `[tool.docker]` schema | `devkit-container` | `setup-project`, at the pinned tag |
| `/run/devkit/wireguard.json` path and fields | `devkit-container` | `aeth_ext` (follow-on) |
| consent protocol | `devkit-vscode` | `setup` |
| template language (placeholders, gates, markers) | `setup` | `devkit-templates`, via the spec version |
| `devkit-hook <name>` command line and payload | `devkit-claude-hooks` | the settings template |
| shim to `devkit-complete query` wire format | `devkit-poe-complete` | internal; it installs its own shims |

## 7. Sequence

Each step is its own plan, branch and PR.

1. Extract `devkit-container`. Unblocks the wireguard work.
2. Wireguard mode in the container, plus the `[tool.docker].wireguard` context flag in `setup`
   and the gated blocks in the compose and Dockerfile templates, wherever those templates live at
   the time.
3. Extract `devkit-vscode`.
4. Extract hooks and completion.
5. Create `devkit-templates` and the fetch, record, offline and spec-version machinery.
6. Slim `aeth-devkit`: constants, CI, TODO, README; optionally the bake removal.

Steps 3, 4 and 5 are independent of 1 and 2 and of each other. Step 6 is last.

## 8. Rejected alternatives

- **Library repos for `setup`, `release`, `pin`, `lock`.** Diamond dependency through `core`;
  co-evolution; one wheel. See section 3.
- **A shared `core` library repo.** Only the `Runner` seam is shared with anything that leaves.
- **Hooks and completion as libraries pulled in at devkit release.** Two-step release for no
  consumer benefit, since nobody invokes them by the `devkit` name.
- **A uv workspace monorepo.** uv workspaces share one lockfile across Python packages that
  co-develop; devkit has one Python package and the rest is Rust and TypeScript. The Cargo
  workspace is already the monorepo for the Rust parts. Neither pain (per-session context, release
  coupling) is addressed by a monorepo; the current repo already simulates per-component releases,
  and that simulation is the awkward part.
- **Templates fetched from HEAD.** Breaks idempotence and `--check`, has no named unit, and makes
  the offline fallback a regression risk.

## 9. Done means

- Every sister project passes `poe setup-project --check` after migration.
- `ScheduledReportAggregator` builds against the new container URL, in both modes.
- Both container smoke tests are green in the container repo's CI.
- A templates tag flows to a project with no devkit release, and an offline run with a stale
  snapshot stops with the note rather than proposing a downgrade.
- `aeth-devkit` CI has two jobs fewer and its README describes only what it still contains.