use super::*;
use agent_client_protocol::v1::Responder;
use cafe_space_protocol::ApprovalDecision;

pub(super) struct Pending {
    pub responder: Responder<RequestPermissionResponse>,
    options: Vec<PermissionOption>,
}

impl Shared {
    pub(super) async fn permission(
        &self,
        request: RequestPermissionRequest,
        responder: Responder<RequestPermissionResponse>,
    ) -> Result<()> {
        if request.options.is_empty() || request.options.len() > MAX_PERMISSIONS {
            return Err(self.fail("ACP_PERMISSION_INVALID"));
        }
        for (index, option) in request.options.iter().enumerate() {
            valid_id(&option.option_id.to_string())
                .map_err(|_| self.fail("ACP_PERMISSION_INVALID"))?;
            if request.options[..index]
                .iter()
                .any(|other| other.option_id == option.option_id)
            {
                return Err(self.fail("ACP_PERMISSION_INVALID"));
            }
        }
        // Reserve before publishing the pending entry, so cancel cannot overtake its UI event.
        let permit = self
            .events
            .reserve()
            .await
            .map_err(|_| self.fail("ACP_OBSERVER_CLOSED"))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| self.fail("ACP_STATE_UNAVAILABLE"))?;
        if state.native_id.as_ref() != Some(&request.session_id) {
            return Err(self.fail("ACP_SESSION_MISMATCH"));
        }
        if !self.running.load(Ordering::Acquire) {
            return Err(self.fail("ACP_PERMISSION_UNEXPECTED"));
        }
        if self.cancelling.load(Ordering::Acquire) {
            return responder
                .respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
                .map_err(|_| self.fail("ACP_APPROVAL_OUTCOME_UNKNOWN"));
        }
        if state.permissions.len() >= MAX_PERMISSIONS
            || state
                .permissions
                .values()
                .any(|pending| pending.responder.id() == responder.id())
        {
            return Err(self.fail("ACP_PERMISSION_LIMIT"));
        }
        state.next_permission = state
            .next_permission
            .checked_add(1)
            .ok_or_else(|| self.fail("ACP_COUNTER_EXHAUSTED"))?;
        let id = state.next_permission;
        state.permissions.insert(
            id,
            Pending {
                responder,
                options: request.options.clone(),
            },
        );
        permit.send(Event::Permission {
            id,
            tool: request.tool_call,
            options: request.options,
        });
        Ok(())
    }
    pub(super) async fn approve(&self, id: u64, decision: ApprovalDecision) -> Result<()> {
        if let Some(error) = *self.failure.borrow() {
            return Err(error);
        }
        let permission;
        let outcome;
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| self.fail("ACP_STATE_UNAVAILABLE"))?;
            let pending = state
                .permissions
                .get(&id)
                .ok_or(Error("ACP_APPROVAL_NOT_FOUND"))?;
            let kind = match decision {
                ApprovalDecision::AllowOnce => PermissionOptionKind::AllowOnce,
                ApprovalDecision::Deny => PermissionOptionKind::RejectOnce,
                _ => return Err(Error("ACP_APPROVAL_UNSUPPORTED")),
            };
            let option = pending.options.iter().find(|option| option.kind == kind);
            outcome = match option {
                Some(option) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    option.option_id.clone(),
                )),
                None if kind == PermissionOptionKind::RejectOnce => {
                    RequestPermissionOutcome::Cancelled
                }
                None => return Err(Error("ACP_APPROVAL_UNSUPPORTED")),
            };
            permission = state
                .permissions
                .remove(&id)
                .ok_or(Error("ACP_APPROVAL_NOT_FOUND"))?;
        }
        permission
            .responder
            .respond(RequestPermissionResponse::new(outcome))
            .map_err(|_| self.fail("ACP_APPROVAL_OUTCOME_UNKNOWN"))?;
        self.emit(Event::PermissionClosed(id)).await
    }
}
