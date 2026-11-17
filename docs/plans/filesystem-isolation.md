# Filesystem isolation — Windows feasibility draft

**Status: not implemented.** The owner confirmed actual file-access restrictions, including commands and their descendants, rather than a list of permitted working directories. This draft authorizes no host ACL changes, administrator setup, real model calls or installer replacement. The companion UI change removes the `never_asks` hint; it changes no permissions.

## 1. Product contract

- Folders are optional. No selection means the current OS user's normal permissions, not administrator elevation. This is the owner's requested initial default; a review suggestion to require choosing unrestricted mode does not supersede it.
- Show an explicit access-mode selection, initially `不限制`, alongside optional multi-folder selection. Persist that mode separately from the folder list. Saving an empty restricted list, losing a configuration file or failing validation must never select unrestricted mode.
- Multiple selected folders define allowed **user-data** roots, not merely cwd choices. Agent code, file tools, shell commands and all descendants must be confined. Minimum system/runtime support resources are separate and documented below; do not promise that no system file outside the chosen folders is read.
- Proposed cwd: the native-resolved user home for a new unrestricted session without a project; the chosen primary folder for a restricted session (first selected initially). Keep the current directory identity check and generation fencing. Do not use a drive root, the daemon's cwd or a fake approved-project record as a shortcut. Missing home/primary directories fail admission.
- Access policy and cwd are distinct. Existing project/session history remains intact and retains its actual unrestricted behavior; old approved projects do not become evidence of isolation. Resolve the new protocol and storage shapes before implementation.

Proposed layout, not implemented:

```text
II 项目（可选）

(o) 不限制   ( ) 指定文件夹

[选择文件夹…]
D:\work\app       [移除]
D:\work\docs      [移除]
```

Selecting folders enters restricted mode. Clearing its final folder requires explicitly changing mode before an unrestricted start. Per-folder errors retain other selections. Do not show restriction as active until the backend confirms it. Changing policy cannot silently reclassify a running process: stop and reap affected sessions before applying a removal, with owner confirmation; retain history for an explicit Resume under the new policy generation. Do not expose a “revoked” state while old handles remain usable.

## 2. What the code enforces today

`ProjectAddForm` approves one cwd at a time; multiple projects already work. `CreateSessionParams` and `host/pi_launch.rs` require a project ID and cwd fence. `managed/mod.rs` explicitly is not a sandbox: Windows Job Objects own process lifetime, and Unix process groups serve a similar lifecycle role. Pi's native tools have owner permissions outside the project. Codex launch is not yet implemented.

The Windows candidate is AppContainer, with LPAC evaluated to avoid broad application-package grants. Existing ordinary AppContainer access is not proof that only our roots are accessible. Test both system capabilities and an outside fixture already readable by `ALL APPLICATION PACKAGES`; do not grant broad filesystem capabilities to make a failing runtime work.

A low-integrity token or Job Object alone does not provide the required read boundary. Do not generalize that every restricted token can only restrict writes: `CreateRestrictedToken` has a specific `WRITE_RESTRICTED` flag. This draft chooses to investigate AppContainer/LPAC, not to prove it is the only possible mechanism.

Launch the complete Agent inside the boundary, before any Agent code executes. Query its token and real child/grandchild tokens to verify confinement; startup flags alone are insufficient evidence. Preserve the existing explicit standard-stream handle list, atomic Job assignment, kill-on-close and bounded reaping. No inherited unrestricted file handles or full-trust launch/command broker.

## 3. Folder grants, revocation and crash recovery

These are requirements for the prototype, not claims of a solved ACL transaction:

