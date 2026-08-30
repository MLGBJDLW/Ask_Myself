use super::BrowserControlOwner;

/// Generation-based ownership lease shared by UI and Agent clients.
#[derive(Debug, Clone, Default)]
pub struct ControlLease {
    owner: BrowserControlOwner,
    generation: u64,
}

impl ControlLease {
    pub fn owner(&self) -> &BrowserControlOwner {
        &self.owner
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn acquire(&mut self, owner: BrowserControlOwner) {
        let owner_kind_changed = !matches!(
            (&self.owner, &owner),
            (
                BrowserControlOwner::Agent { .. },
                BrowserControlOwner::Agent { .. }
            ) | (BrowserControlOwner::User, BrowserControlOwner::User)
                | (BrowserControlOwner::None, BrowserControlOwner::None)
        );
        if owner_kind_changed {
            self.generation = self.generation.saturating_add(1);
        }
        self.owner = owner;
    }

    /// Acquires the lease for an Agent only when the user has explicitly
    /// released control. Agent calls may refresh an existing Agent lease, but
    /// they must never use a read operation to take control back from a user.
    pub fn try_acquire_agent(&mut self, call_id: String) -> bool {
        if matches!(self.owner, BrowserControlOwner::User) {
            return false;
        }
        self.acquire(BrowserControlOwner::Agent { call_id });
        true
    }

    /// Invalidate every observation and pending commit tied to the current
    /// generation without changing who owns control.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }

    pub fn release(&mut self) {
        self.acquire(BrowserControlOwner::None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeover_changes_generation_but_same_owner_kind_does_not() {
        let mut lease = ControlLease::default();
        lease.acquire(BrowserControlOwner::Agent {
            call_id: "one".into(),
        });
        let agent_generation = lease.generation();
        lease.acquire(BrowserControlOwner::Agent {
            call_id: "two".into(),
        });
        assert_eq!(lease.generation(), agent_generation);
        lease.acquire(BrowserControlOwner::User);
        assert!(lease.generation() > agent_generation);
    }

    #[test]
    fn agent_cannot_take_control_back_from_the_user() {
        let mut lease = ControlLease::default();
        lease.acquire(BrowserControlOwner::User);

        assert!(!lease.try_acquire_agent("agent-call".into()));
        assert!(matches!(lease.owner(), BrowserControlOwner::User));
    }

    #[test]
    fn invalidation_advances_generation_without_changing_owner() {
        let mut lease = ControlLease::default();
        lease.acquire(BrowserControlOwner::Agent {
            call_id: "agent-call".into(),
        });
        let generation = lease.generation();

        lease.invalidate();

        assert!(lease.generation() > generation);
        assert!(matches!(
            lease.owner(),
            BrowserControlOwner::Agent { call_id } if call_id == "agent-call"
        ));
    }
}
