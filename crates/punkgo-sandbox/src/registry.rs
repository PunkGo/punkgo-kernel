use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::ExecutionBackend;

/// Registry of available execution backends.
///
/// The Kernel registers backends at startup and resolves them
/// per-actor at execute time. If an actor does not specify a
/// backend, the default ("process") is used.
pub struct BackendRegistry {
    backends: HashMap<String, Arc<dyn ExecutionBackend>>,
    default_name: String,
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
            default_name: "process".to_string(),
        }
    }

    pub fn register(&mut self, backend: Arc<dyn ExecutionBackend>) {
        let name = backend.name().to_string();
        self.backends.insert(name, backend);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ExecutionBackend>> {
        self.backends.get(name)
    }

    pub fn default_backend(&self) -> Option<&Arc<dyn ExecutionBackend>> {
        self.backends.get(&self.default_name)
    }

    pub fn set_default(&mut self, name: &str) {
        self.default_name = name.to_string();
    }

    pub fn list_backends(&self) -> Vec<&str> {
        self.backends.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for BackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}
