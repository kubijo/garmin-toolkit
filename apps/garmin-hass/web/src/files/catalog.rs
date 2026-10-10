//! Latest requested catalogue wins; stale completions cannot clear its loading state.
#[derive(Default)]
pub struct Request {
    loading: Option<String>,
    generation: u64,
}

impl Request {
    pub fn loading(&self) -> Option<&str> {
        self.loading.as_deref()
    }

    pub fn begin(&mut self, device: &str) -> Option<u64> {
        if self.loading() == Some(device) {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.loading = Some(device.into());
        Some(self.generation)
    }

    pub fn finish(&mut self, generation: u64) -> bool {
        if self.generation != generation || self.loading.is_none() {
            return false;
        }
        self.loading = None;
        true
    }

    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.loading = None;
    }
}