1. Use a fresh, private container identity per session/policy generation. Never reuse an identity with outstanding grant cleanup. Inspect directory identity, filesystem support, reparse points, protected DACLs and existing rights before granting. No taking ownership, replacing the owner's ACL, elevating, or recursively following links.
2. Add only the container SID's required ACEs: read/list/traverse, write/create/rename/delete and execute as needed within selected roots, with explicit inheritance flags. Do not grant `WRITE_DAC`, `WRITE_OWNER` or generic full control. Test whether the file owner semantics, newly created objects or alternative APIs still permit ACL modification; a bypass fails the mechanism, not merely the test. Existing unrelated ACEs, deny rules and integrity labels stay intact.
3. Before each ACL operation, flush a private write-ahead record of container identity, policy generation, object/volume identity, exact intended ACE tuple and before/after evidence. Paths/SDDL remain in owner-only storage, not UI events or public diagnostics. Proposed ledger location is a versioned `isolation/` area in the daemon's private root; its schema and recovery implementation are still to be designed. A journal write failure prevents the grant.
4. Inheritable ACE propagation is potentially recursive and non-transactional. Record intent before the API call and verify descendants afterward, including protected DACLs. On failure or ambiguous completion, launch no Agent and retain cleanup state. Revocation first drains and reaps the old process tree, then removes only grants owned by this identity, verifies removal and retires the profile. Deleting a profile is not ACL cleanup. Never restore an old whole-DACL snapshot over intervening owner edits.
5. Startup recovery handles partially granted/revoked trees before admitting related restricted work. Crashes may leave residual ACEs; do not claim physical atomicity. Unresolved cleanup quarantines the identity and blocks its reuse. Missing, moved or replaced objects and conflicting ACL edits require retained evidence and an owner-resolved state, not broad traversal or destructive repair. An unavailable cleanup target must not become an unrestricted session.

Measure grant/verify/revoke separately on disposable trees of 1,000, 10,000 and 100,000 entries, including a `node_modules`-like tree. Record duration, object counts, maximum resource use and cancellation/recovery behavior. There is no measured performance result yet. A slow or interrupted OS propagation call must not stall the daemon's request loop or allow premature launch; the worker/cancellation design is a prototype gate.

ACLs attach to objects, not path strings. Hardlinks, owner moves across boundaries, child objects retaining grants, root replacement and grant-time races need explicit tests. In particular, a file already hardlinked outside a selected tree can share the granted DACL. If a path-only promise cannot be upheld, reject that case or reconsider the mechanism; do not quietly redefine the allowlist. Junction and symbolic-link targets are checked using their actual access rights, including grants from any other source. A prefix check is not containment.

## 4. Required support resources

Do not expose an entire home, `.pi`, `.ssh`, tool-install parent, daemon root or shared cache to satisfy a launch failure. The exact manifest is produced and reviewed by the prototype. Proposed starting rights:

| Resource | Proposed access | Boundary / unresolved compatibility |
|---|---|---|
| Selected user-data folders | Read/list, modify and execute; no ACL/owner-management rights | Each canonical object must be admissible; verify descendants and cross-root aliases. |
| Windows loader, libraries, shell and registry support | Minimum read/execute or named capabilities | Enumerate necessary support; no write grants or broad user-data capability. PowerShell and cmd must actually run inside the boundary. |
| Node, pinned Pi package, git and npm | Read/execute | Prefer owned staged runtime copies if grants to installed binaries are unavailable or unsafe. Account for linked package stores and DLL/script dependencies; never grant the entire package-manager store. Copying and updating are not implemented. |
| Pi authentication and model configuration | Minimum per-session private copy of required records, initially read-only | Do not grant the original `.pi` tree. Credential refresh writes, config command hooks and private broker possibilities require review; no arbitrary file/command broker. Real credentials require owner-approved testing and never appear in logs. |
| Pi native session/history files | Read/write for this session only | Current history lives under daemon-owned storage. A container-writable area conflicts with current owner-only file validation; design a separate area and a validated bounded import/Resume path. Do not weaken the shared private-file checks. |
| Temporary files and package cache | Read/write in isolated session-owned directories | Redirect supported temp/cache variables; do not share the owner's temp tree or npm cache. These directories need the same revocation lifecycle. |
| Git user config | Optional scoped read-only copy | Review includes and executable helpers; no automatic grants to `.gitconfig` include targets or credential-helper paths. Test `git status` without granting SSH credentials. |
| SSH config/keys/agent | None by default | A future explicit resource grant or narrow credential design needs separate consent. No wholesale `.ssh` access. A copied secret is still available to Agent code. |
| npm user config | Optional scoped read-only copy | `.npmrc` may contain credentials and paths; no automatic grant to its referenced files, cache or hooks. `npm -v` must not require exposing the home. |
| Daemon DB, backups, other sessions and control IPC | None | Runtime support must not reopen a path to create unrestricted siblings or modify policy. |

D20's full environment inheritance remains unchanged for unrestricted sessions. The restricted profile needs an explicit reconciliation: inherited API keys remain readable to Agent code, while filesystem isolation does not conceal environment values. Home/temp/session/cache routing and credential resources cannot be silently changed under D20 or reported as secret isolation. Review this contract before a real Pi launch.

