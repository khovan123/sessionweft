use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    thread,
};

use chrono::Utc;
use portable_pty::{
    ChildKiller, CommandBuilder, MasterPty, PtySize, PtySystem, native_pty_system,
};
use thiserror::Error;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::{
    MAX_PTY_OUTPUT_LIMIT, PtyOutputBatch, PtyOutputChunk, PtyResizeRequest, PtySessionDescriptor,
    PtyStatus, StartPtyRequest,
};

#[derive(Clone)]
pub struct PtySupervisor {
    workspace_root: Arc<PathBuf>,
    programs: Arc<BTreeMap<String, PathBuf>>,
    allowed_environment: Arc<std::collections::BTreeSet<String>>,
    sessions: Arc<RwLock<HashMap<Uuid, Arc<PtySession>>>>,
}

impl PtySupervisor {
    pub fn new(
        workspace_root: impl AsRef<Path>,
        programs: BTreeMap<String, PathBuf>,
        allowed_environment: std::collections::BTreeSet<String>,
    ) -> Result<Self, PtyError> {
        let workspace_root = fs::canonicalize(workspace_root).map_err(PtyError::Io)?;
        if !workspace_root.is_dir() {
            return Err(PtyError::Validation(
                "PTY workspace root must be a directory".into(),
            ));
        }
        let mut canonical_programs = BTreeMap::new();
        for (alias, program) in programs {
            if alias.trim().is_empty() {
                return Err(PtyError::Validation(
                    "PTY program aliases cannot be empty".into(),
                ));
            }
            let canonical = fs::canonicalize(program).map_err(PtyError::Io)?;
            if !canonical.is_file() {
                return Err(PtyError::Validation(format!(
                    "PTY program '{alias}' is not a file"
                )));
            }
            canonical_programs.insert(alias, canonical);
        }
        if canonical_programs.is_empty() {
            return Err(PtyError::Validation(
                "at least one PTY program must be allowlisted".into(),
            ));
        }
        Ok(Self {
            workspace_root: Arc::new(workspace_root),
            programs: Arc::new(canonical_programs),
            allowed_environment: Arc::new(allowed_environment),
            sessions: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn start(&self, request: StartPtyRequest) -> Result<PtySessionDescriptor, PtyError> {
        validate_start(&request)?;
        let program = self
            .programs
            .get(&request.program)
            .ok_or_else(|| PtyError::ProgramDenied(request.program.clone()))?;
        let cwd = canonical_child(&self.workspace_root, Path::new(&request.cwd))?;
        for key in request.environment.keys() {
            if !self.allowed_environment.contains(key) {
                return Err(PtyError::EnvironmentDenied(key.clone()));
            }
        }

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: request.rows,
                cols: request.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| PtyError::Backend(error.to_string()))?;
        let mut command = CommandBuilder::new(program);
        command.args(&request.args);
        command.cwd(&cwd);
        command.env_clear();
        for (key, value) in &request.environment {
            command.env(key, value);
        }
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| PtyError::Backend(error.to_string()))?;
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| PtyError::Backend(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| PtyError::Backend(error.to_string()))?;
        let killer = child.clone_killer();
        let pty_id = Uuid::new_v4();
        let session = Arc::new(PtySession {
            descriptor: Mutex::new(PtySessionDescriptor {
                protocol_version: crate::CLIENT_PROTOCOL_VERSION,
                pty_id,
                session_id: request.session_id,
                status: PtyStatus::Running,
                rows: request.rows,
                cols: request.cols,
                output_cursor: 0,
                created_at: Utc::now(),
            }),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            output: Mutex::new(OutputBuffer::new(request.max_output_bytes)),
            notify: Notify::new(),
        });
        self.sessions
            .write()
            .map_err(|_| PtyError::Poisoned)?
            .insert(pty_id, Arc::clone(&session));
        spawn_reader(Arc::clone(&session), reader);
        spawn_waiter(Arc::clone(&session), &mut child);
        Ok(session.descriptor()?)
    }

    pub fn descriptor(&self, pty_id: Uuid) -> Result<PtySessionDescriptor, PtyError> {
        self.session(pty_id)?.descriptor()
    }

    pub fn input(&self, pty_id: Uuid, data: &str) -> Result<(), PtyError> {
        if data.len() > 1024 * 1024 {
            return Err(PtyError::Validation(
                "PTY input cannot exceed 1 MiB per request".into(),
            ));
        }
        let session = self.session(pty_id)?;
        if session.descriptor()?.status != PtyStatus::Running {
            return Err(PtyError::NotRunning(pty_id));
        }
        let mut writer = session.writer.lock().map_err(|_| PtyError::Poisoned)?;
        writer.write_all(data.as_bytes()).map_err(PtyError::Io)?;
        writer.flush().map_err(PtyError::Io)
    }

    pub fn resize(&self, pty_id: Uuid, request: PtyResizeRequest) -> Result<(), PtyError> {
        validate_size(request.rows, request.cols)?;
        let session = self.session(pty_id)?;
        session
            .master
            .lock()
            .map_err(|_| PtyError::Poisoned)?
            .resize(PtySize {
                rows: request.rows,
                cols: request.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| PtyError::Backend(error.to_string()))?;
        let mut descriptor = session.descriptor.lock().map_err(|_| PtyError::Poisoned)?;
        descriptor.rows = request.rows;
        descriptor.cols = request.cols;
        Ok(())
    }

    pub fn cancel(&self, pty_id: Uuid) -> Result<PtySessionDescriptor, PtyError> {
        let session = self.session(pty_id)?;
        {
            let mut descriptor = session.descriptor.lock().map_err(|_| PtyError::Poisoned)?;
            if descriptor.status == PtyStatus::Running {
                session
                    .killer
                    .lock()
                    .map_err(|_| PtyError::Poisoned)?
                    .kill()
                    .map_err(PtyError::Io)?;
                descriptor.status = PtyStatus::Cancelled;
            }
        }
        session.notify.notify_waiters();
        session.descriptor()
    }

    pub fn output_after(&self, pty_id: Uuid, after: u64) -> Result<PtyOutputBatch, PtyError> {
        let session = self.session(pty_id)?;
        let descriptor = session.descriptor()?;
        let output = session.output.lock().map_err(|_| PtyError::Poisoned)?;
        let chunks = output
            .chunks
            .iter()
            .filter(|chunk| chunk.cursor > after)
            .cloned()
            .collect::<Vec<_>>();
        let next = chunks.last().map_or(after, |chunk| chunk.cursor);
        Ok(PtyOutputBatch {
            protocol_version: crate::CLIENT_PROTOCOL_VERSION,
            pty_id,
            after,
            next,
            status: descriptor.status,
            chunks,
            total_retained_bytes: output.retained_bytes,
        })
    }

    pub async fn wait_for_output(
        &self,
        pty_id: Uuid,
        after: u64,
        timeout: std::time::Duration,
    ) -> Result<PtyOutputBatch, PtyError> {
        let session = self.session(pty_id)?;
        let current = self.output_after(pty_id, after)?;
        if !current.chunks.is_empty() || current.status != PtyStatus::Running {
            return Ok(current);
        }
        let _ = tokio::time::timeout(timeout, session.notify.notified()).await;
        self.output_after(pty_id, after)
    }

    fn session(&self, pty_id: Uuid) -> Result<Arc<PtySession>, PtyError> {
        self.sessions
            .read()
            .map_err(|_| PtyError::Poisoned)?
            .get(&pty_id)
            .cloned()
            .ok_or(PtyError::NotFound(pty_id))
    }
}

struct PtySession {
    descriptor: Mutex<PtySessionDescriptor>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    output: Mutex<OutputBuffer>,
    notify: Notify,
}

impl PtySession {
    fn descriptor(&self) -> Result<PtySessionDescriptor, PtyError> {
        self.descriptor
            .lock()
            .map_err(|_| PtyError::Poisoned)
            .map(|value| value.clone())
    }

    fn push_output(&self, data: &[u8]) -> Result<(), PtyError> {
        let mut descriptor = self.descriptor.lock().map_err(|_| PtyError::Poisoned)?;
        let mut output = self.output.lock().map_err(|_| PtyError::Poisoned)?;
        descriptor.output_cursor = descriptor.output_cursor.saturating_add(1);
        output.push(descriptor.output_cursor, data);
        drop(output);
        drop(descriptor);
        self.notify.notify_waiters();
        Ok(())
    }

    fn finish(&self, status: PtyStatus) {
        if let Ok(mut descriptor) = self.descriptor.lock()
            && descriptor.status == PtyStatus::Running
        {
            descriptor.status = status;
        }
        self.notify.notify_waiters();
    }
}

struct OutputBuffer {
    chunks: VecDeque<PtyOutputChunk>,
    retained_bytes: usize,
    max_bytes: usize,
}

impl OutputBuffer {
    const fn new(max_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            retained_bytes: 0,
            max_bytes,
        }
    }

    fn push(&mut self, cursor: u64, data: &[u8]) {
        let mut truncated = false;
        let bounded = if data.len() > self.max_bytes {
            truncated = true;
            &data[data.len() - self.max_bytes..]
        } else {
            data
        };
        while self.retained_bytes.saturating_add(bounded.len()) > self.max_bytes {
            let Some(removed) = self.chunks.pop_front() else {
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(removed.data.len());
            truncated = true;
        }
        let data = String::from_utf8_lossy(bounded).into_owned();
        self.retained_bytes = self.retained_bytes.saturating_add(data.len());
        self.chunks.push_back(PtyOutputChunk {
            cursor,
            data,
            truncated,
            created_at: Utc::now(),
        });
    }
}

fn spawn_reader(session: Arc<PtySession>, mut reader: Box<dyn Read + Send>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    if session.push_output(&buffer[..size]).is_err() {
                        session.finish(PtyStatus::Failed);
                        break;
                    }
                }
                Err(_) => {
                    session.finish(PtyStatus::Failed);
                    break;
                }
            }
        }
    });
}

