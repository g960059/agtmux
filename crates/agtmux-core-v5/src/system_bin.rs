use std::collections::HashMap;
use std::path::Path;

/// Resolve `ps` for app-child environments where PATH may not include `/bin`.
#[must_use]
pub fn resolve_ps_bin() -> String {
    resolve_system_bin("ps", &["/bin/ps", "/usr/bin/ps"])
}

/// Resolve `lsof` for app-child environments where PATH may not include `/usr/sbin`.
#[must_use]
pub fn resolve_lsof_bin() -> String {
    resolve_system_bin("lsof", &["/usr/sbin/lsof", "/usr/bin/lsof"])
}

fn resolve_system_bin(command: &str, fallbacks: &[&str]) -> String {
    let env = std::env::vars().collect::<HashMap<_, _>>();
    resolve_system_bin_from_env_with(command, fallbacks, &env, is_executable_path)
}

fn resolve_system_bin_from_env_with<F>(
    command: &str,
    fallbacks: &[&str],
    env: &HashMap<String, String>,
    is_executable: F,
) -> String
where
    F: Fn(&str) -> bool,
{
    if let Some(path) = env
        .get("PATH")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        for dir in path.split(':').filter(|segment| !segment.is_empty()) {
            let candidate = format!("{dir}/{command}");
            if is_executable(&candidate) {
                return candidate;
            }
        }
    }

    for candidate in fallbacks {
        if is_executable(candidate) {
            return (*candidate).to_string();
        }
    }

    command.to_string()
}

fn is_executable_path(path: &str) -> bool {
    let path = Path::new(path);
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            return metadata.permissions().mode() & 0o111 != 0;
        }
        false
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_ps_bin_prefers_path_lookup() {
        let env = HashMap::from([("PATH".to_string(), "/custom/bin:/usr/bin:/bin".to_string())]);

        let resolved = resolve_system_bin_from_env_with("ps", &["/bin/ps"], &env, |candidate| {
            candidate == "/custom/bin/ps"
        });
        assert_eq!(resolved, "/custom/bin/ps");
    }

    #[test]
    fn resolve_ps_bin_falls_back_to_standard_location_when_path_is_stripped() {
        let env = HashMap::from([("PATH".to_string(), "/opt/homebrew/bin".to_string())]);

        let resolved = resolve_system_bin_from_env_with("ps", &["/bin/ps"], &env, |candidate| {
            candidate == "/bin/ps"
        });
        assert_eq!(resolved, "/bin/ps");
    }

    #[test]
    fn resolve_lsof_bin_falls_back_to_standard_location_when_path_is_stripped() {
        let env = HashMap::from([("PATH".to_string(), "/opt/homebrew/bin".to_string())]);

        let resolved = resolve_system_bin_from_env_with(
            "lsof",
            &["/usr/sbin/lsof", "/usr/bin/lsof"],
            &env,
            |candidate| candidate == "/usr/sbin/lsof",
        );
        assert_eq!(resolved, "/usr/sbin/lsof");
    }

    #[test]
    fn resolve_system_bin_returns_bare_command_when_no_candidate_is_executable() {
        let env = HashMap::from([("PATH".to_string(), "/opt/homebrew/bin".to_string())]);

        let resolved =
            resolve_system_bin_from_env_with("ps", &["/bin/ps", "/usr/bin/ps"], &env, |_| false);
        assert_eq!(resolved, "ps");
    }
}
