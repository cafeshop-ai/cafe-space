//! Update admission and connection-owned pending leases. Used under the host's state lock.

use cafe_space_protocol::{PrepareUpdateParams, UpdateMode, UpdateState, check_binary_hash};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Local,
    Remote,
}

#[derive(Clone, Debug)]
struct Lease {
    owner: u64,
    params: PrepareUpdateParams,
}

#[derive(Clone, Debug)]
pub struct Gate {
    current_hash: String,
    lease: Option<Lease>,
    state: UpdateState,
    // shortcut: M0 has no agents; M1 must coordinate the real registry and stop sessions here.
    active_sessions: usize,
}

impl Gate {
    pub fn new(current_hash: String) -> Result<Self, cafe_space_protocol::ProtocolError> {
        check_binary_hash(&current_hash)?;
        Ok(Self {
            current_hash,
            lease: None,
            state: UpdateState::None,
            active_sessions: 0,
        })
    }

    pub(crate) fn provisional(&mut self, active: bool) {
        debug_assert!(self.lease.is_none());
        self.state = if active {
            UpdateState::Draining
        } else {
            UpdateState::None
        };
    }

    pub fn state(&self) -> UpdateState {
        self.state
    }
    pub fn admits_work(&self) -> bool {
        self.state != UpdateState::Draining
    }
    pub fn target_hash(&self) -> Option<&str> {
        self.lease
            .as_ref()
            .map(|lease| lease.params.target_hash.as_str())
    }

