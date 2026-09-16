// Included by git_plane::tests: exercise production admission/supervision and
// real HTTP publication without adding production fault-injection controls.

#[derive(Clone)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("git-plane condition did not converge");
}

async fn http_snapshot_server(
    store: crate::core::store::Store,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = crate::api::router(crate::api::AppState {
        store,
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/snapshot", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn g492_poisoned_state_keeps_event_path_and_probe_serving() {
    let plane = Arc::new(plane_with_state(|_| {}));
    let poison = plane.clone();
    assert!(
        std::thread::spawn(move || {
            let _cache = poison.lock_state();
            panic!("G492_INITIAL_LOCK_HOLDER_PANIC");
        })
        .join()
        .is_err()
    );
    assert!(plane.state.is_poisoned());
    // This is the exact historical secondary-panic entry point.
    let _ = plane.map_event_path(Path::new("/not-watched/HEAD"));
    let (sink, mut rx) = crate::core::plane_channel();
    let wt = Path::new("/fake/worktree");
    plane
        .apply_probe(
            wt,
            Instant::now(),
            Ok(Probe {
                branch: "fresh".into(),
                commit: "new-head".into(),
                subject: None,
                status: GitStatus::default(),
            }),
            &sink,
        )
        .await;
    assert!(
        matches!(rx.recv().await, Some(PlaneEvent::Git(GitEvent::HeadMoved { commit, .. })) if commit == "new-head")
    );
    assert_eq!(
        plane.lock_state().worktrees[wt].commit.as_deref(),
        Some("new-head")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g492_supervisor_respawns_poisoned_child_and_wire_reports_stopped() {
    use tracing::instrument::WithSubscriber;
    let _guard = PROBE_LOCK.lock().await;
    let (_temp, repo, _) = scratch_repo("g492-supervision");
    let store = crate::core::store::Store::new();
    let health = store.git_plane_health();
    let plane =
        Arc::new(GitPlane::new(repo.clone(), repo.join("worktrees")).with_health(health.clone()));
    let (sink, mut rx) = crate::core::plane_channel();
    let log = LogCapture(Arc::new(Mutex::new(Vec::new())));
    let capture = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || capture.clone())
        .finish();
    let supervisor = tokio::spawn(plane.clone().supervise(sink).with_subscriber(subscriber));
    until(|| plane.generation.load(Ordering::Acquire) == 1).await;
    let poison = plane.clone();
    plane.spawn(false, async move {
        let _cache = poison.lock_state();
        panic!("G492_SUPERVISED_POISON_PAYLOAD");
    });
    until(|| plane.generation.load(Ordering::Acquire) >= 2).await;
    // Recovered task must actually probe, not just increment a restart counter.
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if matches!(
                rx.recv().await,
                Some(PlaneEvent::Git(GitEvent::HeadMoved { .. }))
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert!(health.alive.load(Ordering::Acquire));
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("ERROR") && text.contains("G492_SUPERVISED_POISON_PAYLOAD"),
        "{text}"
    );
    assert!(text.contains("restarting generation"));
    println!("{text}");
    let coalescer_store = store.clone();
    let coalescer = tokio::spawn(async move { coalescer_store.run_coalescer().await });
    store.flush().await;
    let (url, server) = http_snapshot_server(store.clone()).await;
    let client = reqwest::Client::new();
    let alive: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(alive["git_plane_alive"], true);
    assert!(alive["git_plane_last_event_age_ms"].is_u64());
    drop(rx); // Stop just the plane, while HTTP and the coalescer stay live.
    tokio::time::timeout(Duration::from_secs(3), supervisor)
        .await
        .unwrap()
        .unwrap();
    let mut stopped = serde_json::Value::Null;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            stopped = client.get(&url).send().await.unwrap().json().await.unwrap();
            if stopped["git_plane_alive"] == false {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        stopped["git_plane_last_event_age_ms"].as_u64().unwrap()
            >= alive["git_plane_last_event_age_ms"].as_u64().unwrap()
    );
    println!(
        "G492_REAL_HTTP_LIVENESS alive={} stopped={}",
        alive["git_plane_alive"], stopped["git_plane_alive"]
    );
    coalescer.abort();
    server.abort();
    let _ = coalescer.await;
    let _ = server.await;
    assert!(plane.tasks.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g492_supervisor_restarts_a_silent_stall_but_not_idle_loops() {
    let _guard = PROBE_LOCK.lock().await;
    let (_temp, repo, _) = scratch_repo("g492-stall");
    let mut plane = GitPlane::new(repo.clone(), repo.join("worktrees"));
    plane.stall_timeout = Duration::from_millis(200);
    let plane = Arc::new(plane);
    let held = plane
        .git_command_budget
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    let (sink, rx) = crate::core::plane_channel();
    let supervisor = tokio::spawn(plane.clone().supervise(sink));
    until(|| plane.generation.load(Ordering::Acquire) == 1).await;
    until(|| !plane.health.alive.load(Ordering::Acquire)).await;
    drop(held);
    until(|| plane.generation.load(Ordering::Acquire) == 2).await;
    // Startup rescan delay is 500ms; use production's idle heartbeat bound
    // for the healthy control rather than an unrealistically short timeout.
    drop(rx);
    tokio::time::timeout(Duration::from_secs(3), supervisor)
        .await
        .unwrap()
        .unwrap();

    let quiet = Arc::new(GitPlane::new(repo.clone(), repo.join("worktrees")));
    let (sink, rx) = crate::core::plane_channel();
    quiet.clone().start(sink);
    until(|| quiet.generation.load(Ordering::Acquire) == 1).await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(quiet.generation.load(Ordering::Acquire), 1);
    assert!(quiet.health.alive.load(Ordering::Acquire));
    drop(rx);
    until(|| !quiet.supervising.load(Ordering::Acquire)).await;
}

#[tokio::test]
async fn g492_permit_queue_is_not_execution_budget_and_sweep_is_paced() {
    let permits = Arc::new(Semaphore::new(4));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let fake_git = |worktree: PathBuf| {
        let (permits, active, maximum) = (permits.clone(), active.clone(), maximum.clone());
        async move {
            with_probe_budget(permits, async move {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(count, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(worktree)
            })
            .await
        }
    };
    let worktrees: Vec<_> = (0..100)
        .map(|i| PathBuf::from(format!("fake-wt-{i}")))
        .collect();
    let start = Instant::now();
    let results = futures::future::join_all(worktrees.clone().into_iter().map(fake_git)).await;
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(ProbeError::OverBudget)))
            .count(),
        0,
        "permit wait was incorrectly charged to the execution budget"
    );
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(maximum.load(Ordering::SeqCst), 4);
    assert!(start.elapsed() < SWEEP_INTERVAL);
    let interval = Duration::from_secs(6);
    let start = Instant::now();
    let results: Vec<_> = paced_probes(worktrees, interval, fake_git).collect().await;
    assert_eq!(results.len(), 100);
    assert!(results.iter().all(Result::is_ok));
    assert!(
        start.elapsed() >= Duration::from_secs(5),
        "sweep launched a burst instead of pacing"
    );
    assert!(start.elapsed() < interval + Duration::from_secs(1));
    println!(
        "G492_PERMITS 100 fake worktrees x 50ms, max=4, OverBudget=0; paced_ms={}",
        start.elapsed().as_millis()
    );
}

#[tokio::test]
async fn g492_warning_subscriber_cannot_poison_the_cache() {
    use futures::FutureExt;
    use tracing::instrument::WithSubscriber;
    let plane = plane_with_state(|state| {
        state
            .worktrees
            .insert(PathBuf::from("/fake/warnings"), WorktreeState::default());
    });
    let subscriber = tracing_subscriber::fmt()
        .with_writer(|| -> std::io::Sink {
            panic!("G492_FAILING_WARN_WRITER");
        })
        .finish();
    let (sink, _rx) = crate::core::plane_channel();
    let probe = plane
        .apply_probe(
            Path::new("/fake/warnings"),
            Instant::now(),
            Err(ProbeError::OverBudget),
            &sink,
        )
        .with_subscriber(subscriber);
    assert!(
        std::panic::AssertUnwindSafe(probe)
            .catch_unwind()
            .await
            .is_err()
    );
    assert!(
        !plane.state.is_poisoned(),
        "a failing WARN subscriber must not hold the cache lock"
    );
    assert!(plane.lock_state().worktrees[Path::new("/fake/warnings")].stale);
}

#[tokio::test]
async fn g492_warning_limit_preserves_every_skipped_probe_in_counter() {
    let plane = plane_with_state(|_| {});
    let wt = PathBuf::from("/fake/warnings");
    plane
        .lock_state()
        .worktrees
        .insert(wt.clone(), WorktreeState::default());
    let (sink, _rx) = crate::core::plane_channel();
    for _ in 0..1000 {
        plane
            .apply_probe(&wt, Instant::now(), Err(ProbeError::OverBudget), &sink)
            .await;
    }
    assert_eq!(plane.health.skipped.load(Ordering::Relaxed), 1000);
    assert!(plane.backlog.load(Ordering::Acquire));
    let mut state = plane.lock_state();
    let wt = state.worktrees.get_mut(&wt).unwrap();
    let first = wt.last_budget_warning.unwrap();
    assert!(!wt.warn_over_budget(first + WARN_INTERVAL - Duration::from_millis(1)));
    assert!(wt.warn_over_budget(first + WARN_INTERVAL));
    assert!(!wt.warn_over_budget(first + WARN_INTERVAL));
}

#[tokio::test]
async fn g492_snapshot_marks_old_absent_and_stopped_facts() {
    let store = crate::core::store::Store::new();
    let health = store.git_plane_health();
    health.enabled.store(true, Ordering::Release);
    health.alive.store(true, Ordering::Release);
    health.progress();
    let mut agent = load_test_agent(1);
    agent.workspace.worktree_path = Some("/fake/stale".into());
    agent.workspace.head_sha = Some("historical-head".into());
    agent.workspace.behind = 25;
    store.apply(crate::core::model::Change::upsert(agent)).await;
    {
        let mut facts = health.facts.lock().unwrap();
        facts.insert(
            PathBuf::from("/fake/stale"),
            Some(Instant::now() - Duration::from_secs(121)),
        );
        facts.insert(PathBuf::from("/fake/fresh"), Some(Instant::now()));
        facts.insert(PathBuf::from("/fake/absent"), None);
    }
    store.flush().await;
    let (url, server) = http_snapshot_server(store.clone()).await;
    let client = reqwest::Client::new();
    let snapshot: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    let facts = &snapshot["git_worktree_facts"];
    assert_eq!(
        facts["/fake/stale"]["stale"], true,
        "old git facts must never be marked current"
    );
    assert!(facts["/fake/stale"]["fact_age_ms"].as_u64().unwrap() >= 121_000);
    assert_eq!(facts["/fake/fresh"]["stale"], false);
    assert_eq!(facts["/fake/absent"]["stale"], true);
    assert!(facts["/fake/absent"]["fact_age_ms"].is_null());
    assert_eq!(
        snapshot["agents"]["herdr:git-load"]["workspace"]["behind"], 25,
        "policy keeps historical values with an explicit marker, not fabricated fresh zeros"
    );
    health.alive.store(false, Ordering::Release);
    store.flush().await;
    let stopped: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(stopped["git_worktree_facts"]["/fake/fresh"]["stale"], true);
    assert_eq!(snapshot["epoch"], stopped["epoch"]);
    server.abort();
    let _ = server.await;
}
