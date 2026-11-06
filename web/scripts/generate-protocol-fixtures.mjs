import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const fixtureDir = new URL("../../fixtures/v2/", import.meta.url);
const output = fileURLToPath(new URL("../src/protocol-fixtures.generated.ts", import.meta.url));
const cases = [
  ["clientHello", "client-hello.json", "ClientMessage"],
  ["clientRequest", "client-request.json", "ClientMessage"],
  ["projectRequests", "project-requests.json", "readonly ClientMessage[]"],
  ["projectEvents", "project-events.json", "readonly ServerMessage[]"],
  ["projectResponse", "project-response.json", "ServerMessage"],
  ["getProjectRootRequest", "get-project-root-request.json", "ClientMessage"],
  ["projectRootResponse", "project-root-response.json", "ServerMessage"],
  ["projectLocalOnly", "project-local-only.json", "ServerMessage"],
  ["projectStaleFence", "project-stale-fence.json", "ServerMessage"],
  ["createSessionRequest", "create-session-request.json", "ClientMessage"],
  ["openSessionRequest", "open-session-request.json", "ClientMessage"],
  ["closeSessionRequest", "close-session-request.json", "ClientMessage"],
  ["closeSessionDispatched", "close-session-dispatched.json", "ServerMessage"],
  ["sessionLifecycle", "session-lifecycle.json", "readonly ServerMessage[]"],
  ["sessionTransitions", "session-transitions.json", 'Readonly<Record<SessionState["state"], readonly SessionState["state"][]>>'],
  ["approvalRequest", "approval-request.json", "ClientMessage"],
  ["setModelRequest", "set-model-request.json", "ClientMessage"],
  ["setThinkingRequest", "set-thinking-request.json", "ClientMessage"],
  ["subscribeEventsRequest", "subscribe-events-request.json", "ClientMessage"],
  ["subscribeSnapshotRequest", "subscribe-snapshot-request.json", "ClientMessage"],
  ["prepareUpdateRequest", "prepare-update-request.json", "ClientMessage"],
  ["cancelUpdateRequest", "cancel-update-request.json", "ClientMessage"],
  ["shutdownRequest", "shutdown-request.json", "ClientMessage"],
  ["shutdownResponse", "shutdown-response.json", "ServerMessage"],
  ["serverWelcome", "server-welcome.json", "ServerMessage"],
  ["runtimesResponse", "runtimes-response.json", "ServerMessage"],
  ["sessionToolApprovalEvent", "session-tool-approval-event.json", "ServerMessage"],
  ["versionMismatch", "version-mismatch.json", "ServerMessage"],
  ["serverResponse", "server-response.json", "ServerMessage"],
  ["unconfirmedHealthResponse", "unconfirmed-health-response.json", "ServerMessage"],
  ["migrationFailedHealthResponse", "migration-failed-health-response.json", "ServerMessage"],
  ["schemaMigrationFailedResponse", "schema-migration-failed-response.json", "ServerMessage"],
  ["sessionsResponse", "sessions-response.json", "ServerMessage"],
  ["sessionResponse", "session-response.json", "ServerMessage"],
  ["subscriptionResponse", "subscription-response.json", "ServerMessage"],
  ["resumedSubscriptionResponse", "resumed-subscription-response.json", "ServerMessage"],
  ["serverEvent", "server-event.json", "ServerMessage"],
  ["messageChangedEvent", "message-changed-event.json", "ServerMessage"],
  ["approvalResolvedEvent", "approval-resolved-event.json", "ServerMessage"],
  ["unknownResponse", "unknown-response.json", "ServerMessage"],
  ["updateResponse", "update-response.json", "ServerMessage"],
  ["updateChangedEvent", "update-changed-event.json", "ServerMessage"],
  ["ownerEnvironmentRequests", "owner-environment.json", "readonly ClientMessage[]", "requests"],
  ["ownerEnvironmentResponses", "owner-environment.json", "readonly ServerMessage[]", "responses"],
  ["piApprovalRequests", "../pi-rpc/approvals-wire-v5.json", "readonly ClientMessage[]", "requests"],
  ["piApprovalResponses", "../pi-rpc/approvals-wire-v5.json", "readonly ServerMessage[]", "responses"],
  ["piApprovalEvents", "../pi-rpc/approvals-wire-v5.json", "readonly ServerMessage[]", "events"],
  ["piCommandRequests", "../pi-rpc/commands-wire-v5.json", "readonly ClientMessage[]", "requests"],
  ["piCommandResponses", "../pi-rpc/commands-wire-v5.json", "readonly ServerMessage[]", "responses"],
  ["desktopProbeSessionId", "../pi-rpc/desktop-host-v5.json", "string", "sessionId"],
  ["desktopProbePrompt", "../pi-rpc/desktop-host-v5.json", "string", "prompt"],
  ["desktopProbeCancelPrompt", "../pi-rpc/desktop-host-v5.json", "string", "cancelPrompt"],
  ["desktopProbeExpectedText", "../pi-rpc/desktop-host-v5.json", "string", "expectedText"],
  ["modelListRequests", "../pi-rpc/model-list-wire-v5.json", "readonly ClientMessage[]", "requests"],
  ["modelListResponses", "../pi-rpc/model-list-wire-v5.json", "readonly ServerMessage[]", "responses"],
];
let source = "// Generated from fixtures/v2 by scripts/generate-protocol-fixtures.mjs. Do not edit.\n";
source += 'import type { ClientMessage, ServerMessage, SessionState } from "./protocol";\n\n';
for (const [name, file, type, key] of cases) {
  const parsed = JSON.parse(readFileSync(new URL(file, fixtureDir), "utf8"));
  const fixture = key ? parsed[key] : parsed;
  source += `export const ${name} = ${JSON.stringify(fixture, null, 2)} as const satisfies ${type};\n\n`;
}
source = `${source.trimEnd()}\n`;

if (process.argv.includes("--check")) {
  if (readFileSync(output, "utf8") !== source) {
    console.error("Generated TypeScript fixtures are stale; run pnpm generate:protocol-fixtures.");
    process.exitCode = 1;
  }
} else {
  writeFileSync(output, source);
}
