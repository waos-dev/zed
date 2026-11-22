//! WaOS port: subprocesses for a WASI component, which has none of its own, run on the machine
//! its user picked for it (`wasdk::machines`): this computer, as the user's login, outside the
//! program's sandbox. The same API as the other platforms' `Command` and `Child`.
//!
//! A machine runs a command asynchronously, while `spawn` answers at once, so the child's pipes
//! are channels: a task of the program's own starts the command, then carries the input written
//! to the child's `stdin` to the machine, the machine's output and errors to `stdout` and
//! `stderr`, and the child's signals and exit status between the two. A command spawned in a
//! terminal (`spawn_in_terminal`) is carried the same way, its terminal's input, output and size
//! in place of its pipes.
//!
//! What a machine cannot do: unset a variable of the login environment (`env_remove` and
//! `env_clear` keep it), and say a child's process id (`id` is the program's own count). std has
//! no `ExitStatus` to make on WASI, so a child's status is this module's [`ExitStatus`].

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};

use futures::channel::{mpsc, oneshot};
use futures::future::{self, Either, FutureExt as _, Shared};
use futures::{AsyncRead, AsyncWrite, StreamExt as _};
use wasdk::machines::{self, ExitStatus as MachineExitStatus, Signal};

pub use wasdk::machines::TerminalSize;

pub use std::process::Stdio;

/// How a child ended, as the machine says; by default, a success, as std's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitStatus {
    code: Option<i32>,
    signaled: bool,
}

impl Default for ExitStatus {
    fn default() -> Self {
        ExitStatus {
            code: Some(0),
            signaled: false,
        }
    }
}

impl From<std::process::ExitStatus> for ExitStatus {
    fn from(status: std::process::ExitStatus) -> Self {
        ExitStatus {
            code: status.code(),
            signaled: false,
        }
    }
}

impl ExitStatus {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    pub fn code(&self) -> Option<i32> {
        self.code
    }

    fn unknown() -> ExitStatus {
        ExitStatus {
            code: None,
            signaled: false,
        }
    }
}

impl From<MachineExitStatus> for ExitStatus {
    fn from(status: MachineExitStatus) -> Self {
        match status {
            MachineExitStatus::Exited(code) => ExitStatus {
                code: Some(code as i32),
                signaled: false,
            },
            MachineExitStatus::Signaled(_) => ExitStatus {
                code: None,
                signaled: true,
            },
            MachineExitStatus::Unknown => ExitStatus::unknown(),
        }
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.code, self.signaled) {
            (Some(code), _) => write!(f, "exit status: {code}"),
            (None, true) => f.write_str("ended by a signal"),
            (None, false) => f.write_str("unknown exit status"),
        }
    }
}

/// What a child wrote, and how it ended.
#[derive(Clone, Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    variables: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    kill_on_drop: bool,
}

