# tools/ — reference ingest client + fixtures

Contract-level testing utilities for the CMTrace Open api-server.

These scripts speak the bundle-ingest wire protocol defined in
`crates/common-wire` on branch `feat/api-ingest-v0` (PR #7). Once that PR
lands on `main`, these tools work against `main` with no changes —
they pin to the public HTTP surface, not any internal types.

## Why

Three jobs:

1. **Smoke-test the server.** Manually validate the ingest pipeline
   end-to-end without bringing up the real Windows agent.
2. **Nail down the contract.** The Windows agent (Phase 3 M2) implements
   against exactly this flow; the script is executable documentation.
3. **CI hook.** A follow-up can drop a CI job that brings up the compose
   stack, ships the fixture, and asserts the session round-trips.

No new language runtimes: bash, `curl`, `jq`, `sha256sum` (or macOS
`shasum`), `dd`, `zip`. All present on macOS and every standard Linux CI
image.

## Layout

```
tools/
  README.md              you are here
  ship-bundle.sh         init -> chunk* -> finalize
  query.sh               list/fetch devices and sessions
  fixtures/
    README.md            fixture schema, regeneration notes
    build.sh             reproducible zip builder (zip -X -D)
    .gitignore           test-bundle.zip is generated, not committed
```

The generated `tools/fixtures/test-bundle.zip` is gitignored; always
regenerate it with `bash tools/fixtures/build.sh` before shipping.

## Local smoke against `docker compose up`

From the repo root:

```bash
# 1. Bring up the stack (first build takes ~90s).
docker compose up -d

# 2. Build the fixture.
bash tools/fixtures/build.sh

# 3. Ship it.
bash tools/ship-bundle.sh \
  --device-id WIN-LAB01 \
  --bundle tools/fixtures/test-bundle.zip

# 4. Verify the session landed.
bash tools/query.sh devices
bash tools/query.sh sessions WIN-LAB01
# copy a session-id from the previous output:
bash tools/query.sh session <session-id>
```

Expected output of step 3 (values will differ):

```
OK  device_id=WIN-LAB01  session_id=01900000-...  bundle_id=...  bytes=540  parse_state=pending
```

`parse_state=pending` is correct for MVP — the background parser lands in
Phase 3 M2.

## Smoke against BigMac

The BigMac dev host runs the same compose stack on `192.168.2.50:8080`.

```bash
bash tools/ship-bundle.sh \
  --endpoint http://192.168.2.50:8080 \
  --device-id WIN-LAB01 \
  --bundle tools/fixtures/test-bundle.zip

bash tools/query.sh devices  http://192.168.2.50:8080
bash tools/query.sh sessions WIN-LAB01 http://192.168.2.50:8080
```

If you get connection-refused, confirm the api-server container is up on
BigMac (it listens on 8080) and that the host firewall permits it.

## Exercising the resume path

Reusing the same `--bundle-id` on a second run demonstrates idempotency:
the server returns the existing session's id with `resumeOffset == size`,
the client skips the chunk loop, finalize returns 200 (not 201).

```bash
BID=$(uuidgen | tr A-Z a-z)
bash tools/ship-bundle.sh --device-id WIN-LAB01 \
  --bundle tools/fixtures/test-bundle.zip --bundle-id "$BID"
bash tools/ship-bundle.sh --device-id WIN-LAB01 \
  --bundle tools/fixtures/test-bundle.zip --bundle-id "$BID"
```

Both invocations print the same `session_id`.

## Extending the fixture

See `tools/fixtures/README.md` for the `manifest.json` schema and how to
add new artifacts. Keep the archive under 1 MiB.

## Error semantics (quick reference)

The api-server returns JSON error bodies shaped like:

```json
{"error": "offset_mismatch", "message": "..."}
```

Interesting codes `ship-bundle.sh` may surface:

| status | error code          | cause                                               |
|--------|---------------------|-----------------------------------------------------|
| 400    | `bad_request`       | bad sha256 / contentKind / empty chunk              |
| 400    | `sha256_mismatch`   | init sha != staged bytes, or != finalize claim      |
| 400    | `size_overflow`     | chunk would push past declared sizeBytes            |
| 404    | `not_found`         | upload owned by a different device, or unknown id   |
| 409    | `offset_mismatch`   | chunk offset didn't match server's current offset   |
| 409    | `already_finalized` | finalize called twice without idempotent path hit   |
| 409    | `conflict`          | session uniqueness collision (device_id, bundle_id) |

On any non-2xx the script prints the server body and exits non-zero.

## Follow-ups (not done in this PR)

- CI job that boots the compose stack and runs this end-to-end. Skipped
  here because compose cold-start is ~90s and would dominate CI time; a
  pre-warmed image or a lighter test harness is the right next step.
- mTLS support once M2 lands (currently the scripts only send
  `X-Device-Id`; they'll need `--cert`/`--key` args).
