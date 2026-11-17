# M4b #110 — Local session folder contract

Status: backend source implemented on full wire v11, after strict fixtures and generated clients
(D18). Windows workspace tests and Web typechecking/tests pass; final-tip CI, non-author review and
frontend integration remain. Frozen control v2 and schema 6 do not change.

## Two requests, one existing session-create path

1. This computer resolves the selected folder with `resolve_local_session_folder`.
2. The client uses the returned project's ID and cwd fence in the existing `create_session` request.

There is no second create/launch implementation. Existing session IDs, New/Resume checks, unknown
outcomes, native conversation identity, update admission and owned process-tree policies remain.
Folder resolution never starts an Agent or sends a prompt. A recorded folder is a cwd, not a sandbox.

### Request

```json
{
  "method": "resolve_local_session_folder",
  "params": { "root": "D:\\work\\example" }
}
```

`root` is optional. If supplied, it must be a bounded absolute local-directory path under the same
rules as `add_project`; invalid explicit input is rejected, never replaced by a different cwd.
Unknown fields, including an access flag or client-supplied project identity, are rejected.

When `root` is absent, use the most recently active session's still-valid recorded folder, falling
back to the current user's home directory. Rank known `lastActivityAt` descending, preserving the
existing session-list order as a deterministic tie-breaker; unknown timestamps rank last. Missing,
removed or stale projects do not prevent checking other recorded folders. A missing home directory
is a fixed rejection, not a cwd/environment fallback.

### Applied response payload

```json
{
  "kind": "local_session_folder",
  "data": {
    "project": { "id": "opaque-project-id", "name": "example", "cwdFence": "opaque-cwd-fence" },
    "root": "D:\\work\\example"
  }
}
```

The normal outer envelope includes the negotiated full-wire version, request ID and `applied`
status. Root-bearing types redact their Debug output, like `RegisterPiRuntimeParams` and
`PiRuntimeDiscovery`. Paths never enter project/session events or shared summaries.

## Authorization and persistence

- Reject `Access::Remote` with `LOCAL_ONLY` before inspecting any path or changing any record.
  Locality is assigned by the authenticated native transport, never by request data. Remote clients
  continue selecting existing project IDs and cwd fences; this does not change their create policy.
- Reuse `projects::add` and the directory identity checks to deduplicate folders, including Windows
  case/separator aliases. Do not replace identity checking with string-only comparison: a replaced
  directory must still fail its existing fence/identity checks.
- Newly selected local folders are recorded directly, without a separate approval. Publish the
  existing path-free project event on an actual project insertion; reuse is a no-op.
- Derive the default from existing session/project data. Do not add a last-folder settings file or
  schema migration. Successful `create_session` establishes a use; merely opening/cancelling a sheet
  does not change the remembered folder.
- Apply the existing migration, shutdown and update admission gates. Preserve private data and D20;
  do not create a directory, change an ACL, elevate a process or implement filesystem isolation.

## Client timing and outcomes

Opening the sheet only reads existing data. Display the latest session's folder using
`lastActivityAt`; when no usable session exists, display the home folder (native `homeDir` may supply
its path). Do not call the mutating resolver just to show that default.

The native picker only fills the sheet. Resolve once, only after the user presses Start.
Opening the sheet, selecting a folder and then cancelling sends no resolver request. With an
already recorded project and a current fence, the existing `create_session` path can be used directly.

Keep the resolved project and the exact `CreateSessionParams` while a create outcome is unknown.
Reconcile using the same session ID; do not choose another folder, allocate another ID or replay a
prompt automatically. A resolver rejection does not call `create_session`. An uncertain durable
project write is `unknown`, not an invented rejection or success.

## Errors and fixture gate

Reuse bounded project errors (`PROJECT_ROOT_UNSUPPORTED`, `PROJECT_ROOT_UNAVAILABLE`,
`PROJECT_ROOT_NOT_UTF8`, `PROJECT_ROOT_IDENTITY_UNAVAILABLE`, `PROJECT_ROOT_CHANGED`, `PROJECT_LIMIT`)
and existing admission errors. Use fixed `HOME_UNAVAILABLE` for an unavailable default home and
`LOCAL_FOLDER_RESOLUTION_UNKNOWN` for an uncertain resolver write. Never export raw filesystem or
SQLite errors, local paths in error text, or environment values.

The implementation batch must cover strict request/response bounds, remote rejection before any
path access, home/default selection, per-item stale-project fallback, alias deduplication, directory
replacement, no-op selection, cancellation without project writes, New/Resume creation with the
returned fence, and same-ID reconciliation. Keep `fixtures/control-v2/trace.json`,
`fixtures/migrations/m0-api.json` and `fixtures/migrations/v6.sql` byte-identical.