impl Command {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            variables: Vec::new(),
            current_dir: None,
            kill_on_drop: false,
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    pub fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.args.iter().map(OsString::as_os_str)
    }

    pub fn env(&mut self, key: impl AsRef<OsStr>, val: impl AsRef<OsStr>) -> &mut Self {
        self.variables
            .push((key.as_ref().to_owned(), val.as_ref().to_owned()));
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (key, val) in vars {
            self.env(key, val);
        }
        self
    }

    /// A machine's command cannot unset its login environment's variable; this forgets only
    /// what the command set.
    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.variables.retain(|(name, _)| name != key.as_ref());
        self
    }

    /// The machine's login environment stays; this forgets only what the command set.
    pub fn env_clear(&mut self) -> &mut Self {
        self.variables.clear();
        self
    }

    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.current_dir = Some(dir.as_ref().to_owned());
        self
    }

    /// A machine's command always has its three pipes; what is not taken is drained.
    pub fn stdin(&mut self, _cfg: impl Into<Stdio>) -> &mut Self {
        self
    }

    pub fn stdout(&mut self, _cfg: impl Into<Stdio>) -> &mut Self {
        self
    }

    pub fn stderr(&mut self, _cfg: impl Into<Stdio>) -> &mut Self {
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Self {
        self.kill_on_drop = kill_on_drop;
        self
    }

    pub fn spawn(&mut self) -> io::Result<Child> {
        Ok(Child::start(self.clone()))
    }

    /// Starts the command in a new terminal of the machine's, of `size`.
    pub fn spawn_in_terminal(&mut self, size: TerminalSize) -> TerminalChild {
        TerminalChild::start(self.clone(), size)
    }

    pub async fn output(&mut self) -> io::Result<Output> {
        self.spawn()?.output().await
    }

    pub async fn status(&mut self) -> io::Result<ExitStatus> {
        self.spawn()?.status().await
    }

    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    /// The machine's command for this one.
    fn machine_command(&self) -> machines::Command {
        let command = machines::Command::new(&self.program.to_string_lossy());
        for argument in &self.args {
            command.add_argument(&argument.to_string_lossy());
        }
        for (name, value) in &self.variables {
            command.variable(&name.to_string_lossy(), &value.to_string_lossy());
        }
        if let Some(directory) = &self.current_dir {
            command.working_directory(&directory.to_string_lossy());
        }
        command
    }
}

/// How the child ended, or why its task could not say; shared, so it can be asked again.
type Status = Shared<oneshot::Receiver<Result<ExitStatus, String>>>;

pub struct Child {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildOutput>,
    pub stderr: Option<ChildOutput>,
    id: u32,
    status: Status,
    signals: mpsc::UnboundedSender<Signal>,
    kill_on_drop: bool,
}

impl fmt::Debug for Child {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Child").field("id", &self.id).finish_non_exhaustive()
    }
}

impl Child {
    /// Starts `command` on the machine in a task of the program's own.
    fn start(command: Command) -> Child {
        static NEXT_ID: AtomicU32 = AtomicU32::new(1);
        let (input, input_received) = mpsc::unbounded();
        let (output_sent, output) = mpsc::unbounded();
        let (errors_sent, errors) = mpsc::unbounded();
        let (status_sent, status) = oneshot::channel();
        let (signals, signals_received) = mpsc::unbounded();
        let kill_on_drop = command.kill_on_drop;
        wasdk::spawn(async move {
            let status = run(
                command,
                input_received,
                output_sent,
                errors_sent,
                signals_received,
            )
            .await;
            let _ = status_sent.send(status.map_err(|error| error.to_string()));
        });
        Child {
            stdin: Some(ChildStdin(input)),
            stdout: Some(ChildOutput::new(output)),
            stderr: Some(ChildOutput::new(errors)),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            status: status.shared(),
            signals,
            kill_on_drop,
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn kill(&mut self) -> io::Result<()> {
        let _ = self.signals.unbounded_send(Signal::Kill);
        Ok(())
    }

    pub fn try_status(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.status.peek() {
            Some(Ok(status)) => status.clone().map(Some).map_err(io::Error::other),
            Some(Err(_)) => Err(gone()),
            None => Ok(None),
        }
    }

    pub fn status(
        &mut self,
    ) -> impl std::future::Future<Output = io::Result<ExitStatus>> + Send + 'static {
        self.stdin.take();
        let status = self.status.clone();
        async move { status.await.map_err(|_| gone())?.map_err(io::Error::other) }
    }

    pub async fn output(mut self) -> io::Result<Output> {
        use futures::AsyncReadExt as _;
        self.stdin.take();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let read_stdout = async {
            if let Some(mut output) = self.stdout.take() {
                output.read_to_end(&mut stdout).await?;
            }
            io::Result::Ok(())
        };
        let read_stderr = async {
            if let Some(mut errors) = self.stderr.take() {
                errors.read_to_end(&mut stderr).await?;
            }
            io::Result::Ok(())
        };
        let (read_stdout, read_stderr) = futures::join!(read_stdout, read_stderr);
        read_stdout?;
        read_stderr?;
        let status = self.status().await?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop && self.status.peek().is_none() {
            let _ = self.signals.unbounded_send(Signal::Kill);
        }
    }
}

/// A command running in a terminal of the machine's: what is written is typed into the terminal,
/// and `output` is what the terminal shows. Closing it, or dropping it, hangs the command up,
/// as closing a PTY does.
pub struct TerminalChild {
    pub output: Option<ChildOutput>,
    input: mpsc::UnboundedSender<Vec<u8>>,
    sizes: mpsc::UnboundedSender<TerminalSize>,
    signals: mpsc::UnboundedSender<Signal>,
    status: Status,
}

impl TerminalChild {
    /// Starts `command` in a terminal of `size`, in a task of the program's own.
    fn start(command: Command, size: TerminalSize) -> TerminalChild {
        let (input, input_received) = mpsc::unbounded();
        let (sizes, sizes_received) = mpsc::unbounded();
        let (output_sent, output) = mpsc::unbounded();
        let (status_sent, status) = oneshot::channel();
        let (signals, signals_received) = mpsc::unbounded();
        wasdk::spawn(async move {
            let status = run_in_terminal(
                command,
                size,
                input_received,
                sizes_received,
                output_sent,
                signals_received,
            )
            .await;
            let _ = status_sent.send(status.map_err(|error| error.to_string()));
        });
        TerminalChild {
            output: Some(ChildOutput::new(output)),
            input,
            sizes,
            signals,
            status: status.shared(),
        }
    }

