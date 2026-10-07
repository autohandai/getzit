mod common;

use common::Fixture;
use zit::evidence;
use zit::workspace;

/// Two checks scoped to their own directory; each run leaves a mark in `log`.
fn fixture() -> (Fixture, std::path::PathBuf) {
    let scratch = tempfile::tempdir().unwrap().keep();
    let log = scratch.join("runs.log");
    let config = format!(
        "[[check]]\nname = \"ui\"\nrun = \"echo ui >> {log} && test -f ui/ok\"\ninputs = [\"ui\"]\n\n\
         [[check]]\nname = \"api\"\nrun = \"echo api >> {log} && cat api/ok\"\ninputs = [\"api/\"]\n",
        log = log.display()
    );
    let fx = Fixture::new(&[("zit.toml", &config), ("ui/ok", "1\n"), ("api/ok", "api-output\n")]);
    (fx, log)
}

fn runs(log: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

#[test]
fn a_state_without_config_has_no_checks() {
    let fx = Fixture::new(&[("a.txt", "a\n")]);
    let cur = fx.repo.current().unwrap();
    assert!(evidence::verify(&fx.repo, &cur, false).unwrap().is_empty());
}

#[test]
fn verify_runs_each_check_against_the_state_and_records_the_result() {
    let (fx, log) = fixture();
    let cur = fx.repo.current().unwrap();
    let verdicts = evidence::verify(&fx.repo, &cur, false).unwrap();
    assert_eq!(runs(&log), ["ui", "api"]);
    assert!(verdicts.iter().all(|v| v.evidence.passed && !v.cached));
    assert_eq!(verdicts[1].evidence.output.trim(), "api-output");
    assert!(workspace::list(&fx.repo).unwrap().is_empty(), "check views are disposed");
}

#[test]
fn evidence_is_reused_for_the_same_inputs() {
    let (fx, log) = fixture();
    let cur = fx.repo.current().unwrap();
    evidence::verify(&fx.repo, &cur, false).unwrap();
    let again = evidence::verify(&fx.repo, &cur, false).unwrap();
    assert!(again.iter().all(|v| v.cached));
    assert_eq!(runs(&log).len(), 2);
}

#[test]
fn only_checks_whose_inputs_changed_run_again() {
    let (fx, log) = fixture();
    let cur = fx.repo.current().unwrap();
    evidence::verify(&fx.repo, &cur, false).unwrap();

    let c = fx.change("agent", &[("ui/button.txt", "new\n")]);
    let verdicts = evidence::verify(&fx.repo, &c.id, false).unwrap();
    let cached: Vec<(&str, bool)> = verdicts.iter().map(|v| (v.evidence.check.as_str(), v.cached)).collect();
    assert_eq!(cached, [("ui", false), ("api", true)]);
    assert_eq!(runs(&log), ["ui", "api", "ui"]);
}

#[test]
fn failures_are_evidence_too_and_can_be_rerun() {
    let (fx, log) = fixture();
    let ws = fx.workspace("agent");
    std::fs::remove_file(ws.path().join("ui/ok")).unwrap();
    let c = zit::change::record(&fx.repo, &ws.id, &Default::default()).unwrap().unwrap();
    workspace::dispose(&fx.repo, &ws.id).unwrap();

    let first = evidence::verify(&fx.repo, &c.id, false).unwrap();
    assert!(!first[0].evidence.passed);
    assert_ne!(first[0].evidence.exit_code, 0);
    assert!(evidence::verify(&fx.repo, &c.id, false).unwrap()[0].cached);
    assert!(!evidence::verify(&fx.repo, &c.id, true).unwrap()[0].cached);
    assert_eq!(runs(&log).iter().filter(|r| *r == "ui").count(), 2);
}

#[test]
fn lookup_reports_missing_evidence_without_running_anything() {
    let (fx, log) = fixture();
    let cur = fx.repo.current().unwrap();
    let found = evidence::lookup(&fx.repo, &cur).unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|(_, e)| e.is_none()));
    assert!(runs(&log).is_empty());
}

#[test]
fn input_paths_are_normalised_before_they_are_looked_up() {
    let fx = Fixture::new(&[
        ("zit.toml", "[[check]]\nname = \"ui\"\nrun = \"true\"\ninputs = [\"./ui/\"]\n"),
        ("ui/a", "1\n"),
    ]);
    evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap();
    let c = fx.change("agent", &[("ui/a", "2\n")]);
    assert!(!evidence::verify(&fx.repo, &c.id, false).unwrap()[0].cached, "a changed input must not reuse evidence");
}

#[test]
fn an_input_outside_the_state_is_refused() {
    let fx = Fixture::new(&[("zit.toml", "[[check]]\nname = \"x\"\nrun = \"true\"\ninputs = [\"../x\"]\n")]);
    assert!(evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).is_err());
}