fn spawn_waiter(
    session: Arc<PtySession>,
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
) {
    let mut child = std::mem::replace(child, empty_child());
    thread::spawn(move || match child.wait() {
        Ok(_) => session.finish(PtyStatus::Exited),
        Err(_) => session.finish(PtyStatus::Failed),
    });
}

fn empty_child() -> Box<dyn portable_pty::Child + Send + Sync> {
    struct EmptyChild;
    impl std::fmt::Debug for EmptyChild {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("EmptyChild")
        }
    }
    impl ChildKiller for EmptyChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }
    impl portable_pty::Child for EmptyChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
    }
    Box::new(EmptyChild)
}

fn validate_start(request: &StartPtyRequest) -> Result<(), PtyError> {
    if request.program.trim().is_empty() || request.program.len() > 128 {
        return Err(PtyError::Validation(
            "PTY program alias must be between 1 and 128 bytes".into(),
        ));
    }
    if request.args.len() > 256 || request.args.iter().any(|value| value.len() > 16 * 1024) {
        return Err(PtyError::Validation(
            "PTY arguments exceed configured limits".into(),
        ));
    }
    validate_size(request.rows, request.cols)?;
    if request.max_output_bytes == 0 || request.max_output_bytes > MAX_PTY_OUTPUT_LIMIT {
        return Err(PtyError::Validation(format!(
            "PTY output limit must be between 1 and {MAX_PTY_OUTPUT_LIMIT} bytes"
        )));
    }
    Ok(())
}

