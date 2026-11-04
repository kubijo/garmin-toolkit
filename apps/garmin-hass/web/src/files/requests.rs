//! Admission and invalidation for requests owned by a file window.
use super::catalog;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationToken {
    connection_generation: u64,
    operation: u64,
}

#[derive(Default)]
pub struct Requests {
    pub catalog: catalog::Request,
    active: Option<OperationToken>,
    next_operation: u64,
}

impl Requests {
    pub fn active(&self) -> Option<OperationToken> {
        self.active
    }

    pub fn begin(&mut self, connection_generation: u64) -> Option<OperationToken> {
        if self.active.is_some() {
            return None;
        }
        let token = OperationToken {
            connection_generation,
            operation: self.next_operation,
        };
        self.next_operation = self.next_operation.wrapping_add(1);
        self.active = Some(token);
        Some(token)
    }

    pub fn finish(&mut self, token: OperationToken, connection_generation: u64) -> bool {
        if self.active != Some(token) || token.connection_generation != connection_generation {
            return false;
        }
        self.active = None;
        true
    }

    pub fn interrupt(&mut self) -> bool {
        self.active.take().is_some()
    }

    pub fn close(&mut self) {
        self.catalog.invalidate();
        self.interrupt();
    }
}
