// Generated from fixtures/v2 by scripts/generate-protocol-fixtures.mjs. Do not edit.
import type { ClientMessage, ServerMessage } from "./protocol";

export const clientHello = {
  "type": "hello",
  "supportedProtocolVersions": [
    2
  ],
  "clientVersion": "0.1.0-dev"
} as const satisfies ClientMessage;

export const clientRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-list-sessions",
  "request": {
    "method": "list_sessions"
  }
} as const satisfies ClientMessage;

export const createSessionRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-create-session",
  "request": {
    "method": "create_session",
    "params": {
      "sessionId": "session-fixture",
      "projectId": "project-fixture",
      "runtimeId": "pi",
      "expectedCwdFence": "cwd-fixture",
      "start": {
        "mode": "new"
      }
    }
  }
} as const satisfies ClientMessage;

export const approvalRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-approval",
  "request": {
    "method": "respond_to_approval",
    "params": {
      "sessionId": "session-fixture",
      "expectedStreamId": "stream-fixture",
      "expectedCwdFence": "cwd-fixture",
      "approvalId": "approval-fixture",
      "decision": "allow_for_session"
    }
  }
} as const satisfies ClientMessage;

export const setModelRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-set-model",
  "request": {
    "method": "set_model",
    "params": {
      "sessionId": "session-fixture",
      "expectedStreamId": "stream-fixture",
      "expectedCwdFence": "cwd-fixture",
      "provider": "fixture-provider",
      "modelId": "fixture-model"
    }
  }
} as const satisfies ClientMessage;

export const setThinkingRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-set-thinking",
  "request": {
    "method": "set_thinking",
    "params": {
      "sessionId": "session-fixture",
      "expectedStreamId": "stream-fixture",
      "expectedCwdFence": "cwd-fixture",
      "level": "medium"
    }
  }
} as const satisfies ClientMessage;

export const subscribeEventsRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-resume-events",
  "request": {
    "method": "subscribe_events",
    "params": {
      "after": {
        "eventStreamId": "daemon-fixture",
        "sequence": 12
      }
    }
  }
} as const satisfies ClientMessage;

export const subscribeSnapshotRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-snapshot-events",
  "request": {
    "method": "subscribe_events",
    "params": {}
  }
} as const satisfies ClientMessage;

export const prepareUpdateRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-prepare-update",
  "request": {
    "method": "prepare_update",
    "params": {
      "mode": "when_idle",
      "targetHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
} as const satisfies ClientMessage;

export const cancelUpdateRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-cancel-update",
  "request": {
    "method": "cancel_update"
  }
} as const satisfies ClientMessage;

export const serverWelcome = {
  "type": "welcome",
  "protocolVersion": 2,
  "daemonVersion": "0.1.0-dev"
} as const satisfies ServerMessage;

export const versionMismatch = {
  "type": "version_mismatch",
  "daemonVersion": "0.1.0-dev",
  "supportedProtocolVersions": [
    2
  ]
} as const satisfies ServerMessage;