/// Checks run in a view at a stable path that is updated in place, so
/// path-keyed build caches (cargo, tsc, …) survive from one state to the next.
#[test]
fn verification_reuses_one_view_keeping_ignored_build_output_and_nothing_else() {
    let scratch = tempfile::tempdir().unwrap().keep();
    let log = scratch.join("log");
    let run = format!(
        "pwd >> {log}; cat a.txt >> {log}; test -e junk.txt && echo LEAKED >> {log}; test -e target/cache && echo WARM >> {log}; \
         mkdir -p target && touch target/cache && echo junk > junk.txt && echo scribble >> a.txt",
        log = log.display()
    );
    let config = format!("[[check]]\nname = \"build\"\nrun = '''{run}'''\n");
    let fx = Fixture::new(&[("zit.toml", &config), (".gitignore", "target/\n"), ("a.txt", "one\n")]);
    evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap();
    let c = fx.change("agent", &[("a.txt", "two\n")]);
    evidence::verify(&fx.repo, &c.id, false).unwrap();

    let lines = runs(&log);
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert_eq!(lines[0], lines[2], "both states were checked at the same path");
    assert_eq!((lines[1].as_str(), lines[3].as_str()), ("one", "two"), "each run saw exactly its own state");
    assert_eq!(lines[4], "WARM", "ignored build output survived; untracked junk and edits did not");
    assert!(workspace::list(&fx.repo).unwrap().is_empty(), "the view is not a workspace anyone has to manage");
}

#[test]
fn concurrent_verifications_do_not_share_a_view() {
    let scratch = tempfile::tempdir().unwrap().keep();
    let log = scratch.join("log");
    let config = format!("[[check]]\nname = \"slow\"\nrun = \"pwd >> {}; sleep 1\"\n", log.display());
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "one\n")]);
    let a = fx.change("a", &[("a.txt", "two\n")]);
    let b = fx.change("b", &[("a.txt", "three\n")]);
    std::thread::scope(|s| {
        for c in [&a, &b] {
            let repo = fx.repo.clone();
            s.spawn(move || evidence::verify(&repo, &c.id, false).unwrap());
        }
    });
    let lines = runs(&log);
    assert_eq!(lines.len(), 2);
    assert_ne!(lines[0], lines[1], "two checks at once need two views");
}

#[test]
fn checks_get_a_shared_cache_directory() {
    let fx = Fixture::new(&[
        ("zit.toml", "[[check]]\nname = \"c\"\nrun = 'test -d \"$ZIT_CACHE_DIR\"'\n"),
        ("a.txt", "a\n"),
    ]);
    assert!(evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap()[0].evidence.passed);
}

#[test]
fn each_verification_starts_with_an_empty_private_temp_directory() {
    let scratch = tempfile::tempdir().unwrap().keep();
    let log = scratch.join("log");
    let run = format!("ls \"$TMPDIR\" | wc -l | tr -d ' ' >> {}; touch \"$TMPDIR/left-behind\"", log.display());
    let config = format!("[[check]]\nname = \"t\"\nrun = '''{run}'''\n");
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "one\n")]);
    evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap();
    let c = fx.change("agent", &[("a.txt", "two\n")]);
    evidence::verify(&fx.repo, &c.id, false).unwrap();
    assert_eq!(runs(&log), ["0", "0"]);
}

/// A hung check fails at its deadline instead of blocking accept forever; what it started is stopped too.
#[test]
fn a_check_that_hangs_fails_at_its_timeout() {
    let scratch = tempfile::tempdir().unwrap();
    let pidfile = scratch.path().join("child.pid");
    let config = format!(
        "[[check]]\nname = \"hang\"\nrun = \"sleep 60 & echo $! > {}; wait\"\ntimeout = 1\n",
        pidfile.display()
    );
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "a\n")]);
    let current = fx.repo.current().unwrap();
    let started = std::time::Instant::now();
    let verdicts = evidence::verify(&fx.repo, &current, false).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(15), "took {:?}", started.elapsed());
    assert!(!verdicts[0].evidence.passed);
    assert!(verdicts[0].evidence.output.contains("timed out after 1s"), "{}", verdicts[0].evidence.output);
    let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    // Dead means gone, or (on Linux, until init reaps the orphan) a zombie.
    let dead = || {
        (unsafe { libc::kill(pid, 0) }) != 0
            || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|s| s.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z')))
    };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !dead() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(dead(), "the check's child outlived it");
    let again = evidence::verify(&fx.repo, &current, false).unwrap();
    assert!(!again[0].cached, "a timed-out result was remembered as the state's verdict");
}

/// Two one-second checks: `[accept] jobs = 2` runs them side by side.
#[test]
fn checks_run_concurrently_up_to_jobs() {
    let two_sleeps = "[[check]]\nname = \"a\"\nrun = \"sleep 1\"\n\n[[check]]\nname = \"b\"\nrun = \"sleep 1\"\n";
    let timed = |config: &str| {
        let fx = Fixture::new(&[("zit.toml", config), ("a.txt", "a\n")]);
        let started = std::time::Instant::now();
        let verdicts = evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap();
        assert!(verdicts.iter().all(|v| v.evidence.passed && !v.cached));
        started.elapsed()
    };
    let sequential = timed(two_sleeps);
    let parallel = timed(&format!("[accept]\njobs = 2\n\n{two_sleeps}"));
    eprintln!("two 1s checks: sequential {sequential:?}, jobs = 2 {parallel:?}");
    assert!(sequential >= std::time::Duration::from_secs(2), "{sequential:?}");
    assert!(parallel < std::time::Duration::from_millis(1900), "{parallel:?}");
}

