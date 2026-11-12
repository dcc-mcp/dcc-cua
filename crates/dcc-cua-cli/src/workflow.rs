//! Host-startup-owned recipes. Model requests select an id, never a command.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dcc_cua_host::{
    TrustedTaskActionScope, TrustedTaskAuthorizationRegistration, TrustedTaskAuthorizationTarget,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_POLICY_BYTES: u64 = 1024 * 1024;
const MAX_RECIPES: usize = 32;
const MAX_RUNS: usize = 128;
const MAX_ACTIVE_RUNS: usize = 32;
const MAX_HASH_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowPolicy {
    version: u32,
    environment_id: String,
    session_id: String,
    expires_at_unix_ms: u64,
    recipes: Vec<Recipe>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    id: String,
    kind: WorkflowKind,
    executable: PathBuf,
    executable_sha256: String,
    arguments: Vec<String>,
    working_directory: PathBuf,
    artifact_root: PathBuf,
    result: ArtifactResult,
    allowed_exit_codes: Vec<i32>,
    timeout_ms: u64,
    allow_cancel: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    gui: bool,
    allowed_methods: Vec<String>,
    allowed_actions: Vec<TrustedTaskActionScope>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WorkflowKind {
    Install,
    Uninstall,
}

impl WorkflowKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactResult {
    relative_path: PathBuf,
    present: bool,
    #[serde(default)]
    sha256: Option<String>,
}

struct RunState {
    child: Child,
    // Keep the hash-verified executable open without write/delete sharing on Windows.
    _executable: Option<File>,
    _directories: Option<Arc<Vec<File>>>,
    recipe: Recipe,
    deadline: Instant,
    policy_expiry: u64,
    policy_deadline: Instant,
    state: &'static str,
    reason: Option<&'static str>,
    exit_code: Option<i32>,
    exit_observed: bool,
    result_verified: bool,
    cancellation_requested: bool,
    termination_reason: Option<&'static str>,
    verification: Option<Verification>,
}

struct Verification {
    receiver: std::sync::mpsc::Receiver<Result<bool, String>>,
    deadline: Instant,
}

pub(crate) struct WorkflowManager {
    policy: Option<WorkflowPolicy>,
    policy_deadline: Option<Instant>,
    runs: BTreeMap<String, Arc<Mutex<RunState>>>,
    consumed_uninstall: BTreeSet<String>,
}

impl WorkflowManager {
    pub(crate) fn disabled() -> Self {
        Self {
            policy: None,
            policy_deadline: None,
            runs: BTreeMap::new(),
            consumed_uninstall: BTreeSet::new(),
        }
    }

    pub(crate) fn from_policy_file(
        path: &Path,
        environment_id: &str,
        session_id: &str,
    ) -> Result<Self, String> {
        let loaded_at = Instant::now();
        let loaded_unix_ms = unix_ms();
        let file = File::open(path).map_err(|_| "workflow policy is unavailable")?;
        let mut bytes = Vec::new();
        file.take(MAX_POLICY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "workflow policy could not be read")?;
        if bytes.len() as u64 > MAX_POLICY_BYTES {
            return Err("workflow policy exceeds its size bound".into());
        }
        let mut policy: WorkflowPolicy = serde_json::from_slice(&bytes)
            .map_err(|_| "workflow policy does not match the closed schema")?;
        if policy.version != 1
            || !valid_id(&policy.environment_id)
            || !valid_id(&policy.session_id)
            || policy.environment_id != environment_id
            || policy.session_id != session_id
            || policy.expires_at_unix_ms <= loaded_unix_ms
            || policy.expires_at_unix_ms.saturating_sub(loaded_unix_ms) > 24 * 60 * 60 * 1000
            || policy.recipes.is_empty()
            || policy.recipes.len() > MAX_RECIPES
        {
            return Err("workflow policy binding, expiry, or bounds are invalid".into());
        }
        let mut ids = BTreeSet::new();
        for recipe in &mut policy.recipes {
            validate_recipe(recipe)?;
            if !ids.insert(recipe.id.clone()) {
                return Err("workflow policy recipe ids must be unique".into());
            }
        }
        let remaining_ms = policy.expires_at_unix_ms.saturating_sub(loaded_unix_ms);
        let policy_deadline = loaded_at + Duration::from_millis(remaining_ms);
        if Instant::now() >= policy_deadline || unix_ms() >= policy.expires_at_unix_ms {
            return Err("workflow policy has expired".into());
        }
        Ok(Self {
            policy_deadline: Some(policy_deadline),
            policy: Some(policy),
            runs: BTreeMap::new(),
            consumed_uninstall: BTreeSet::new(),
        })
    }

    pub(crate) fn environment_id(&self) -> Option<&str> {
        self.policy
            .as_ref()
            .map(|policy| policy.environment_id.as_str())
    }

    pub(crate) fn session_id(&self) -> Option<&str> {
        self.policy
            .as_ref()
            .map(|policy| policy.session_id.as_str())
    }

    pub(crate) fn expires_at_unix_ms(&self) -> Option<u64> {
        self.policy.as_ref().map(|policy| policy.expires_at_unix_ms)
    }

    pub(crate) fn recipe_authorizes(
        &self,
        recipe_id: &str,
        methods: &[String],
        actions: &[TrustedTaskActionScope],
    ) -> Result<(), String> {
        let recipe = self.recipe(recipe_id)?;
        if methods
            .iter()
            .any(|method| !recipe.allowed_methods.contains(method))
            || actions
                .iter()
                .any(|action| !recipe.allowed_actions.contains(action))
        {
            return Err("workflow request exceeds its trusted recipe scope".into());
        }
        Ok(())
    }

    pub(crate) fn gui_recipe_authorizes(
        &self,
        id: &str,
        methods: &[String],
        actions: &[TrustedTaskActionScope],
    ) -> Result<(), String> {
        self.recipe_authorizes(id, methods, actions)?;
        if !self.recipe(id)?.gui {
            return Err("MCP owned window tasks require a GUI recipe".into());
        }
        Ok(())
    }

    fn recipe(&self, id: &str) -> Result<&Recipe, String> {
        let policy = self
            .policy
            .as_ref()
            .ok_or("workflow policy is not configured")?;
        if unix_ms() >= policy.expires_at_unix_ms
            || self
                .policy_deadline
                .is_none_or(|deadline| Instant::now() >= deadline)
        {
            return Err("workflow policy has expired".into());
        }
        policy
            .recipes
            .iter()
            .find(|recipe| recipe.id == id)
            .ok_or_else(|| "workflow recipe is not authorized".into())
    }

    pub(crate) fn start(&mut self, recipe_id: &str) -> Result<Value, String> {
        if !cfg!(windows) {
            return Err("pinned native workflow launch is unsupported on this platform".into());
        }
        require_standard_user()?;
        let recipe = self.recipe(recipe_id)?.clone();
        if self.consumed_uninstall.contains(recipe_id) {
            return Err("the normal uninstall recipe has already been consumed".into());
        }
        if self.runs.len() >= MAX_RUNS {
            return Err("workflow connection run capacity is exhausted".into());
        }
        let active = self
            .runs
            .values()
            .filter(|run| run.lock().map_or(true, |run| !run.exit_observed))
            .count();
        if active >= MAX_ACTIVE_RUNS {
            return Err("workflow active run capacity is exhausted".into());
        }
        if self.runs.values().any(|run| {
            run.lock()
                .map_or(true, |run| run.recipe.id == recipe_id && !run.exit_observed)
        }) {
            return Err("workflow recipe already has an active run".into());
        }
        let directories = Arc::new(pin_workflow_directories(&recipe)?);
        let executable = pin_executable(&recipe)?;
        validate_result_location(&recipe)?;
        let mut command = Command::new(&recipe.executable);
        command
            .args(&recipe.arguments)
            .current_dir(&recipe.working_directory)
            .env_clear()
            .env("TEMP", &recipe.artifact_root)
            .env("TMP", &recipe.artifact_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                command.env("SystemRoot", system_root);
            }
            if !recipe.gui {
                command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
        }
        let policy_expiry = self
            .policy
            .as_ref()
            .expect("validated policy")
            .expires_at_unix_ms;
        let policy_deadline = self.policy_deadline.ok_or("workflow policy has expired")?;
        if unix_ms() >= policy_expiry || Instant::now() >= policy_deadline {
            return Err("workflow policy has expired".into());
        }
        let (supervisor_sender, supervisor_receiver) =
            std::sync::mpsc::sync_channel::<Arc<Mutex<RunState>>>(1);
        std::thread::Builder::new()
            .name("cua-workflow".into())
            .spawn(move || {
                let Ok(supervisor_state) = supervisor_receiver.recv() else {
                    return;
                };
                loop {
                    let Ok(mut state) = supervisor_state.lock() else {
                        return;
                    };
                    refresh(&mut state);
                    if state.exit_observed && state.verification.is_none() {
                        return;
                    }
                    drop(state);
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
            .map_err(|_| "workflow deadline supervisor could not be started")?;
        let child = command
            .spawn()
            .map_err(|_| "workflow process could not be launched")?;
        let id = format!("workflow-{}", uuid::Uuid::new_v4());
        let state = Arc::new(Mutex::new(RunState {
            child,
            _executable: Some(executable),
            _directories: Some(directories),
            deadline: Instant::now() + Duration::from_millis(recipe.timeout_ms),
            policy_expiry,
            policy_deadline,
            state: "running",
            reason: None,
            exit_code: None,
            exit_observed: false,
            result_verified: false,
            cancellation_requested: false,
            termination_reason: None,
            verification: None,
            recipe,
        }));
        if state
            .lock()
            .map_err(|_| "workflow state is unavailable")?
            .recipe
            .kind
            == WorkflowKind::Uninstall
        {
            self.consumed_uninstall.insert(recipe_id.to_owned());
        }
        if supervisor_sender.send(state.clone()).is_err() {
            let mut run = state.lock().map_err(|_| "workflow state is unavailable")?;
            run.state = "outcome_unknown";
            run.reason = Some("deadline_supervisor_unavailable");
            if run.recipe.allow_cancel {
                let _ = run.child.kill();
                refresh(&mut run);
            }
        }
        self.runs.insert(id.clone(), state);
        self.status(&id)
    }

    pub(crate) fn process_id(&mut self, id: &str) -> Result<u32, String> {
        let state = self
            .runs
            .get(id)
            .ok_or("workflow is not owned by this connection")?;
        let mut state = state.lock().map_err(|_| "workflow state is unavailable")?;
        refresh(&mut state);
        if state.state != "running" || state.termination_reason.is_some() {
            return Err("workflow process is not active".into());
        }
        Ok(state.child.id())
    }

    pub(crate) fn authorization_deadline(&mut self, id: &str) -> Result<Instant, String> {
        self.process_id(id)?;
        let run = self
            .runs
            .get(id)
            .ok_or("workflow is not owned by this connection")?
            .lock()
            .map_err(|_| "workflow state is unavailable")?;
        let deadline = run.deadline.min(run.policy_deadline);
        if Instant::now() >= deadline {
            return Err("workflow authorization deadline has expired".into());
        }
        Ok(deadline)
    }

    pub(crate) fn owns_process(&self, process_id: u32) -> bool {
        self.runs.values().any(|run| {
            run.lock().map_or(true, |run| {
                run.child.id() == process_id && !run.exit_observed
            })
        })
    }

    pub(crate) fn authorizes_process(&mut self, id: &str, process_id: u32) -> Result<(), String> {
        if self.process_id(id)? != process_id {
            return Err("workflow process does not match its retained owned child".into());
        }
        Ok(())
    }

    pub(crate) fn status(&mut self, id: &str) -> Result<Value, String> {
        let state = self
            .runs
            .get(id)
            .ok_or("workflow is not owned by this connection")?;
        let mut state = state.lock().map_err(|_| "workflow state is unavailable")?;
        refresh(&mut state);
        Ok(report(id, &state))
    }

    pub(crate) fn cancel(&mut self, id: &str) -> Result<Value, String> {
        let state = self
            .runs
            .get(id)
            .ok_or("workflow is not owned by this connection")?;
        let mut state = state.lock().map_err(|_| "workflow state is unavailable")?;
        refresh(&mut state);
        if state.state == "running" {
            if !state.recipe.allow_cancel {
                return Err("owned workflow process cancellation is not authorized".into());
            }
            state.cancellation_requested = true;
            refresh(&mut state);
        }
        Ok(report(id, &state))
    }

    pub(crate) fn stop_all(&mut self) -> Value {
        let ids = self.runs.keys().cloned().collect::<Vec<_>>();
        let _attempts = ids.iter().map(|id| self.cancel(id)).collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.runs.values().any(|run| {
            run.lock().map_or(true, |mut run| {
                refresh(&mut run);
                !run.exit_observed || run.verification.is_some()
            })
        }) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let results = ids.iter().map(|id| self.status(id).unwrap_or_else(|_| {
            json!({"workflow_id": id, "state":"outcome_unknown", "reason":"cancellation_not_confirmed"})
        })).collect::<Vec<_>>();
        let success = self.runs.values().all(|run| {
            run.lock()
                .is_ok_and(|run| run.exit_observed && run.verification.is_none())
        });
        self.policy = None;
        self.policy_deadline = None;
        json!({"success":success, "workflows":results, "policy_revoked":true})
    }
}

impl Drop for WorkflowManager {
    fn drop(&mut self) {
        let _ = self.stop_all();
    }
}

fn refresh(run: &mut RunState) {
    refresh_state(run);
    if run.exit_observed {
        run._executable = None;
        if run.state != "verifying" {
            run._directories = None;
        }
    }
}

fn refresh_state(run: &mut RunState) {
    if run.state != "verifying"
        && let Some(verification) = run.verification.as_ref()
    {
        if matches!(
            verification.receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ) {
            return;
        }
        // A late result confirms cleanup without changing the unknown outcome.
        run.verification = None;
    }
    if run.state == "verifying" {
        let Some(verification) = run.verification.as_ref() else {
            run.state = "outcome_unknown";
            run.reason = Some("result_verification_unavailable");
            return;
        };
        if Instant::now() >= verification.deadline || unix_ms() >= run.policy_expiry {
            run.state = "outcome_unknown";
            run.reason = Some("result_verification_deadline_elapsed");
            return;
        }
        match verification.receiver.try_recv() {
            Ok(Ok(true)) => {
                run.state = "succeeded";
                run.result_verified = true;
            }
            Ok(Ok(false)) => {
                run.state = "failed";
                run.reason = Some("result_verification_failed");
            }
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                run.state = "outcome_unknown";
                run.reason = Some("result_verification_unavailable");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        }
        run.verification = None;
        return;
    }
    if run.exit_observed {
        return;
    }
    match run.child.try_wait() {
        Ok(Some(exit)) => {
            run.exit_observed = true;
            run.exit_code = exit.code();
            if let Some(reason) = run.termination_reason {
                run.state = reason;
                return;
            }
            if run.state == "outcome_unknown" {
                return;
            }
            if !exit
                .code()
                .is_some_and(|code| run.recipe.allowed_exit_codes.contains(&code))
            {
                run.state = "failed";
                run.reason = Some("unexpected_process_exit");
                return;
            }
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let recipe = run.recipe.clone();
            let Some(directories) = run._directories.as_ref().cloned() else {
                run.state = "outcome_unknown";
                run.reason = Some("result_verification_directory_guards_unavailable");
                return;
            };
            let deadline = run.deadline.min(run.policy_deadline);
            if std::thread::Builder::new()
                .name("cua-result-check".into())
                .spawn(move || {
                    let result = verify_result(&recipe);
                    drop(directories);
                    let _ = sender.send(result);
                })
                .is_err()
            {
                run.state = "outcome_unknown";
                run.reason = Some("result_verification_unavailable");
            } else {
                run.state = "verifying";
                run.verification = Some(Verification { receiver, deadline });
            }
        }
        Err(_) => {
            run.state = "outcome_unknown";
            run.reason = Some("process_state_unavailable");
        }
        Ok(None) => {
            if run.state == "outcome_unknown" {
                return;
            }
            let expiry = unix_ms() >= run.policy_expiry || Instant::now() >= run.policy_deadline;
            let timeout = Instant::now() >= run.deadline;
            let requested = run.cancellation_requested;
            if (expiry || timeout || requested) && run.termination_reason.is_none() {
                if !run.recipe.allow_cancel {
                    run.state = "outcome_unknown";
                    run.reason = Some(if expiry {
                        "authorization_expired_process_still_running"
                    } else {
                        "deadline_elapsed_process_still_running"
                    });
                } else {
                    let reason = if expiry {
                        "expired"
                    } else if timeout {
                        "timed_out"
                    } else {
                        "cancelled"
                    };
                    match run.child.kill() {
                        Ok(()) => {
                            run.termination_reason = Some(reason);
                        }
                        Err(_) => {
                            run.state = "outcome_unknown";
                            run.reason = Some("owned_process_termination_unconfirmed");
                        }
                    }
                }
            }
        }
    }
}

fn report(id: &str, run: &RunState) -> Value {
    json!({
        "workflow_id":id, "recipe_id":run.recipe.id, "kind":run.recipe.kind.as_str(),
        "state":run.state, "reason":run.reason, "process_id":run.child.id(),
        "exit_code":run.exit_code, "result_verified":run.result_verified,
        "process_exited":run.exit_observed,
        "cancellation_requested":run.cancellation_requested,
        "rollback_verified":false,
    })
}

fn validate_recipe(recipe: &mut Recipe) -> Result<(), String> {
    // Reuse the Host's closed method/action validator before any process can launch.
    TrustedTaskAuthorizationRegistration {
        connection_id: None,
        task_id: None,
        task_grant_id: "workflow-policy-validation".into(),
        application_label: "Workflow recipe validation".into(),
        target: TrustedTaskAuthorizationTarget::ExactWindow {
            process_id: 1,
            window_handle: 1,
        },
        allowed_host_methods: recipe.allowed_methods.clone(),
        allowed_actions: recipe.allowed_actions.clone(),
        allowed_browser_origins: Vec::new(),
        browser_scope: None,
        recording_output_dir: None,
        capture_preparation: None,
        expires_at_unix_ms: unix_ms().saturating_add(60_000),
    }
    .validate()
    .map_err(|_| "workflow recipe method or action ceiling is invalid")?;
    if !valid_id(&recipe.id)
        || recipe.arguments.len() > 32
        || recipe
            .arguments
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('\0'))
        || !(1..=24 * 60 * 60 * 1000).contains(&recipe.timeout_ms)
        || recipe.allowed_exit_codes.is_empty()
        || recipe.allowed_exit_codes.len() > 32
        || recipe.allowed_methods.is_empty()
        || recipe.allowed_methods.len() > 32
        || recipe
            .allowed_methods
            .iter()
            .any(|method| !valid_id(method))
        || recipe.allowed_actions.len() > 32
        || !valid_hash(&recipe.executable_sha256)
        || recipe
            .result
            .sha256
            .as_deref()
            .is_some_and(|hash| !valid_hash(hash))
        || (!recipe.result.present && recipe.result.sha256.is_some())
        || (recipe.kind == WorkflowKind::Install) != recipe.result.present
    {
        return Err("workflow recipe does not satisfy its bounded contract".into());
    }
    if !recipe.executable.is_absolute()
        || !recipe.artifact_root.is_absolute()
        || !recipe.working_directory.is_absolute()
    {
        return Err("workflow paths must be absolute and canonical".into());
    }
    for path in [
        &recipe.executable,
        &recipe.artifact_root,
        &recipe.working_directory,
    ] {
        validate_local_path(path)?;
    }
    recipe.executable =
        fs::canonicalize(&recipe.executable).map_err(|_| "workflow executable is unavailable")?;
    recipe.artifact_root = fs::canonicalize(&recipe.artifact_root)
        .map_err(|_| "workflow artifact root is unavailable")?;
    recipe.working_directory = fs::canonicalize(&recipe.working_directory)
        .map_err(|_| "workflow working directory is unavailable")?;
    if !recipe.artifact_root.is_dir()
        || !recipe.working_directory.is_dir()
        || !recipe.working_directory.starts_with(&recipe.artifact_root)
        || !recipe.executable.is_file()
        || restricted_executable(&recipe.executable)
    {
        return Err("workflow executable or directory scope is not permitted".into());
    }
    if recipe.result.relative_path.as_os_str().is_empty()
        || recipe
            .result
            .relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || recipe.result.relative_path.components().any(|component| {
            let name = component.as_os_str().to_string_lossy();
            name.chars().any(|character| {
                matches!(character, ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\0')
            }) || name.ends_with([' ', '.'])
                || matches!(
                    name.split('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_uppercase()
                        .as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
    {
        return Err("workflow result must be a bounded relative artifact path".into());
    }
    validate_result_location(recipe)
}

fn restricted_executable(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    (cfg!(windows) && extension != "exe")
        || matches!(
            extension.as_str(),
            "bat" | "cmd" | "ps1" | "vbs" | "js" | "sh" | "py" | "com" | "scr"
        )
        || matches!(
            stem.as_str(),
            "cmd"
                | "powershell"
                | "pwsh"
                | "bash"
                | "sh"
                | "zsh"
                | "fish"
                | "wsl"
                | "wscript"
                | "cscript"
                | "mshta"
                | "node"
                | "nodejs"
                | "rundll32"
                | "regsvr32"
                | "msiexec"
                | "control"
                | "explorer"
                | "consent"
                | "credentialuibroker"
                | "logonui"
                | "winlogon"
                | "lsass"
                | "sudo"
                | "pkexec"
                | "doas"
                | "login"
                | "passwd"
        )
        || stem.starts_with("python")
}

fn pin_executable(recipe: &Recipe) -> Result<File, String> {
    if fs::canonicalize(&recipe.executable).ok().as_ref() != Some(&recipe.executable) {
        return Err("workflow executable identity changed".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // FILE_SHARE_READ; forbid write and replacement while pinned.
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let mut file = options
        .open(&recipe.executable)
        .map_err(|_| "workflow executable could not be pinned")?;
    let metadata = file
        .metadata()
        .map_err(|_| "workflow executable metadata is unavailable")?;
    if !metadata.is_file() || is_reparse(&metadata) {
        return Err("workflow executable is not an ordinary pinned file".into());
    }
    let actual = hash_reader(&mut file).map_err(|_| "workflow executable could not be verified")?;
    if actual != recipe.executable_sha256.to_ascii_lowercase() {
        return Err("workflow executable digest changed".into());
    }
    Ok(file)
}

fn validate_result_location(recipe: &Recipe) -> Result<(), String> {
    if fs::canonicalize(&recipe.artifact_root).ok().as_ref() != Some(&recipe.artifact_root)
        || fs::canonicalize(&recipe.working_directory).ok().as_ref()
            != Some(&recipe.working_directory)
    {
        return Err("workflow directory identity changed".into());
    }
    let mut path = recipe.artifact_root.clone();
    for component in recipe.result.relative_path.components() {
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || !fs::canonicalize(&path)
                        .is_ok_and(|path| path.starts_with(&recipe.artifact_root))
                {
                    return Err("workflow result escaped its artifact root".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err("workflow result location is unavailable".into()),
        }
    }
    Ok(())
}

fn verify_result(recipe: &Recipe) -> Result<bool, String> {
    let mut directories = pin_workflow_directories(recipe)?;
    if !pin_result_parents(recipe, &mut directories)? {
        // Absence is decided at the first missing component under pinned ancestors.
        return Ok(!recipe.result.present);
    }
    validate_result_location(recipe)?;
    let path = recipe.artifact_root.join(&recipe.result.relative_path);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(!recipe.result.present);
        }
        Err(_) => return Err("workflow result is unavailable".into()),
    };
    if !recipe.result.present {
        return Ok(false);
    }
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "workflow result could not be pinned")?;
    let metadata = file
        .metadata()
        .map_err(|_| "workflow result metadata is unavailable")?;
    if !metadata.is_file() || is_reparse(&metadata) {
        return Ok(false);
    }
    match &recipe.result.sha256 {
        Some(expected) => {
            let actual =
                hash_reader(&mut file).map_err(|_| "workflow result could not be verified")?;
            Ok(actual == expected.to_ascii_lowercase())
        }
        None => Ok(true),
    }
}

fn hash_reader(reader: &mut impl Read) -> std::io::Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_HASH_BYTES {
            return Err(std::io::Error::other(
                "workflow hash input exceeds its size bound",
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::MAX, |value| {
            value.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

fn validate_local_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use std::path::Prefix;
        if !matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        {
            return Err("workflow paths must use a local disk namespace".into());
        }
        let root = path
            .ancestors()
            .last()
            .ok_or("workflow disk root is unavailable")?;
        let wide = root
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let drive_type =
            unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) };
        if !matches!(drive_type, 3 | 6) {
            // DRIVE_FIXED or DRIVE_RAMDISK; reject mapped network drives.
            return Err("workflow paths require a verified local fixed or memory disk".into());
        }
    }
    for ancestor in path.ancestors() {
        let metadata =
            fs::symlink_metadata(ancestor).map_err(|_| "workflow path is unavailable")?;
        if is_reparse(&metadata) {
            return Err("workflow paths cannot contain reparse or symbolic links".into());
        }
    }
    Ok(())
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(windows)]
fn pin_workflow_directories(recipe: &Recipe) -> Result<Vec<File>, String> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut guards = Vec::new();
    let executable_parent = recipe
        .executable
        .parent()
        .ok_or("workflow executable parent is unavailable")?;
    let paths = [
        executable_parent,
        &recipe.artifact_root,
        &recipe.working_directory,
    ];
    for path in paths {
        let ancestors = path.ancestors().collect::<Vec<_>>();
        for ancestor in ancestors.into_iter().rev() {
            let metadata = match fs::symlink_metadata(ancestor) {
                Ok(metadata) => metadata,
                Err(_) => return Err("workflow directory ancestor is unavailable".into()),
            };
            if !metadata.is_dir() || is_reparse(&metadata) {
                return Err("workflow directory ancestors must be ordinary directories".into());
            }
            let guard = fs::OpenOptions::new()
                .access_mode(0x81) // FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES participates in share reservations.
                .share_mode(1) // FILE_SHARE_READ; deny write/reparse changes and rename/delete.
                .custom_flags(0x0200_0000 | 0x0020_0000) // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
                .open(ancestor)
                .map_err(|_| "workflow directory ancestor could not be pinned")?;
            let metadata = guard
                .metadata()
                .map_err(|_| "workflow pinned directory metadata is unavailable")?;
            if !metadata.is_dir() || is_reparse(&metadata) {
                return Err("workflow pinned directory identity is invalid".into());
            }
            guards.push(guard);
        }
    }
    Ok(guards)
}

#[cfg(not(windows))]
fn pin_workflow_directories(_recipe: &Recipe) -> Result<Vec<File>, String> {
    Err("pinned native workflow launch is unsupported on this platform".into())
}

#[cfg(windows)]
fn pin_result_parents(recipe: &Recipe, guards: &mut Vec<File>) -> Result<bool, String> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut path = recipe.artifact_root.clone();
    for component in recipe
        .result
        .relative_path
        .parent()
        .unwrap_or(Path::new(""))
        .components()
    {
        path.push(component.as_os_str());
        let guard = match fs::OpenOptions::new()
            .access_mode(0x81)
            .share_mode(1)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(&path)
        {
            Ok(guard) => guard,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err("workflow result parent could not be pinned".into()),
        };
        let metadata = guard
            .metadata()
            .map_err(|_| "workflow result parent metadata is unavailable")?;
        if !metadata.is_dir() || is_reparse(&metadata) {
            return Err("workflow result parents must be ordinary pinned directories".into());
        }
        guards.push(guard);
    }
    Ok(true)
}

#[cfg(not(windows))]
fn pin_result_parents(_recipe: &Recipe, _guards: &mut Vec<File>) -> Result<bool, String> {
    Err("pinned native workflow launch is unsupported on this platform".into())
}

#[cfg(windows)]
fn require_standard_user() -> Result<(), String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("workflow launch could not verify the standard-user token".into());
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0;
    let verified = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut std::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    };
    unsafe {
        CloseHandle(token);
    }
    if verified == 0 || length != std::mem::size_of::<TOKEN_ELEVATION>() as u32 {
        return Err("workflow launch could not verify the standard-user token".into());
    }
    if elevation.TokenIsElevated != 0 {
        return Err(
            "workflow launch requires a standard-user Host; elevation is not delegated".into(),
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn require_standard_user() -> Result<(), String> {
    Err("pinned native workflow launch is unsupported on this platform".into())
}

#[cfg(test)]
mod tests;