export const serverResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-list-sessions",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "sessions",
        "data": {
          "items": [],
          "eventStreamId": "daemon-fixture",
          "sequence": 0
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const unconfirmedHealthResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "update-health",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "health",
        "data": {
          "ready": false,
          "daemonVersion": "0.1.0",
          "update": "draining",
          "binaryHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          "eventStreamId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          "awaitingUpdateConfirmation": true
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const sessionsResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-list-sessions",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "sessions",
        "data": {
          "items": [
            {
              "id": "session-fixture",
              "projectId": "project-fixture",
              "runtimeId": "pi",
              "cwdFence": "cwd-fixture",
              "model": {
                "provider": "fixture-provider",
                "id": "fixture-model"
              },
              "thinkingLevel": "medium",
              "capabilities": {
                "modelControl": true,
                "thinkingControl": true,
                "deliveryControl": true,
                "sessionControl": true,
                "approvalForSession": true
              },
              "mode": "new",
              "state": {
                "state": "ready"
              },
              "streamId": "stream-fixture",
              "title": "Fixture session"
            }
          ],
          "eventStreamId": "daemon-fixture",
          "sequence": 0
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const sessionResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-get-session",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "session",
        "data": {
          "session": {
            "session": {
              "id": "session-fixture",
              "projectId": "project-fixture",
              "runtimeId": "pi",
              "cwdFence": "cwd-fixture",
              "model": {
                "provider": "fixture-provider",
                "id": "fixture-model"
              },
              "thinkingLevel": "medium",
              "capabilities": {
                "modelControl": true,
                "thinkingControl": true,
                "deliveryControl": true,
                "sessionControl": true,
                "approvalForSession": true
              },
              "mode": "new",
              "state": {
                "state": "ready"
              },
              "streamId": "stream-fixture",
              "title": "Fixture session"
            },
            "messages": [
              {
                "id": "message-fixture",
                "role": "assistant",
                "parts": [
                  {
                    "kind": "text",
                    "data": {
                      "text": "Fixture response"
                    }
                  }
                ],
                "status": "complete"
              }
            ],
            "historyTruncated": false,
            "approvals": []
          },
          "eventStreamId": "daemon-fixture",
          "sequence": 1
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const subscriptionResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-subscribe-events",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "subscription",
        "data": {
          "eventStreamId": "daemon-fixture",
          "sequence": 0,
          "mode": "snapshot_required",
          "snapshot": {
            "update": "none",
            "projects": [],
            "runtimes": [],
            "sessions": []
          }
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const resumedSubscriptionResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-resume-events",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "subscription",
        "data": {
          "eventStreamId": "daemon-fixture",
          "sequence": 15,
          "mode": "resumed"
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const serverEvent = {
  "type": "event",
  "protocolVersion": 2,
  "eventStreamId": "daemon-fixture",
  "sequence": 1,
  "event": {
    "kind": "session_changed",
    "data": {
      "session": {
        "id": "session-fixture",
        "projectId": "project-fixture",
        "runtimeId": "pi",
        "cwdFence": "cwd-fixture",
        "model": {
          "provider": "fixture-provider",
          "id": "fixture-model"
        },
        "thinkingLevel": "medium",
        "capabilities": {
          "modelControl": true,
          "thinkingControl": true,
          "deliveryControl": true,
          "sessionControl": true,
          "approvalForSession": true
        },
        "mode": "new",
        "state": {
          "state": "running"
        },
        "activeRunId": "run-fixture",
        "streamId": "stream-fixture",
        "title": "Fixture session"
      }
    }
  }
} as const satisfies ServerMessage;

export const messageChangedEvent = {
  "type": "event",
  "protocolVersion": 2,
  "eventStreamId": "daemon-fixture",
  "sequence": 2,
  "event": {
    "kind": "message_changed",
    "data": {
      "sessionId": "session-fixture",
      "message": {
        "id": "message-fixture",
        "role": "assistant",
        "parts": [
          {
            "kind": "text",
            "data": {
              "text": "Fixture response"
            }
          }
        ],
        "status": "complete"
      }
    }
  }
} as const satisfies ServerMessage;

export const approvalResolvedEvent = {
  "type": "event",
  "protocolVersion": 2,
  "eventStreamId": "daemon-fixture",
  "sequence": 2,
  "event": {
    "kind": "approval_resolved",
    "data": {
      "sessionId": "session-fixture",
      "approvalId": "approval-fixture",
      "decision": "allow_for_session"
    }
  }
} as const satisfies ServerMessage;

export const unknownResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-create-session",
  "result": {
    "status": "unknown",
    "data": {
      "error": {
        "code": "OUTCOME_UNKNOWN",
        "message": "reconcile by sessionId; do not retry the create request"
      }
    }
  }
} as const satisfies ServerMessage;

export const updateResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-prepare-update",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "update",
        "data": {
          "update": "pending"
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const updateChangedEvent = {
  "type": "event",
  "protocolVersion": 2,
  "eventStreamId": "daemon-fixture",
  "sequence": 1,
  "event": {
    "kind": "update_changed",
    "data": {
      "update": "pending"
    }
  }
} as const satisfies ServerMessage;
