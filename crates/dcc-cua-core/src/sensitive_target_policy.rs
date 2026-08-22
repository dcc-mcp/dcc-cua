use crate::contracts::{ComputerUseError, ComputerUseErrorCode, ComputerUseResult};

// These shared rules are defense in depth, not an exhaustive classification of
// every sensitive application. The hard boundary is that an observed target
// without a verifiable application identity is always rejected.
const SENSITIVE_IDENTITY_MARKERS: &[&str] = &[
    "password",
    "credential",
    "authentication",
    "sign in",
    "login",
    "terminal",
    "command prompt",
    "security",
    "consent",
];

const RESTRICTED_EXECUTABLE_STEMS: &[&str] = &[
    "alacritty",
    "bash",
    "cmd",
    "conhost",
    "consent",
    "credentialuibroker",
    "cscript",
    "fish",
    "gnome-terminal",
    "gnome-terminal-server",
    "konsole",
    "logonui",
    "lsass",
    "mintty",
    "mshta",
    "node",
    "nodejs",
    "powershell",
    "pwsh",
    "regsvr32",
    "rundll32",
    "sh",
    "terminal",
    "wezterm",
    "winlogon",
    "windowsterminal",
    "wscript",
    "wsl",
    "wt",
    "xterm",
    "zsh",
];

pub(crate) fn validate_observed_application_identity(identity: &str) -> ComputerUseResult<()> {
    let identity = identity.trim();
    if identity.is_empty() || identity.eq_ignore_ascii_case("native-window") {
        return Err(invalid_target(
            "target application identity could not be verified",
        ));
    }
    validate_sensitive_application_identity(identity)
}

pub(crate) fn validate_launch_application_identity(identity: &str) -> ComputerUseResult<()> {
    let identity = identity.trim();
    if identity.is_empty() {
        return Err(invalid_target("launch application identity is empty"));
    }
    validate_sensitive_application_identity(identity)
}

fn validate_sensitive_application_identity(identity: &str) -> ComputerUseResult<()> {
    let lowercase = identity.to_ascii_lowercase();
    let stem = executable_stem(&lowercase);
    let compact = lowercase
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .collect::<String>();
    if SENSITIVE_IDENTITY_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
        || RESTRICTED_EXECUTABLE_STEMS.contains(&stem)
        || compact.contains("windowsterminal")
        || compact.contains("credentialuibroker")
        || is_python_interpreter(stem)
    {
        return Err(invalid_target(
            "system, terminal, command interpreter, authentication, password, and security applications are not allowed",
        ));
    }
    Ok(())
}

fn executable_stem(identity: &str) -> &str {
    let leaf = identity
        .trim_matches(['\'', '"'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    leaf.strip_suffix(".exe")
        .or_else(|| leaf.strip_suffix(".app"))
        .unwrap_or(leaf)
}

fn is_python_interpreter(stem: &str) -> bool {
    let Some(version) = stem.strip_prefix("python") else {
        return false;
    };
    version.is_empty()
        || version == "w"
        || version
            .strip_prefix('w')
            .unwrap_or(version)
            .chars()
            .all(|value| value.is_ascii_digit() || value == '.')
}

fn invalid_target(message: &'static str) -> ComputerUseError {
    ComputerUseError::new(ComputerUseErrorCode::InvalidTarget, message)
}
