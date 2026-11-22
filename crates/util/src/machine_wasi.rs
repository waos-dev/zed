//! WaOS port: the machine a WASI component's commands run on (`wasdk::machines`), and what it
//! says of itself. Natively these are facts of Zed's own process, read where they are needed:
//! `cfg!(target_os)`, `std::env::consts::ARCH`, `$SHELL`. A component has no system of its own,
//! so they are the machine's: what it runs (`platform`) and its user's login shell.
//!
//! The machine is reached asynchronously, the user asked to pick one if none was granted, so
//! the program connects to it once (`connect`) before it opens a project, as Zed connects to an
//! SSH host before opening a remote project. Its facts are then answered at once; before that,
//! there are none.
//!
//! The project lives on the machine: once connected, the machine's root is mounted at `/` in
//! Zed's own files (`wasdk::mounts`), so Zed's file layer, its git code's own reads of `.git` and
//! every other path read the machine's files at the paths its commands name them by, Zed's own
//! `/data`, `/cache` and `/tmp` staying its own; and `HOME` is the machine's, where Zed's folder
//! prompt starts (WaOS's projects-on-machines proposal).

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use wasdk::machines::{self, Machine, Platform};

thread_local! {
    /// The machine the program's commands run on, once connected.
    static MACHINE: RefCell<Option<Rc<Machine>>> = const { RefCell::new(None) };
    /// The machine's root, mounted at `/` for the rest of the run.
    static ROOT_MOUNTED: RefCell<Option<wasdk::mounts::Mounted>> = const { RefCell::new(None) };
}

/// The machine the user granted the program, or one they pick now; the same one from then on.
pub async fn connect() -> io::Result<Rc<Machine>> {
    if let Some(machine) = connected() {
        return Ok(machine);
    }
    let machines = machines::get_machines()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "WaOS gives no machines here"))?;
    let machine = match machines.granted().into_iter().next() {
        Some(machine) => machine,
        None => machines.pick().await.ok_or_else(|| {
            io::Error::new(io::ErrorKind::PermissionDenied, "no machine was picked")
        })?,
    };
    let machine = Rc::new(machine);
    MACHINE.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&machine)));
    log::info!(
        "machine {:?}: {:?}, login shell {:?}",
        machine.name(),
        machine.platform(),
        machine.login_shell()
    );
    mount_root(&machine)?;
    take_its_home().await;
    Ok(machine)
}

/// Mounts `machine`'s root at `/`, so its files are read at their own paths.
fn mount_root(machine: &Machine) -> io::Result<()> {
    let mounted = wasdk::mounts::mount(&machine.root(), "/")
        .map_err(|error| io::Error::other(format!("the machine's root was not mounted: {error:?}")))?;
    ROOT_MOUNTED.with(|cell| *cell.borrow_mut() = Some(mounted));
    log::info!("the machine's root is mounted at /");
    Ok(())
}

/// The machine's `HOME`, as its login environment has it, taken as the program's and as Zed's
/// `~` (`paths::home_dir`), where a folder prompt with no project starts; left as it is where the
/// machine says none.
async fn take_its_home() {
    let output = crate::command::new_command("printenv").arg("HOME").output().await;
    let home = match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        Ok(_) | Err(_) => String::new(),
    };
    if home.is_empty() {
        log::warn!("the machine says no HOME");
        return;
    }
    log::info!("the machine's home is {home}");
    crate::paths::set_home_dir(std::path::PathBuf::from(&home));
    // SAFETY: a component has one thread, so nothing reads the environment meanwhile.
    unsafe { std::env::set_var("HOME", home) };
}

/// The machine, once `connect` has answered.
pub fn connected() -> Option<Rc<Machine>> {
    MACHINE.with(|machine| machine.borrow().clone())
}

/// What the connected machine runs.
pub fn platform() -> Option<Platform> {
    connected().map(|machine| machine.platform())
}

/// The connected machine's login shell; none before connecting, and on Windows.
pub fn login_shell() -> Option<String> {
    connected().and_then(|machine| machine.login_shell())
}