    /// Types `bytes` into the terminal.
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.unbounded_send(bytes);
    }

    pub fn resize(&self, size: TerminalSize) {
        let _ = self.sizes.unbounded_send(size);
    }

    /// Closes the terminal, which hangs the command up.
    pub fn close(&self) {
        self.input.close_channel();
    }

    pub fn kill(&self) {
        let _ = self.signals.unbounded_send(Signal::Kill);
    }

    pub fn status(&self) -> impl std::future::Future<Output = io::Result<ExitStatus>> + Send + 'static {
        let status = self.status.clone();
        async move { status.await.map_err(|_| gone())?.map_err(io::Error::other) }
    }
}

fn gone() -> io::Error {
    io::Error::other("the child's task ended before it said how the child ended")
}

/// Runs `command` on the machine, carrying its pipes and signals, until it ends.
async fn run(
    command: Command,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    output: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
    errors: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
    mut signals: mpsc::UnboundedReceiver<Signal>,
) -> io::Result<ExitStatus> {
    let machine = crate::machine::connect().await?;
    let (child, pipes) = machine
        .exec(&command.machine_command())
        .await
        .map_err(|error| {
            io::Error::other(format!("{:?} did not start: {error:?}", command.program))
        })?;
    let carry_input = async {
        while let Some(bytes) = input.next().await {
            if pipes.write(bytes).await.is_err() {
                break;
            }
        }
        pipes.close_input();
    };
    let carry_output = carry(pipes.output(), output);
    let carry_errors = carry(pipes.errors(), errors);
    let status = wait_for_exit(&child, &mut signals);
    let ((), (), (), status) = futures::join!(carry_input, carry_output, carry_errors, status);
    Ok(status)
}

/// How `child` ended, sending it the signals the program sends meanwhile.
async fn wait_for_exit(
    child: &machines::Child,
    signals: &mut mpsc::UnboundedReceiver<Signal>,
) -> ExitStatus {
    let mut exit = std::pin::pin!(child.exit_status());
    loop {
        match future::select(exit, signals.next()).await {
            Either::Left((status, _)) => return ExitStatus::from(status),
            Either::Right((Some(signal), pending)) => {
                child.signal(signal);
                exit = pending;
            }
            Either::Right((None, pending)) => return ExitStatus::from(pending.await),
        }
    }
}

