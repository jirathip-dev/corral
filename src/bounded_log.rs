//! Synchronous, bounded diagnostics: two 1 MiB files, no lossy queue and no
//! severity-based dropping. Rotate before crossing the cap, including ERROR
//! and panic records. I/O failure falls back to stderr; if that also fails,
//! fail-stop rather than pretend that diagnostics were recorded. No service
//! restart or external rotator is involved. The bound is retained history,
//! not a promise to preserve an unlimited lifetime of records.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing_subscriber::fmt::MakeWriter;

const CAP: u64 = 1024 * 1024;

#[derive(Clone)]
pub struct BoundedLog(Arc<Mutex<Files>>);

struct Files {
    path: PathBuf,
    file: Option<File>,
    len: u64,
    cap: u64,
}

impl BoundedLog {
    pub fn open(dir: &Path) -> Self {
        Self::with_cap(dir.join("corrald.log"), CAP)
    }

    fn with_cap(path: PathBuf, cap: u64) -> Self {
        let mut files = Files {
            path,
            file: None,
            len: 0,
            cap,
        };
        // On failure, the first write reports it via the emergency sink.
        let _ = files.reopen();
        Self(Arc::new(Mutex::new(files)))
    }

    pub fn install_panic_hook(&self) {
        let log = self.clone();
        std::panic::set_hook(Box::new(move |panic| {
            // Same bounded writer as tracing, not an unbounded launchd fd.
            // Write returns success only after a real sink accepted the bytes.
            let _ = writeln!(log.make_writer(), "ERROR CORRAL_ERROR_OR_PANIC: {panic}");
        }));
    }
}

impl Files {
    fn reopen(&mut self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&self.path)?;
        self.len = file.metadata()?.len();
        if self.len > self.cap {
            file.set_len(self.cap)?;
            self.len = self.cap;
        }
        self.file = Some(file);
        Ok(())
    }

    fn record(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        if self.file.is_none() {
            self.reopen()?;
        }
        while !bytes.is_empty() {
            if self.len >= self.cap {
                // Unix rename atomically replaces the previous generation.
                // Keep the current handle until replacement succeeds.
                fs::rename(&self.path, self.path.with_extension("log.1"))?;
                self.file = None;
                self.reopen()?;
            }
            let count = bytes.len().min((self.cap - self.len) as usize);
            let Some(file) = &mut self.file else {
                return Err(io::Error::other("diagnostic file unavailable"));
            };
            file.write_all(&bytes[..count])?;
            self.len += count as u64;
            bytes = &bytes[count..];
        }
        Ok(())
    }
}

pub struct LogGuard<'a>(MutexGuard<'a, Files>);

impl<'a> MakeWriter<'a> for BoundedLog {
    type Writer = LogGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        LogGuard(self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Write for LogGuard<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = self.0.record(bytes) {
            self.0.file = None; // reopen/recount after any partial write failure
            // Best-effort notice must not itself panic inside a cache holder.
            let mut emergency = io::stderr().lock();
            let _ = writeln!(emergency, "CORRAL_LOG_IO_FAILURE: {error}; using stderr");
            if emergency.write_all(bytes).is_err() {
                // Neither sink accepted this record. Avoid a silent subscriber
                // drop or recursive panic-hook failure; signal fatal failure.
                std::process::abort();
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(()) // File::write_all is synchronous; no userspace queue to flush.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_and_panic_survive_rotation() {
        const CHILD: &str = "CORRAL_TEST_BOUNDED_PANIC_DIR";
        if let Some(dir) = std::env::var_os(CHILD) {
            let log = BoundedLog::with_cap(PathBuf::from(dir).join("corrald.log"), 1024);
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(log.clone())
                .init();
            log.install_panic_hook();
            log.make_writer().write_all(&[b'x'; 1024]).unwrap();
            tracing::error!("G492_ERROR_AT_CAP");
            let remaining = 1024 - log.0.lock().unwrap().len;
            log.make_writer()
                .write_all(&vec![b'y'; remaining as usize])
                .unwrap();
            panic!("G492_PANIC_AT_CAP");
        }
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bounded_log::tests::error_and_panic_survive_rotation",
                "--nocapture",
            ])
            .env(CHILD, dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success(), "the child must really panic");
        let current = fs::read(dir.path().join("corrald.log")).unwrap();
        let previous = fs::read(dir.path().join("corrald.log.1")).unwrap();
        assert!(current.len() <= 1024 && previous.len() <= 1024);
        assert!(String::from_utf8_lossy(&current).contains("G492_PANIC_AT_CAP"));
        assert!(String::from_utf8_lossy(&current).contains("CORRAL_ERROR_OR_PANIC"));
        assert!(String::from_utf8_lossy(&previous).contains("G492_ERROR_AT_CAP"));
        println!("G492_ERROR_OR_PANIC: ERROR and real panic survived full-file rotation");
    }

    #[test]
    fn oversized_writes_are_bounded_and_poison_is_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let log = BoundedLog::with_cap(dir.path().join("corrald.log"), 64);
        let copy = log.clone();
        let _ = std::thread::spawn(move || {
            let _guard = copy.0.lock().unwrap();
            panic!("poison writer");
        })
        .join();
        log.make_writer().write_all(&[b'x'; 1024]).unwrap();
        log.make_writer()
            .write_all(b"ERROR_AFTER_POISON\n")
            .unwrap();
        for name in ["corrald.log", "corrald.log.1"] {
            assert!(fs::metadata(dir.path().join(name)).unwrap().len() <= 64);
        }
        assert_eq!(
            fs::read(dir.path().join("corrald.log")).unwrap(),
            b"ERROR_AFTER_POISON\n"
        );
    }
}
