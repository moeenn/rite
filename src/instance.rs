//! Keeps a single editor window: a second `rite` hands its files to the first
//! one over a unix socket and exits.

use iced::futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Files to open, with the xdg-activation token of the launcher if it gave one.
pub type Batch = (Vec<PathBuf>, Option<String>);

static INCOMING: Mutex<Option<UnboundedReceiver<Batch>>> = Mutex::new(None);

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("rite.sock")
}

/// Sends the files to a running instance. Returns false when there is none,
/// in which case this process becomes the instance.
pub fn forward(files: &[PathBuf]) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    // The first line is the activation token, empty when there is none.
    let mut payload = Vec::new();
    payload.extend_from_slice(std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default().as_bytes());
    payload.push(b'\n');
    for file in files {
        payload.extend_from_slice(cwd.join(file).to_string_lossy().as_bytes());
        payload.push(b'\n');
    }
    stream.write_all(&payload).is_ok()
}

/// Starts listening for files forwarded by later invocations.
pub fn listen() {
    let path = socket_path();
    // A socket left behind by a crashed instance would block the bind.
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    let (tx, rx) = unbounded();
    *INCOMING.lock().unwrap() = Some(rx);
    std::thread::spawn(move || serve(listener, tx));
}

fn serve(listener: UnixListener, tx: UnboundedSender<Batch>) {
    for stream in listener.incoming().flatten() {
        let mut text = String::new();
        if { stream }.read_to_string(&mut text).is_err() {
            continue;
        }
        let mut lines = text.lines();
        let token = lines.next().filter(|t| !t.is_empty()).map(str::to_string);
        let files = lines.filter(|l| !l.is_empty()).map(Path::new).map(Path::to_path_buf).collect();
        if tx.unbounded_send((files, token)).is_err() {
            break;
        }
    }
}

/// Batches forwarded by other invocations; the files are empty when only a
/// raise of the window was requested.
pub fn incoming() -> UnboundedReceiver<Batch> {
    INCOMING.lock().unwrap().take().unwrap_or_else(|| unbounded().1)
}
