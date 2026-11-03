import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const fixtureDir = new URL("../../fixtures/v2/", import.meta.url);
const output = fileURLToPath(new URL("../src/protocol-fixtures.generated.ts", import.meta.url));
const cases = [
  ["clientHello", "client-hello.json", "ClientMessage"],
  ["clientRequest", "client-request.json", "ClientMessage"],
  ["createSessionRequest", "create-session-request.json", "ClientMessage"],
  ["setModelRequest", "set-model-request.json", "ClientMessage"],
  ["setThinkingRequest", "set-thinking-request.json", "ClientMessage"],
  ["subscribeEventsRequest", "subscribe-events-request.json", "ClientMessage"],
  ["subscribeSnapshotRequest", "subscribe-snapshot-request.json", "ClientMessage"],
  ["serverWelcome", "server-welcome.json", "ServerMessage"],
  ["versionMismatch", "version-mismatch.json", "ServerMessage"],
  ["serverResponse", "server-response.json", "ServerMessage"],
  ["sessionsResponse", "sessions-response.json", "ServerMessage"],
  ["sessionResponse", "session-response.json", "ServerMessage"],
  ["subscriptionResponse", "subscription-response.json", "ServerMessage"],
  ["resumedSubscriptionResponse", "resumed-subscription-response.json", "ServerMessage"],
  ["serverEvent", "server-event.json", "ServerMessage"],
  ["messageChangedEvent", "message-changed-event.json", "ServerMessage"],
  ["unknownResponse", "unknown-response.json", "ServerMessage"],
];
let source = "// Generated from fixtures/v2 by scripts/generate-protocol-fixtures.mjs. Do not edit.\n";
source += 'import type { ClientMessage, ServerMessage } from "./protocol";\n\n';
for (const [name, file, type] of cases) {
  const fixture = JSON.parse(readFileSync(new URL(file, fixtureDir), "utf8"));
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
