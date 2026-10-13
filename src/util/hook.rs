use log::{error, info, warn};
use std::process::Command;

/// Execute a shell command asynchronously in a blocking worker thread.
pub async fn run_hook(cmd: &str, hook_name: &str) -> bool {
    let cmd = cmd.to_string();
    let hook = hook_name.to_string();
    tokio::task::spawn_blocking(move || {
        info!("Executing {} hook: {}", hook, cmd);
        #[cfg(unix)]
        let res = Command::new("sh").arg("-c").arg(&cmd).status();
        #[cfg(windows)]
        let res = Command::new("cmd").arg("/C").arg(&cmd).status();

        match res {
            Ok(status) => {
                if !status.success() {
                    warn!(
                        "{} hook '{}' exited with status: {:?}",
                        hook,
                        cmd,
                        status.code()
                    );
                    false
                } else {
                    info!("{} hook '{}' completed successfully", hook, cmd);
                    true
                }
            }
            Err(e) => {
                error!("Failed to execute {} hook '{}': {}", hook, cmd, e);
                false
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Retrieve the command string for a hook ("up" or "down") from the listener config.
/// Checks `listener.up` / `listener.down` first, then falls back to `listener.options["up"]` / `listener.options["down"]`.
pub fn get_hook_cmd(listener: &crate::def::config::Listener, name: &str) -> Option<String> {
    if name == "up" {
        if let Some(ref cmd) = listener.up {
            let trimmed = cmd.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    } else if name == "down" {
        if let Some(ref cmd) = listener.down {
            let trimmed = cmd.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    if let Some(ref options) = listener.options {
        if let Some(val) = options.get(name) {
            if let Some(cmd) = val.as_str() {
                let trimmed = cmd.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// RAII Guard to ensure the down command is executed when the listener terminates,
/// even in case of panics, unhandled cancellations, or errors.
pub struct DownHookGuard {
    down_cmd: Option<String>,
}

impl DownHookGuard {
    pub fn new(down_cmd: Option<String>) -> Self {
        Self { down_cmd }
    }

    /// Defuse the guard so `drop()` will not run the command again.
    /// Returns the down command string if one was configured.
    pub fn defuse(&mut self) -> Option<String> {
        self.down_cmd.take()
    }
}

impl Drop for DownHookGuard {
    fn drop(&mut self) {
        if let Some(cmd) = self.down_cmd.take() {
            info!("Executing down hook on cleanup guard drop: {}", cmd);
            #[cfg(unix)]
            let res = Command::new("sh").arg("-c").arg(&cmd).status();
            #[cfg(windows)]
            let res = Command::new("cmd").arg("/C").arg(&cmd).status();

            match res {
                Ok(status) => {
                    if !status.success() {
                        warn!(
                            "down hook '{}' exited with status: {:?}",
                            cmd,
                            status.code()
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to execute down hook '{}': {}", cmd, e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_run_hook_echo() {
        let success = run_hook("echo test_run_hook", "test").await;
        assert!(success);
    }

    #[test]
    fn test_get_hook_cmd_priority() {
        let mut listener = crate::def::config::Listener {
            endpoint: "127.0.0.1:12345".to_string(),
            name: "test".to_string(),
            user: None,
            pw: None,
            proto: "tproxy".to_string(),
            router: "default".to_string(),
            up: Some("up_direct".to_string()),
            down: None,
            options: None,
        };

        assert_eq!(get_hook_cmd(&listener, "up"), Some("up_direct".to_string()));
        assert_eq!(get_hook_cmd(&listener, "down"), None);

        let mut opts = HashMap::new();
        opts.insert("down".to_string(), toml::Value::String("down_opts".to_string()));
        listener.options = Some(opts);
        assert_eq!(get_hook_cmd(&listener, "down"), Some("down_opts".to_string()));
    }
}
