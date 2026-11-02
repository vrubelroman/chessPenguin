use serde::{Deserialize, Serialize};

/// A durable training area. The selected move is included; `end_prefix`, when present,
/// is also included and forms an early boundary. Node IDs never participate.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingScope {
    pub prefix: Vec<String>,
    pub end_prefix: Option<Vec<String>>,
    /// Replay the path before the selected move/target as automatic context.
    pub include_context: bool,
}

impl TrainingScope {
    pub fn whole() -> Self {
        Self::default()
    }

    pub fn from_prefix(prefix: Vec<String>) -> Self {
        Self {
            prefix,
            ..Self::default()
        }
    }

    pub fn between(prefix: Vec<String>, end_prefix: Vec<String>) -> Self {
        Self {
            prefix,
            end_prefix: Some(end_prefix),
            ..Self::default()
        }
    }

    pub fn with_context(mut self, include_context: bool) -> Self {
        self.include_context = include_context;
        self
    }

    pub fn key(&self) -> TrainingScopeKey {
        TrainingScopeKey {
            prefix: self.prefix.clone(),
            end_prefix: self.end_prefix.clone(),
        }
    }

    pub(crate) fn parent_prefix(&self) -> &[String] {
        self.prefix
            .get(..self.prefix.len().saturating_sub(1))
            .unwrap_or(&[])
    }
}

impl Default for TrainingScope {
    fn default() -> Self {
        Self {
            prefix: Vec::new(),
            end_prefix: None,
            include_context: true,
        }
    }
}

/// Identity of a training area for queues and history. Context presentation deliberately
/// does not split learning data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default)]
pub struct TrainingScopeKey {
    pub prefix: Vec<String>,
    pub end_prefix: Option<Vec<String>>,
}
