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
}
