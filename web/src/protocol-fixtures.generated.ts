// Generated from fixtures/v2 by scripts/generate-protocol-fixtures.mjs. Do not edit.
import type { ClientMessage, ServerMessage, SessionState } from "./protocol";

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

export const projectRequests = [
  {
    "type": "request",
    "protocolVersion": 2,
    "requestId": "project-add",
    "request": {
      "method": "add_project",
      "params": {
        "name": "Fixture 茶",
        "root": "/synthetic/project-root"
      }
    }
  },
  {
    "type": "request",
    "protocolVersion": 2,
    "requestId": "project-add-windows",
    "request": {
      "method": "add_project",
      "params": {
        "name": "Windows fixture",
        "root": "C:\\synthetic\\project-root"
      }
    }
  },
  {
    "type": "request",
    "protocolVersion": 2,
    "requestId": "project-update",
    "request": {
      "method": "update_project",
      "params": {
        "projectId": "project-fixture",
        "expectedCwdFence": "cwd-fixture",
        "name": "Fixture 茶",
        "root": "/synthetic/new-root"
      }
    }
  },
  {
    "type": "request",
    "protocolVersion": 2,
    "requestId": "project-remove",
    "request": {
      "method": "remove_project",
      "params": {
        "projectId": "project-fixture",
        "expectedCwdFence": "cwd-next"
      }
    }
  },
  {
    "type": "request",
    "protocolVersion": 2,
    "requestId": "project-rename",
    "request": {
      "method": "update_project",
      "params": {
        "projectId": "project-fixture",
        "expectedCwdFence": "cwd-next",
        "name": "Renamed 茶"
      }
    }
  }
] as const satisfies readonly ClientMessage[];

