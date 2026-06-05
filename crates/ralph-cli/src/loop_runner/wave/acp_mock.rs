#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
use ralph_adapters::CliBackend;

/// Result of executing a single ACP wave worker prompt.
pub enum AcpWaveExecutionResult {
    Completed(std::result::Result<bool, String>),
    TimedOut,
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub enum MockAcpExecution {
    Success {
        success: bool,
        events: Vec<ralph_core::Event>,
    },
    Error {
        error: String,
        events: Vec<ralph_core::Event>,
    },
    Timeout {
        events: Vec<ralph_core::Event>,
    },
}

#[cfg(test)]
impl MockAcpExecution {
    pub fn success(success: bool, events: Vec<ralph_core::Event>) -> Self {
        Self::Success { success, events }
    }

    pub fn error(error: impl Into<String>, events: Vec<ralph_core::Event>) -> Self {
        Self::Error {
            error: error.into(),
            events,
        }
    }

    pub fn timeout(events: Vec<ralph_core::Event>) -> Self {
        Self::Timeout { events }
    }

    pub fn write_capture(
        &self,
        worker_backend: &CliBackend,
        prompt: &str,
        worker_events_path: &Path,
    ) {
        let capture_path = PathBuf::from(format!("{}.capture", worker_events_path.display()));
        let env = worker_backend
            .env_vars
            .iter()
            .cloned()
            .collect::<BTreeMap<_, _>>();
        let capture = serde_json::json!({
            "command": worker_backend.command.as_str(),
            "args": &worker_backend.args,
            "env": env,
            "prompt": prompt,
        });
        fs::write(
            &capture_path,
            serde_json::to_string(&capture).expect("serialize mock ACP invocation"),
        )
        .expect("write mock ACP invocation");
    }

    pub fn write_events(&self, worker_events_path: &Path) {
        let events = match self {
            Self::Success { events, .. }
            | Self::Error { events, .. }
            | Self::Timeout { events } => events,
        };

        if events.is_empty() {
            let _ = fs::remove_file(worker_events_path);
            return;
        }

        let content = events
            .iter()
            .map(|event| serde_json::to_string(event).expect("serialize mock ACP event"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(worker_events_path, format!("{content}\n")).expect("write mock ACP events");
    }
}

#[cfg(test)]
pub static MOCK_ACP_EXECUTIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::VecDeque<MockAcpExecution>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::VecDeque::new()));

#[cfg(test)]
pub static MOCK_ACP_EXECUTION_SERIAL: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