/// Runs `command` in a terminal of the machine's, carrying its input, output, size and signals,
/// until it ends.
async fn run_in_terminal(
    command: Command,
    size: TerminalSize,
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    mut sizes: mpsc::UnboundedReceiver<TerminalSize>,
    output: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
    mut signals: mpsc::UnboundedReceiver<Signal>,
) -> io::Result<ExitStatus> {
    let machine = crate::machine::connect().await?;
    let (child, terminal) = machine
        .exec_in_terminal(&command.machine_command(), size)
        .await
        .map_err(|error| {
            io::Error::other(format!("{:?} did not start: {error:?}", command.program))
        })?;
    let typing = async {
        while let Some(bytes) = input.next().await {
            if terminal.write(bytes).await.is_err() {
                break;
            }
        }
        child.signal(Signal::Hangup);
    };
    let resizing = async {
        while let Some(size) = sizes.next().await {
            terminal.resize(size);
        }
    };
    let showing = carry(terminal.output(), output);
    let session = async { futures::join!(showing, wait_for_exit(&child, &mut signals)).1 };
    // The program's side may stay open after the command ends; the command's end ends the run.
    let controlling = async {
        futures::join!(typing, resizing);
        future::pending::<ExitStatus>().await
    };
    let status = future::select(std::pin::pin!(session), std::pin::pin!(controlling))
        .await
        .factor_first()
        .0;
    Ok(status)
}

/// Carries one of the machine's output streams into a child's pipe, until it ends.
async fn carry(
    stream: Option<wasdk::StreamReader<u8>>,
    pipe: mpsc::UnboundedSender<io::Result<Vec<u8>>>,
) {
    let Some(mut stream) = stream else {
        return;
    };
    loop {
        let (result, chunk) = stream.read(Vec::with_capacity(16 * 1024)).await;
        if !chunk.is_empty() {
            // Read on even when nobody takes the pipe, so the command is never held up.
            let _ = pipe.unbounded_send(Ok(chunk));
        }
        if result == wasdk::StreamResult::Dropped {
            return;
        }
    }
}

/// A child's input: what is written goes to the machine in order.
#[derive(Debug)]
pub struct ChildStdin(mpsc::UnboundedSender<Vec<u8>>);

impl AsyncWrite for ChildStdin {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.0.unbounded_send(buf.to_vec()) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(_) => Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.close_channel();
        Poll::Ready(Ok(()))
    }
}

/// A child's output or errors, as the machine sends them.
#[derive(Debug)]
pub struct ChildOutput {
    chunks: mpsc::UnboundedReceiver<io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    read: usize,
}

impl ChildOutput {
    fn new(chunks: mpsc::UnboundedReceiver<io::Result<Vec<u8>>>) -> Self {
        Self {
            chunks,
            pending: Vec::new(),
            read: 0,
        }
    }
}

impl AsyncRead for ChildOutput {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        while self.read == self.pending.len() {
            match self.chunks.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.pending = chunk;
                    self.read = 0;
                }
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Pending => return Poll::Pending,
            }
        }
        let count = buf.len().min(self.pending.len() - self.read);
        let start = self.read;
        buf[..count].copy_from_slice(&self.pending[start..start + count]);
        self.read += count;
        Poll::Ready(Ok(count))
    }
}

/// Where `program` is on the machine's `PATH`, looked up from `directory` as `which` does. The
/// program's own files are not the machine's, so the machine's shell answers.
pub async fn which(program: &OsStr, directory: &Path) -> Option<PathBuf> {
    let output = Command::new("/bin/sh")
        .args(["-c", "command -v \"$1\"", "sh"])
        .arg(program)
        .current_dir(directory)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let found = String::from_utf8(output.stdout).ok()?;
    let found = found.trim();
    (!found.is_empty()).then(|| PathBuf::from(found))
}

pub use gpui_util::new_std_command;

pub fn new_command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

/// A machine's command for a std one, as the other modules build them.
pub fn from_std(command: &std::process::Command) -> Command {
    let mut ours = Command::new(command.get_program());
    ours.args(command.get_args());
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            ours.env(name, value);
        }
    }
    if let Some(directory) = command.get_current_dir() {
        ours.current_dir(directory);
    }
    ours
}