fn validate_size(rows: u16, cols: u16) -> Result<(), PtyError> {
    if rows == 0 || cols == 0 || rows > 1_000 || cols > 1_000 {
        return Err(PtyError::Validation(
            "PTY rows and columns must be between 1 and 1000".into(),
        ));
    }
    Ok(())
}

fn canonical_child(root: &Path, requested: &Path) -> Result<PathBuf, PtyError> {
    let candidate = if requested.is_absolute() {
        requested.to_owned()
    } else {
        root.join(requested)
    };
    let canonical = fs::canonicalize(candidate).map_err(PtyError::Io)?;
    if !canonical.starts_with(root) || !canonical.is_dir() {
        return Err(PtyError::WorkspaceEscape(canonical));
    }
    Ok(canonical)
}

#[must_use]
pub fn discover_programs(names: &[&str]) -> BTreeMap<String, PathBuf> {
    let Some(path) = env::var_os("PATH") else {
        return BTreeMap::new();
    };
    let directories = env::split_paths(&path).collect::<Vec<_>>();
    names
        .iter()
        .filter_map(|name| {
            directories
                .iter()
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file())
                .and_then(|candidate| fs::canonicalize(candidate).ok())
                .map(|candidate| ((*name).to_owned(), candidate))
        })
        .collect()
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("PTY validation failed: {0}")]
    Validation(String),
    #[error("PTY program '{0}' is not allowlisted")]
    ProgramDenied(String),
    #[error("PTY environment variable '{0}' is not allowlisted")]
    EnvironmentDenied(String),
    #[error("PTY path escapes workspace: {0}")]
    WorkspaceEscape(PathBuf),
    #[error("PTY session {0} was not found")]
    NotFound(Uuid),
    #[error("PTY session {0} is not running")]
    NotRunning(Uuid),
    #[error("PTY internal state lock was poisoned")]
    Poisoned,
    #[error("PTY backend error: {0}")]
    Backend(String),
    #[error("PTY I/O error: {0}")]
    Io(std::io::Error),
}