/// `serial = true` on a check keeps it alone: nothing else runs while it does.
#[test]
fn a_serial_check_never_runs_alongside_another() {
    let scratch = tempfile::tempdir().unwrap().keep();
    let log = scratch.join("log");
    let check = |name: &str, serial: bool| {
        format!(
            "[[check]]\nname = \"{name}\"\nrun = \"echo start {name} >> {log}; sleep 0.5; echo end {name} >> {log}\"\nserial = {serial}\n\n",
            log = log.display()
        )
    };
    let config = format!("[accept]\njobs = 3\n\n{}{}{}", check("a", false), check("s", true), check("b", false));
    let fx = Fixture::new(&[("zit.toml", &config), ("a.txt", "a\n")]);
    let verdicts = evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap();
    let names: Vec<&str> = verdicts.iter().map(|v| v.evidence.check.as_str()).collect();
    assert_eq!(names, ["a", "s", "b"], "verdicts keep the declared order");
    let lines = runs(&log);
    let at = |line: &str| lines.iter().position(|l| l == line).unwrap();
    assert_eq!(at("start s") + 1, at("end s"), "nothing ran while the serial check did: {lines:?}");
    assert!(at("end s") < at("start b") && at("end a") < at("start s"), "{lines:?}");
}

/// What a parallel verification stores is what a sequential one would look up.
#[test]
fn parallel_evidence_is_the_same_evidence() {
    let checks = "[[check]]\nname = \"one\"\nrun = \"cat a/x\"\ninputs = [\"a\"]\n\n[[check]]\nname = \"two\"\nrun = \"test -f a/missing\"\ninputs = [\"a\"]\n";
    let fx = Fixture::new(&[("zit.toml", &format!("[accept]\njobs = 2\n\n{checks}")), ("a/x", "from a\n")]);
    let parallel = evidence::verify(&fx.repo, &fx.repo.current().unwrap(), false).unwrap();
    assert!(parallel.iter().all(|v| !v.cached));

    // The same checks and inputs, sequential: every key is already known.
    let sequential_state = fx.change("agent", &[("zit.toml", checks)]);
    let sequential = evidence::verify(&fx.repo, &sequential_state.id, false).unwrap();
    assert!(sequential.iter().all(|v| v.cached), "{sequential:?}");
    let essentials = |vs: &[evidence::Verdict]| {
        vs.iter()
            .map(|v| (v.evidence.check.clone(), v.evidence.key.clone(), v.evidence.passed, v.evidence.exit_code))
            .collect::<Vec<_>>()
    };
    assert_eq!(essentials(&parallel), essentials(&sequential));
    assert_eq!(essentials(&parallel)[0].2, true);
    assert_eq!(essentials(&parallel)[1].2, false);
}

/// Nothing is materialised, workspace or verification view, when the volume
/// holding Zit's home has less than `zit.minFreeMB` free; 0 turns the guard off.
#[test]
fn materialising_is_refused_when_the_volume_is_nearly_full() {
    let fx = Fixture::new(&[("zit.toml", "[[check]]\nname = \"t\"\nrun = \"true\"\n"), ("a.txt", "a\n")]);
    let current = fx.repo.current().unwrap();
    let new = || zit::workspace::NewWorkspace { from: None, intent: "x", agent: "a", session: None };
    // No volume is this large: 999 TB.
    common::git(&fx.root(), &["config", "zit.minFreeMB", "999999999"]);
    let err = workspace::materialise(&fx.repo, &new()).unwrap_err().to_string();
    assert!(err.contains("zit.minFreeMB") && err.contains("MB free"), "{err}");
    assert!(workspace::list(&fx.repo).unwrap().is_empty(), "nothing half-made");
    let err = evidence::verify(&fx.repo, &current, false).unwrap_err().to_string();
    assert!(err.contains("zit.minFreeMB"), "{err}");

    common::git(&fx.root(), &["config", "zit.minFreeMB", "0"]);
    let ws = workspace::materialise(&fx.repo, &new()).unwrap();
    workspace::dispose(&fx.repo, &ws.id).unwrap();
    assert!(evidence::verify(&fx.repo, &current, false).unwrap()[0].evidence.passed);
}

/// A misplaced or misspelt key in zit.toml is an error, not silently ignored.
#[test]
fn an_unknown_key_in_zit_toml_is_an_error() {
    let config = "[[check]]\nname = \"t\"\nrun = \"true\"\nignore = [\".coverage\"]\n";
    let fx = Fixture::new(&[("zit.toml", config), ("a.txt", "a\n")]);
    let current = fx.repo.current().unwrap();
    let err = evidence::verify(&fx.repo, &current, false).unwrap_err().to_string();
    assert!(err.contains("unknown field") && err.contains("ignore"), "{err}");
}
