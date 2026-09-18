// Minimal reproduction of GitPlane::new_commondir_watcher (notify 8.2.0) on one
// commondir: create a RecommendedWatcher, watch(<repo>/.git, Recursive), then
// print every event delivered by the backend for 2 seconds.
//
// Purpose: establish whether the watcher's OWN registration pass delivers
// events for the commondir's children (which map_event_path would resolve to
// the main checkout), and whether they arrive after `watch()` returns.
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn main() {
    let repo = std::env::args().nth(1).expect("usage: g573probe <repo>");
    let commondir = PathBuf::from(&repo).join(".git");
    let (tx, rx) = mpsc::channel::<(PathBuf, notify::Result<Event>)>();
    let callback_source = commondir.clone();
    let callback_tx = tx.clone();
    let t0 = Instant::now();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            let _ = callback_tx.send((callback_source.clone(), res));
        },
        Config::default(),
    )
    .expect("watcher");
    watcher
        .watch(&commondir, RecursiveMode::Recursive)
        .expect("watch");
    println!(
        "REGISTERED_AT_MS={} watcher_kind={:?}",
        t0.elapsed().as_millis(),
        RecommendedWatcher::kind()
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut n = 0usize;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok((_cd, Ok(ev))) => {
                n += 1;
                println!(
                    "EVENT t_ms={} kind={:?} paths={:?}",
                    t0.elapsed().as_millis(),
                    ev.kind,
                    ev.paths
                );
            }
            Ok((_cd, Err(e))) => println!("WATCH_ERR {e:?}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    println!("TOTAL_EVENTS={n}");
}
