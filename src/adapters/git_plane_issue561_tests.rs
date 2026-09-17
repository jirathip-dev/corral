// Included by git_plane::tests. Only temporary repositories; no live daemon.

async fn g561_stop(
    plane: &GitPlane,
    rx: mpsc::Receiver<PlaneEvent>,
    task: tokio::task::JoinHandle<()>,
) {
    drop(rx);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(plane.tasks.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g561_generation_preserves_facts() {
    let _guard = PROBE_LOCK.lock().await;
    let (_temp, repo, sha) = scratch_repo("g561-survive");
    let repo = fs::canonicalize(repo).unwrap();
    let plane = Arc::new(GitPlane::new(repo.clone(), repo.join("worktrees")));
    let (sink, rx) = crate::core::plane_channel();
    let supervisor = tokio::spawn(plane.clone().supervise(sink));
    until(|| {
        plane
            .health
            .facts
            .lock()
            .unwrap()
            .get(&repo)
            .is_some_and(Option::is_some)
            && plane.lock_state().pending.is_empty()
    })
    .await;
    let permits = plane
        .git_command_budget
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    let observed = plane.health.facts.lock().unwrap()[&repo];
    let status = plane.lock_state().worktrees[&repo].status.clone();
    let subject = plane.lock_state().worktrees[&repo].subject.clone();
    plane.spawn(true, async {}); // critical loop ended normally, not a panic
    until(|| plane.generation.load(Ordering::Acquire) == 2).await;
    // Git admission is held: an eager re-hydration cannot hide a cleared cache.
    let (commit_after, status_after, subject_after) = {
        let state = plane.lock_state();
        let wt = &state.worktrees[&repo];
        (wt.commit.clone(), wt.status.clone(), wt.subject.clone())
    };
    let observed_after = plane.health.facts.lock().unwrap()[&repo];
    g561_stop(&plane, rx, supervisor).await;
    drop(permits);
    assert_eq!(
        commit_after.as_deref(),
        Some(sha.as_str()),
        "generation restart cleared cached commit"
    );
    assert_eq!(
        status_after, status,
        "generation restart cleared cached status"
    );
    assert_eq!(
        subject_after, subject,
        "generation restart cleared cached subject"
    );
    assert_eq!(
        observed_after, observed,
        "restart changed observation age without a probe"
    );
}

fn g561_git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g561_changed_inventory_and_missed_change_converge() {
    let _guard = PROBE_LOCK.lock().await;
    let (temp, repo, old_sha) = scratch_repo("g561-freshness");
    let repo = fs::canonicalize(repo).unwrap();
    let worktrees = temp.path().join("worktrees");
    let removed = worktrees.join("removed");
    let added = worktrees.join("added");
    g561_git(
        &repo,
        &["worktree", "add", "--detach", removed.to_str().unwrap()],
    );
    let removed = fs::canonicalize(removed).unwrap();
    let store = crate::core::store::Store::new();
    let plane =
        Arc::new(GitPlane::new(repo.clone(), worktrees).with_health(store.git_plane_health()));
    let (sink, mut rx) = crate::core::plane_channel();
    let supervisor = tokio::spawn(plane.clone().supervise(sink));
    until(|| {
        let facts = plane.health.facts.lock().unwrap();
        facts.len() == 2
            && facts.values().all(Option::is_some)
            && plane.lock_state().pending.is_empty()
    })
    .await;
    let permits = plane
        .git_command_budget
        .clone()
        .acquire_many_owned(4)
        .await
        .unwrap();
    let observed = plane.health.facts.lock().unwrap()[&repo];
    while rx.try_recv().is_ok() {}
    plane.spawn(true, async {});
    until(|| !plane.health.alive.load(Ordering::Acquire)).await;
    // Old watcher has been joined; hold Git admission across replacement.
    until(|| plane.tasks.lock().unwrap().is_empty()).await;
    g561_git(&repo, &["worktree", "remove", removed.to_str().unwrap()]);
    g561_git(
        &repo,
        &["commit", "--allow-empty", "-m", "changed while stopped"],
    );
    g561_git(
        &repo,
        &["worktree", "add", "--detach", added.to_str().unwrap()],
    );
    let added = fs::canonicalize(added).unwrap();
    // Untracked working-directory data is outside the metadata watcher.
    fs::write(repo.join("missed.txt"), "freshness witness").unwrap();
    until(|| plane.generation.load(Ordering::Acquire) == 2).await;
    assert_eq!(
        plane.lock_state().worktrees[&repo].commit.as_deref(),
        Some(old_sha.as_str())
    );
    assert_eq!(plane.health.facts.lock().unwrap()[&repo], observed);
    drop(permits);
    let started = Instant::now();
    tokio::time::timeout(SWEEP_INTERVAL + Duration::from_secs(5), async {
        loop {
            let ready = {
                let state = plane.lock_state();
                state.worktrees.len() == 2
                    && !state.worktrees.contains_key(&removed)
                    && state
                        .worktrees
                        .get(&added)
                        .is_some_and(|st| st.commit.is_some() && st.status.is_some())
                    && state.worktrees[&repo].commit.as_deref() != Some(old_sha.as_str())
                    && state.worktrees[&repo]
                        .status
                        .as_ref()
                        .is_some_and(|st| st.dirty_worktree)
            };
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("changed facts did not converge within one paced sweep + 5s allowance");
    let mut saw_added = false;
    let mut saw_removed = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            PlaneEvent::Git(GitEvent::WorktreeAdded { worktree }) if worktree == added => {
                saw_added = true
            }
            PlaneEvent::Git(GitEvent::WorktreeRemoved { worktree }) if worktree == removed => {
                saw_removed = true
            }
            _ => {}
        }
    }
    assert!(saw_added && saw_removed);
    assert!(!plane.health.facts.lock().unwrap().contains_key(&removed));
    assert!(plane.health.facts.lock().unwrap()[&repo] > observed);
    let skipped = plane.health.skipped.load(Ordering::Relaxed);
    let (sink, _rx) = crate::core::plane_channel();
    plane
        .apply_probe(&repo, Instant::now(), Err(ProbeError::OverBudget), &sink)
        .await;
    assert_eq!(plane.health.skipped.load(Ordering::Relaxed), skipped + 1);
    store.flush().await;
    let (url, server) = http_snapshot_server(store.clone()).await;
    let client = reqwest::Client::new();
    let alive: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(alive["git_plane_alive"], true);
    assert!(alive["git_plane_last_event_age_ms"].is_u64());
    assert_eq!(alive["git_plane_skipped"], skipped + 1);
    println!(
        "G561_FRESHNESS convergence_ms={} alive={} age_ms={} skipped={}",
        started.elapsed().as_millis(),
        alive["git_plane_alive"],
        alive["git_plane_last_event_age_ms"],
        alive["git_plane_skipped"]
    );
    g561_stop(&plane, rx, supervisor).await;
    store.flush().await;
    let stopped: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(stopped["git_plane_alive"], false);
    assert!(stopped["git_plane_last_event_age_ms"].is_u64());
    assert_eq!(stopped["git_plane_skipped"], skipped + 1);
    server.abort();
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g561_panic_between_cache_and_send_reemits_all_facts() {
    use tracing::instrument::WithSubscriber;
    let _guard = PROBE_LOCK.lock().await;
    let (temp, repo, sha) = scratch_repo("g561-panic");
    let repo = fs::canonicalize(repo).unwrap();
    let linked = temp.path().join("linked");
    g561_git(
        &repo,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let linked = fs::canonicalize(linked).unwrap();
    let plane = Arc::new(GitPlane::new(repo.clone(), linked.clone()));
    let (sink, mut rx) = crate::core::plane_channel();
    let supervisor = tokio::spawn(plane.clone().supervise(sink.clone()));
    until(|| {
        let facts = plane.health.facts.lock().unwrap();
        facts.len() == 2
            && facts.values().all(Option::is_some)
            && plane.lock_state().pending.is_empty()
    })
    .await;
    while rx.try_recv().is_ok() {}
    let subscriber = tracing_subscriber::fmt()
        .with_writer(|| -> std::io::Sink {
            panic!("G561_PANIC_AFTER_CACHE_BEFORE_SEND");
        })
        .finish();
    let child = plane.clone();
    let path = repo.clone();
    plane.spawn(false, async move {
        child
            .apply_probe(
                &path,
                Instant::now(),
                Ok(Probe {
                    branch: "main".into(),
                    commit: "not-emitted".into(),
                    subject: Some("not-emitted".into()),
                    status: GitStatus::default(),
                }),
                &sink,
            )
            .with_subscriber(subscriber)
            .await;
    });
    until(|| plane.generation.load(Ordering::Acquire) == 2).await;
    let mut heads = HashSet::new();
    let mut statuses = HashSet::new();
    tokio::time::timeout(Duration::from_secs(8), async {
        while heads.len() != 2 || statuses.len() != 2 {
            match rx.recv().await.unwrap() {
                PlaneEvent::Git(GitEvent::HeadMoved {
                    worktree, commit, ..
                }) => {
                    assert_eq!(commit, sha);
                    heads.insert(worktree);
                }
                PlaneEvent::Git(GitEvent::DirtyChanged { worktree, .. }) => {
                    statuses.insert(worktree);
                }
                _ => {}
            }
        }
    })
    .await
    .expect("panic recovery must re-emit head AND status for both worktrees");
    assert_eq!(heads, HashSet::from([repo, linked]));
    assert_eq!(statuses, heads);
    g561_stop(&plane, rx, supervisor).await;
}

#[tokio::test]
async fn g561_cancelled_publication_replays_complete_facts() {
    let plane = Arc::new(plane_with_state(|_| {}));
    let wt = PathBuf::from("/fake/g561-cancelled");
    let (blocked, mut rx) = mpsc::channel(1);
    blocked
        .send(PlaneEvent::Git(GitEvent::WorktreeAdded {
            worktree: wt.clone(),
        }))
        .await
        .unwrap();
    let child = plane.clone();
    let path = wt.clone();
    let probe = || Probe {
        branch: "main".into(),
        commit: "same".into(),
        subject: None,
        status: GitStatus::default(),
    };
    let task = tokio::spawn(async move {
        child
            .apply_probe(&path, Instant::now(), Ok(probe()), &blocked)
            .await;
    });
    until(|| {
        plane
            .lock_state()
            .worktrees
            .get(&wt)
            .is_some_and(|st| st.publication_pending)
    })
    .await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(rx.recv().await.is_some());
    let (sink, mut rx) = crate::core::plane_channel();
    plane
        .apply_probe(&wt, Instant::now(), Ok(probe()), &sink)
        .await;
    assert!(matches!(
        rx.recv().await,
        Some(PlaneEvent::Git(GitEvent::HeadMoved { .. }))
    ));
    assert!(matches!(
        rx.recv().await,
        Some(PlaneEvent::Git(GitEvent::DirtyChanged { .. }))
    ));
    assert!(!plane.lock_state().worktrees[&wt].publication_pending);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn g561_watcher_boot_has_zero_hydration_for_known_facts() {
    let _guard = PROBE_LOCK.lock().await;
    let (_temp, repo, _) = scratch_repo("g561-no-burst");
    let repo = fs::canonicalize(repo).unwrap();
    let plane = Arc::new(GitPlane::new(repo.clone(), repo.join("worktrees")));
    let (sink, rx) = crate::core::plane_channel();
    plane.rescan(&sink).await;
    let probe = probe_worktree(&repo, None, None, plane.git_command_budget.clone()).await;
    plane.apply_probe(&repo, Instant::now(), probe, &sink).await;
    let calls = TEST_PROBE_RUNS.load(Ordering::SeqCst);
    // Run the real watcher bootstrap separately from the paced safety net.
    // Neither a supervisor nor a status sweep can hide an eager watcher burst.
    let (tx, cmd_rx) = mpsc::channel(1);
    let watcher = tokio::spawn(plane.clone().run_watcher(sink, cmd_rx, tx));
    until(|| plane.progress_ms[0].load(Ordering::Acquire) > 0).await;
    tokio::time::sleep(DEBOUNCE + Duration::from_millis(200)).await;
    let hydration_probes = TEST_PROBE_RUNS.load(Ordering::SeqCst) - calls;
    plane.stopped.store(true, Ordering::Relaxed);
    drop(rx);
    tokio::time::timeout(Duration::from_secs(3), watcher)
        .await
        .unwrap()
        .unwrap();
    plane.tasks.lock().unwrap().abort_all();
    while plane.next_task().await.is_some() {}
    assert_eq!(
        hydration_probes, 0,
        "unchanged restart must not eagerly hydrate known paths"
    );
}

fn g561_cpu_seconds() -> f64 {
    [libc::RUSAGE_SELF, libc::RUSAGE_CHILDREN]
        .into_iter()
        .map(|who| {
            let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
            // SAFETY: getrusage initializes the supplied rusage on success.
            assert_eq!(unsafe { libc::getrusage(who, usage.as_mut_ptr()) }, 0);
            let usage = unsafe { usage.assume_init() };
            [usage.ru_utime, usage.ru_stime]
                .into_iter()
                .map(|t| t.tv_sec as f64 + t.tv_usec as f64 / 1_000_000.0)
                .sum::<f64>()
        })
        .sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "three 65-second CPU/wall windows over 130 temporary worktrees"]
async fn g561_restart_cost_measurement() {
    let _guard = PROBE_LOCK.lock().await;
    let (temp, repo, _) = scratch_repo("g561-cost");
    let repo = fs::canonicalize(repo).unwrap();
    let worktrees = temp.path().join("worktrees");
    fs::create_dir(&worktrees).unwrap();
    // Read-only host census at the pinned base: 107 herdr paths + 23 Projects roots.
    // One seeded repository with 129 linked worktrees, plus its main checkout.
    for i in 1..130 {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["worktree", "add", "--detach"])
            .arg(worktrees.join(format!("wt-{i}")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let plane = Arc::new(GitPlane::new(repo, worktrees));
    let (sink, mut rx) = crate::core::plane_channel();
    let supervisor = tokio::spawn(plane.clone().supervise(sink));
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let ready = {
                let facts = plane.health.facts.lock().unwrap();
                facts.len() == 130 && facts.values().all(Option::is_some)
            };
            if ready && plane.lock_state().pending.is_empty() {
                break;
            }
            while rx.try_recv().is_ok() {}
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initial hydration");
    for trial in 1..=3 {
        while rx.try_recv().is_ok() {}
        let generation = plane.generation.load(Ordering::Acquire);
        let calls = GIT_CALLS.load(Ordering::SeqCst);
        let cpu = g561_cpu_seconds();
        let started = Instant::now();
        plane.spawn(true, async {});
        until(|| plane.generation.load(Ordering::Acquire) > generation).await;
        let restart_ms = started.elapsed().as_millis();
        let restart = Instant::now();
        let mut emitted = 0;
        while restart.elapsed() < Duration::from_secs(65) {
            while rx.try_recv().is_ok() {
                emitted += 1;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let cpu_seconds = g561_cpu_seconds() - cpu;
        let wall_seconds = started.elapsed().as_secs_f64();
        println!(
            "G561_COST trial={trial} inventory=130 window_after_generation_s=65 restart_ms={restart_ms} wall_s={wall_seconds:.6} cpu_self_plus_reaped_children_s={cpu_seconds:.6} one_core_percent={:.6} git_calls={} emitted={} skipped={}",
            cpu_seconds / wall_seconds * 100.0,
            GIT_CALLS.load(Ordering::SeqCst) - calls,
            emitted,
            plane.health.skipped.load(Ordering::Relaxed)
        );
    }
    g561_stop(&plane, rx, supervisor).await;
}