    pub fn prepare(
        &mut self,
        access: Access,
        owner: u64,
        params: PrepareUpdateParams,
    ) -> Result<UpdateState, &'static str> {
        if access != Access::Local {
            return Err("LOCAL_ONLY");
        }
        check_binary_hash(&params.target_hash).map_err(|_| "INVALID_BINARY_HASH")?;
        if self.state == UpdateState::Draining {
            return Err("UPDATE_IN_PROGRESS");
        }
        if let Some(lease) = &self.lease {
            if lease.owner != owner || lease.params != params {
                return Err("UPDATE_ALREADY_PENDING");
            }
            return Ok(self.state);
        }
        if params.target_hash == self.current_hash {
            return Ok(UpdateState::None);
        }
        self.lease = Some(Lease { owner, params });
        self.state = UpdateState::Pending;
        self.advance();
        Ok(self.state)
    }

    pub fn cancel(&mut self, access: Access, owner: u64) -> Result<UpdateState, &'static str> {
        if access != Access::Local {
            return Err("LOCAL_ONLY");
        }
        if self.state == UpdateState::Draining {
            return Err("UPDATE_IN_PROGRESS");
        }
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.owner != owner)
        {
            return Err("UPDATE_LEASE_OWNED");
        }
        self.lease = None;
        self.state = UpdateState::None;
        Ok(self.state)
    }

    pub fn disconnect(&mut self, owner: u64) -> bool {
        if self.state == UpdateState::Pending
            && self
                .lease
                .as_ref()
                .is_some_and(|lease| lease.owner == owner)
        {
            self.lease = None;
            self.state = UpdateState::None;
            true
        } else {
            false
        }
    }

    fn advance(&mut self) {
        if self.state == UpdateState::Pending
            && self.lease.as_ref().is_some_and(|lease| {
                lease.params.mode == UpdateMode::Now || self.active_sessions == 0
            })
        {
            self.state = UpdateState::Draining;
            self.active_sessions = 0;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_active_for_test(&mut self, count: usize) {
        self.active_sessions = count;
        self.advance();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(mode: UpdateMode) -> PrepareUpdateParams {
        PrepareUpdateParams {
            mode,
            target_hash: "b".repeat(64),
        }
    }
    fn gate() -> Gate {
        Gate::new("a".repeat(64)).unwrap()
    }

    #[test]
    fn pending_lease_allows_work_and_only_its_connection_can_cancel() {
        let mut gate = gate();
        gate.set_active_for_test(1);
        assert_eq!(
            gate.prepare(Access::Local, 7, params(UpdateMode::WhenIdle)),
            Ok(UpdateState::Pending)
        );
        assert!(gate.admits_work());
        assert_eq!(
            gate.prepare(Access::Local, 7, params(UpdateMode::WhenIdle)),
            Ok(UpdateState::Pending)
        );
        assert_eq!(
            gate.prepare(Access::Local, 8, params(UpdateMode::WhenIdle)),
            Err("UPDATE_ALREADY_PENDING")
        );
        assert_eq!(gate.cancel(Access::Local, 8), Err("UPDATE_LEASE_OWNED"));
        assert!(!gate.disconnect(8));
        assert_eq!(gate.cancel(Access::Local, 7), Ok(UpdateState::None));
        assert_eq!(gate.target_hash(), None);
        assert!(gate.admits_work());
    }

    #[test]
    fn losing_pending_connection_cancels_without_stopping_sessions() {
        let mut gate = gate();
        gate.set_active_for_test(2);
        gate.prepare(Access::Local, 7, params(UpdateMode::WhenIdle))
            .unwrap();
        assert!(gate.disconnect(7));
        assert_eq!(gate.state(), UpdateState::None);
        assert_eq!(gate.active_sessions, 2);
        gate.set_active_for_test(0);
        assert_eq!(gate.state(), UpdateState::None);
    }

    #[test]
    fn idle_transition_freezes_admission_and_cannot_be_cancelled_or_lost() {
        let mut gate = gate();
        gate.set_active_for_test(1);
        gate.prepare(Access::Local, 7, params(UpdateMode::WhenIdle))
            .unwrap();
        gate.set_active_for_test(0);
        assert_eq!(gate.state(), UpdateState::Draining);
        assert!(!gate.admits_work());
        assert_eq!(gate.cancel(Access::Local, 7), Err("UPDATE_IN_PROGRESS"));
        assert!(!gate.disconnect(7));
        assert_eq!(gate.target_hash(), Some("b".repeat(64).as_str()));
        assert_eq!(
            gate.prepare(Access::Local, 8, params(UpdateMode::Now)),
            Err("UPDATE_IN_PROGRESS")
        );
    }

    #[test]
    fn now_stops_synthetic_work_and_remote_requests_never_mutate_the_gate() {
        let mut gate = gate();
        gate.set_active_for_test(2);
        assert_eq!(
            gate.prepare(Access::Remote, 7, params(UpdateMode::Now)),
            Err("LOCAL_ONLY")
        );
        assert_eq!(gate.cancel(Access::Remote, 7), Err("LOCAL_ONLY"));
        assert_eq!(gate.state(), UpdateState::None);
        assert_eq!(gate.active_sessions, 2);
        assert_eq!(
            gate.prepare(Access::Local, 7, params(UpdateMode::Now)),
            Ok(UpdateState::Draining)
        );
        assert_eq!(gate.active_sessions, 0);
        assert!(!gate.admits_work());
    }

    #[test]
    fn idle_when_idle_drains_immediately_and_matching_binary_does_not_restart() {
        let mut first = gate();
        assert_eq!(
            first.prepare(Access::Local, 1, params(UpdateMode::WhenIdle)),
            Ok(UpdateState::Draining)
        );
        let mut second = gate();
        assert_eq!(
            second.prepare(
                Access::Local,
                1,
                PrepareUpdateParams {
                    mode: UpdateMode::Now,
                    target_hash: "a".repeat(64)
                }
            ),
            Ok(UpdateState::None)
        );
        assert!(second.admits_work());
        assert_eq!(second.target_hash(), None);
        assert!(
            second
                .prepare(
                    Access::Local,
                    1,
                    PrepareUpdateParams {
                        mode: UpdateMode::Now,
                        target_hash: "bad".into()
                    }
                )
                .is_err()
        );
        assert_eq!(second.state(), UpdateState::None);
    }
}