export const projectEvents = [
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "project-events",
    "sequence": 1,
    "event": {
      "kind": "project_changed",
      "data": {
        "project": {
          "id": "project-fixture",
          "name": "Fixture 茶",
          "cwdFence": "cwd-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "project-events",
    "sequence": 2,
    "event": {
      "kind": "project_changed",
      "data": {
        "project": {
          "id": "project-fixture",
          "name": "Fixture 茶",
          "cwdFence": "cwd-next"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "project-events",
    "sequence": 3,
    "event": {
      "kind": "project_removed",
      "data": {
        "projectId": "project-fixture"
      }
    }
  }
] as const satisfies readonly ServerMessage[];

export const projectResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "project-add",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "project",
        "data": {
          "id": "project-fixture",
          "name": "Fixture 茶",
          "cwdFence": "cwd-fixture"
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const getProjectRootRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "project-root",
  "request": {
    "method": "get_project_root",
    "params": {
      "projectId": "project-fixture"
    }
  }
} as const satisfies ClientMessage;

export const projectRootResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "project-root",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "project_root",
        "data": {
          "projectId": "project-fixture",
          "cwdFence": "cwd-fixture",
          "root": "/synthetic/project-root"
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const projectLocalOnly = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "project-add",
  "result": {
    "status": "rejected",
    "data": {
      "error": {
        "code": "LOCAL_ONLY",
        "message": "This operation requires a local connection."
      }
    }
  }
} as const satisfies ServerMessage;

export const projectStaleFence = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "project-update",
  "result": {
    "status": "rejected",
    "data": {
      "error": {
        "code": "STALE_CWD_FENCE",
        "message": "Session context changed; read the current state before acting."
      }
    }
  }
} as const satisfies ServerMessage;

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

export const openSessionRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-open-session",
  "request": {
    "method": "open_session",
    "params": {
      "sessionId": "session-fixture",
      "expectedStreamId": "session-stream-fixture",
      "expectedCwdFence": "cwd-fixture"
    }
  }
} as const satisfies ClientMessage;

export const closeSessionRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "fixture-close-session",
  "request": {
    "method": "close_session",
    "params": {
      "sessionId": "session-fixture",
      "expectedStreamId": "session-stream-fixture",
      "expectedCwdFence": "cwd-fixture"
    }
  }
} as const satisfies ClientMessage;

export const closeSessionDispatched = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "fixture-close-session",
  "result": {
    "status": "dispatched",
    "data": {
      "operationId": "session-stream-fixture"
    }
  }
} as const satisfies ServerMessage;

export const sessionLifecycle = [
  {
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
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "starting"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 2,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "ready"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 3,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "running"
          },
          "activeRunId": "run-fixture",
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 4,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "ready"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 5,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "stopping"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 6,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "stopped"
          },
          "streamId": "session-stream-fixture",
          "stopReason": "SESSION_CLOSED"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 7,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-failed-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "starting"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 8,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-failed-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "failed",
            "data": {
              "code": "AGENT_CRASHED"
            }
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 9,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-restarted-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "starting"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 10,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-restarted-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "ready"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-fixture",
    "sequence": 11,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-restarted-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "running"
          },
          "activeRunId": "run-fixture",
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-restarted-fixture",
    "sequence": 1,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-restarted-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "stopped"
          },
          "streamId": "session-stream-fixture",
          "stopReason": "DAEMON_RESTARTED"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-restarted-fixture",
    "sequence": 2,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-exited-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "starting"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-restarted-fixture",
    "sequence": 3,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-exited-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "ready"
          },
          "streamId": "session-stream-fixture"
        }
      }
    }
  },
  {
    "type": "event",
    "protocolVersion": 2,
    "eventStreamId": "daemon-restarted-fixture",
    "sequence": 4,
    "event": {
      "kind": "session_changed",
      "data": {
        "session": {
          "id": "session-exited-fixture",
          "projectId": "project-fixture",
          "runtimeId": "pi",
          "cwdFence": "cwd-fixture",
          "capabilities": {
            "modelControl": false,
            "thinkingControl": false,
            "deliveryControl": false,
            "sessionControl": true,
            "approvalForSession": false
          },
          "mode": "new",
          "state": {
            "state": "stopped"
          },
          "streamId": "session-stream-fixture",
          "stopReason": "AGENT_EXITED"
        }
      }
    }
  }
] as const satisfies readonly ServerMessage[];

export const sessionTransitions = {
  "starting": [
    "ready",
    "waiting_for_local_ui",
    "stopping",
    "stopped",
    "failed"
  ],
  "ready": [
    "running",
    "waiting_for_local_ui",
    "stopping",
    "stopped",
    "failed"
  ],
  "running": [
    "ready",
    "waiting_for_local_ui",
    "stopping",
    "stopped",
    "failed"
  ],
  "waiting_for_local_ui": [
    "ready",
    "running",
    "stopping",
    "stopped",
    "failed"
  ],
  "stopping": [
    "stopped",
    "failed"
  ],
  "stopped": [],
  "failed": []
} as const satisfies Readonly<Record<SessionState["state"], readonly SessionState["state"][]>>;

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

export const shutdownRequest = {
  "type": "request",
  "protocolVersion": 2,
  "requestId": "owner-stop",
  "request": {
    "method": "shutdown"
  }
} as const satisfies ClientMessage;

export const shutdownResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "owner-stop",
  "result": {
    "status": "applied",
    "data": {
      "result": {
        "kind": "shutdown_accepted"
      }
    }
  }
} as const satisfies ServerMessage;

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

export const migrationFailedHealthResponse = {
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
          "eventStreamId": "boot-fixture"
        }
      }
    }
  }
} as const satisfies ServerMessage;

export const schemaMigrationFailedResponse = {
  "type": "response",
  "protocolVersion": 2,
  "requestId": "migration-work",
  "result": {
    "status": "rejected",
    "data": {
      "error": {
        "code": "SCHEMA_MIGRATION_FAILED",
        "message": "Schema migration failed; local recovery is required."
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
