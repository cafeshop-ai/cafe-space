# Optional Agent capability metadata

This additive extension does not change the frozen v1 baseline. `SessionSnapshot.agent?` is `{name:S(128), modelControl:boolean, deliveryControl:boolean}`. Absence keeps legacy Pi behavior. If present, all three fields are required and unknown nested fields are discarded. Wrong types are rejected by both TS and Go codecs; reducers retain the canonical metadata across events.

These are presentation capabilities, never authorization. Existing gateway host/client authentication, device control and stream/session/cwd fences still apply. A host independently rejects unsupported commands even if an old client ignores the metadata.

`modelControl:false` disables the model/reasoning form and explains local configuration ownership. `deliveryControl:false` hides running-turn steer/followUp choices and keeps a running draft unsendable; idle plain text remains available. Neither flag grants file, lifecycle or cancellation authority. The ACP host permits cancellation only for its own active turn and rejects all unsupported settings/files/history/lifecycle commands.

Experimental ACP transport is source-only. Old deployments may discard this field; the source runner rejects an old protocol build, but real gateway/browser version matching still requires product acceptance. Deterministic checks: `src/protocol/acp-host.test.ts`, `relay/internal/protocol/agent_test.go`, Composer and ModelControls tests.