## 5. Daemon IPC and network gates

Source evidence: `windows_security::OwnerSecurity::new(false)` builds a protected owner-SID-only DACL. `local::windows::create` uses that descriptor and `reject_remote_clients(true)`; it grants no application-package group. This is source inspection, not a live AppContainer connection test. Verify the live pipe's DACL, then attempt a connection from the container and a descendant: both must fail. Also deny reads of policy ledgers/DB and attempts to reach an unconfined helper. Do not loosen pipe rights to fix the probe.

**Local proxy connectivity is a release gate.** AppContainer loopback restrictions may prevent the owner's existing HTTP/SOCKS proxy from working. Microsoft's documented exemption for communication with unpackaged localhost services requires administrator privileges. Do not run `CheckNetIsolation`, change firewall rules, rebind the proxy to a LAN address, bypass the proxy or request elevation automatically. A broad loopback grant could also expose unrelated full-trust local services.

Check the actual non-elevated profile and the owner's chosen proxy only after authorization, without disclosing its address, credentials or response body in shared evidence. First prove transport, then one bounded model call using disposable project fixtures. Do not infer success from a shell outside the container. If this requires administrator setup, the current no-admin candidate fails; report the conflict before considering any opt-in exemption or narrowly scoped networking broker. No broker is approved or implemented by this draft.

## 6. Prototype and acceptance evidence

Separate prototype work from the UI deletion. Before running it, disclose any builds, persistent profile/grant changes and real model usage and obtain the applicable owner authorization. Begin with disposable owned directories and synthetic credentials; no ACL edits on real projects or installed tools.

| Gate | Required evidence |
|---|---|
| Allowed work | A real confined Node/Pi process plus cmd/PowerShell descendants can read/write/list both allowed fixture folders. Run `git status` and `npm -v`, capturing exit status only. |
| Outside access | Direct and descendant reads/writes/listing fail against an otherwise owner-readable third folder. Include relative parent paths, absolute paths, a junction, a symbolic link, hardlinks, broad-package ACEs, root replacement and rename races. Missing symlink-creation privilege is an untested gate, not a pass. |
| Authority escape | Child/grandchild tokens retain isolation; normal-user IPC connection succeeds as a control while confined IPC access fails. Attempts to change ACLs, open another session's storage or obtain an unconfined process fail. |
| Network and Pi lifecycle | Through the owner's actual proxy, one short disposable Pi turn streams correctly. Native history supports explicit Resume. Cancel/close and daemon-crash cleanup retain existing lifecycle guarantees. No output containing secrets is exported. |
| Recovery and scale | Inject failure/crash before and after journal flush, grant propagation, launch and revocation. Verify no automatic unrestricted start, no identity reuse with residual grants, bounded request handling, and 1k/10k/100k grant/revoke timings. |

Evidence identifies the exact code SHA, Windows version, elevation state, backend/capabilities, fixture identities, operation outcomes and cleanup state. No raw credentials, private paths, native stderr or owner file contents in published reports. All gates are **unrun**; no supported backend or compatibility promise is established yet.

## 7. Integration after feasibility passes

- Specify and review policy/capability/fence messages and fixtures first (D18); bump the full protocol for shape changes. Update CLI, generated types, admission, launch/Resume, settings and optional-project onboarding together. Policy writes stay local-owner-only; remote clients cannot supply paths or select unrestricted fallback.
- Migrate stored shapes with backups and explicit legacy semantics. Keep `fixtures/control-v2/trace.json`, `fixtures/migrations/m0-api.json` and `fixtures/migrations/v6.sql` frozen. Do not treat old approved cwd records as isolation consent or evidence.
- An unavailable backend, invalid policy, grant failure or startup failure rejects restricted work with a fixed error code. macOS/Linux restricted mode stays unavailable until their own enforcement is implemented and tested; never silently run unrestricted.
- Non-author review and same-SHA CI precede merging. The existing installed preview remains unchanged; a replacement installer requires its separate authorized build and acceptance.

## References

- [AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation)
- [Launch an AppContainer / LPAC](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
- [CreateRestrictedToken flags](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken)
- [Inheritable ACE propagation](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-setnamedsecurityinfow)
- [Loopback restrictions and exemptions](https://learn.microsoft.com/en-us/windows/uwp/communication/interprocess-communication#loopback)
