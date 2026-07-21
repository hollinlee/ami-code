#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use support::pty::{PtyHarness, PtyHarnessOptions};

#[test]
fn viewport_matrix_starts_and_quits_without_panics() {
    for (cols, rows) in [(160, 50), (120, 40), (100, 30), (80, 24)] {
        let mut app = PtyHarness::spawn(&[], cols, rows);
        app.expect_screen(&format!("workbench {cols}x{rows}"), |screen| {
            screen.contains("nvim") && screen.contains("pi") && screen.contains("shell")
        });
        app.send(b"\x11");
        assert_eq!(app.wait_for_exit().exit_code(), 0, "viewport {cols}x{rows}");
    }

    let mut compact = PtyHarness::spawn(&[], 5, 2);
    compact.expect_screen("compact fallback", |screen| screen.contains("workb"));
    compact.send(b"\x11");
    assert_eq!(compact.wait_for_exit().exit_code(), 0);
}

#[test]
fn responsive_breakpoints_hide_secondary_panes_before_backends() {
    let mut app = PtyHarness::spawn(&[], 120, 40);
    app.expect_screen("full workbench", |screen| {
        screen.contains("sidebar") && screen.contains("shell")
    });

    app.resize(100, 30);
    app.expect_screen("sidebar collapsed first", |screen| {
        !screen.contains("sidebar")
            && screen.contains("nvim")
            && screen.contains("pi")
            && screen.contains("shell")
    });

    app.resize(120, 23);
    app.expect_screen("shell collapsed first", |screen| {
        screen.contains("sidebar")
            && screen.contains("nvim")
            && screen.contains("pi")
            && !screen.contains("shell")
    });

    app.resize(120, 40);
    app.expect_screen("secondary panes restored", |screen| {
        screen.contains("sidebar") && screen.contains("shell")
    });
    app.send(b"\x11");
    assert_eq!(app.wait_for_exit().exit_code(), 0);
}

#[test]
fn workbench_recovers_after_compact_resize() {
    let mut app = PtyHarness::spawn(&[], 120, 40);
    app.expect_screen("initial workbench", |screen| {
        screen.contains("nvim") && screen.contains("pi") && screen.contains("shell")
    });

    app.resize(5, 2);
    app.expect_screen("resized compact fallback", |screen| {
        screen.contains("workb")
    });

    app.resize(160, 50);
    app.expect_screen("restored workbench", |screen| {
        screen.contains("sidebar")
            && screen.contains("nvim")
            && screen.contains("pi")
            && screen.contains("shell")
    });
    app.send(b"\x11");
    assert_eq!(app.wait_for_exit().exit_code(), 0);
}

#[test]
fn right_click_menu_is_frontend_owned_and_dismissible() {
    let mut app = PtyHarness::spawn(&[], 120, 40);
    app.expect_screen("workbench ready for context menu", |screen| {
        screen.contains("nvim") && screen.contains("pi")
    });

    // SGR mouse coordinates are one-based. This is inside Nvim content.
    app.send(b"\x1b[<2;30;5M");
    app.expect_screen("fixed right-click menu", |screen| {
        screen.contains("Copy") && screen.contains("Paste")
    });

    // Clicking outside the menu closes it and is consumed by frontend chrome.
    app.send(b"\x1b[<0;2;2M\x1b[<0;2;2m");
    app.expect_screen("context menu dismissed", |screen| !screen.contains("Paste"));

    // Open over Shell, then ensure an ordinary key dismisses the menu without
    // leaking into the focused backend. The following Enter is forwarded.
    app.send(b"\x1b[<2;30;35M");
    app.expect_screen("Shell context menu", |screen| screen.contains("Paste"));
    app.send(b"x");
    app.expect_screen("key-dismissed context menu", |screen| {
        !screen.contains("Paste")
    });
    app.send(b"\r");
    let input_path = app.child_input_path();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !input_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(input_path).unwrap(), "<>\n");
    app.send(b"\x11");
    assert_eq!(app.wait_for_exit().exit_code(), 0);
}

#[test]
fn native_pi_contract_is_shared_by_workbench_and_standalone() {
    let mut workbench = PtyHarness::spawn(&[], 120, 40);
    workbench.expect_screen("native Pi fixture in workbench", |screen| {
        screen.contains("fixture pi ready")
    });
    let args = wait_for_file(&workbench.pi_args_path());
    assert_native_pi_args(&args, &workbench.state_path());
    assert_eq!(wait_for_file(&workbench.pi_env_path()), "unset\n");
    workbench.send(b"\x11");
    assert_eq!(workbench.wait_for_exit().exit_code(), 0);

    let explicit_agent_dir = PathBuf::from("/tmp/ami-code-native-pi-profile");
    let mut standalone = PtyHarness::spawn_with_options(
        &["pi"],
        80,
        24,
        PtyHarnessOptions {
            pi_coding_agent_dir: Some(explicit_agent_dir.clone()),
            pi_exit_immediately: false,
            nvim_fixture: false,
        },
    );
    standalone.expect_screen("native Pi fixture standalone", |screen| {
        screen.contains("fixture pi ready")
    });
    assert_native_pi_args(
        &wait_for_file(&standalone.pi_args_path()),
        &standalone.state_path(),
    );
    assert_eq!(
        wait_for_file(&standalone.pi_env_path()),
        format!("set={}\n", explicit_agent_dir.display())
    );
    let pid = wait_for_pid(&standalone.pi_pids_path());
    standalone.send(b"\x11");
    assert_eq!(standalone.wait_for_exit().exit_code(), 0);
    wait_for_process_exit("standalone Pi", pid);
}

#[test]
fn native_pi_crash_loop_stays_pane_local_and_reaps_generations() {
    let mut app = PtyHarness::spawn_with_options(
        &[],
        120,
        40,
        PtyHarnessOptions {
            pi_coding_agent_dir: None,
            pi_exit_immediately: true,
            nvim_fixture: true,
        },
    );
    let pids_path = app.pi_pids_path();
    let deadline = Instant::now() + Duration::from_secs(15);
    let observed_pids = loop {
        let pids = fs::read_to_string(&pids_path).unwrap_or_default();
        if pids.lines().count() >= 2 {
            break parse_pids(&pids);
        }
        assert!(Instant::now() < deadline, "Pi did not restart: {pids:?}");
        thread::sleep(Duration::from_millis(25));
    };
    for pid in observed_pids {
        wait_for_process_exit("exited Pi generation", pid);
    }

    let nvim_pid = wait_for_pid(&app.nvim_pids_path());
    let shell_pid = wait_for_pid(&app.child_pid_path());
    assert!(process_exists(nvim_pid), "Nvim exited after Pi crash");
    assert!(process_exists(shell_pid), "Shell exited after Pi crash");
    app.expect_screen("Pi crash remains pane local", |screen| {
        screen.contains("sidebar") && screen.contains("nvim") && screen.contains("shell")
    });
    assert!(process_exists(nvim_pid), "Nvim did not remain alive");
    assert!(process_exists(shell_pid), "Shell did not remain alive");

    app.send(b"\x11");
    assert_eq!(app.wait_for_exit().exit_code(), 0);
    for pid in parse_pids(&fs::read_to_string(&pids_path).unwrap()) {
        wait_for_process_exit("Pi generation", pid);
    }
    wait_for_process_exit("Nvim", nvim_pid);
    wait_for_process_exit("Shell", shell_pid);
}

#[test]
fn clean_quit_restores_terminal_and_reaps_backend() {
    let mut app = PtyHarness::spawn(&["shell"], 80, 24);
    app.expect_screen("standalone shell", |screen| screen.contains("shell"));
    let pid_path = app.child_pid_path();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pid_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let backend_pid = fs::read_to_string(&pid_path)
        .expect("fixture shell wrote its pid")
        .trim()
        .parse::<u32>()
        .expect("fixture shell pid is numeric");

    app.send(b"\x11");
    assert_eq!(app.wait_for_exit().exit_code(), 0);

    let raw = app.raw();
    let enter_alt = rfind(raw, b"\x1b[?1049h").expect("entered alternate screen");
    let leave_alt = rfind(raw, b"\x1b[?1049l").expect("left alternate screen");
    let enable_paste = rfind(raw, b"\x1b[?2004h").expect("enabled bracketed paste");
    let disable_paste = rfind(raw, b"\x1b[?2004l").expect("disabled bracketed paste");
    assert!(enter_alt < leave_alt);
    assert!(enable_paste < disable_paste);

    let deadline = Instant::now() + Duration::from_secs(2);
    while process_exists(backend_pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_exists(backend_pid),
        "backend {backend_pid} survived app exit"
    );
}

fn assert_native_pi_args(args: &str, state: &Path) {
    let lines = args.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 4, "unexpected Pi argv log: {args:?}");
    assert_eq!(lines[0], "start");
    assert_eq!(lines[1], "arg=--session-dir");
    let session_dir = lines[2].strip_prefix("arg=").unwrap();
    assert!(
        Path::new(session_dir).starts_with(state),
        "session escaped app state: {session_dir}"
    );
    assert!(session_dir.contains("/pi/managed-v1/sessions/"));
    assert!(Path::new(session_dir).is_dir());
    assert_eq!(lines[3], "arg=--no-approve");
    for forbidden in [
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
    ] {
        assert!(!args.contains(forbidden));
    }
}

fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match fs::read_to_string(path) {
            Ok(contents) if !contents.is_empty() => return contents,
            Ok(_) | Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            result => panic!("file {} was not populated: {result:?}", path.display()),
        }
    }
}

fn wait_for_pid(path: &Path) -> u32 {
    parse_pids(&wait_for_file(path))[0]
}

fn parse_pids(contents: &str) -> Vec<u32> {
    contents
        .lines()
        .map(|line| line.parse::<u32>().unwrap())
        .collect()
}

fn wait_for_process_exit(label: &str, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_exists(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_exists(pid),
        "{label} backend {pid} survived app exit: {}",
        process_description(pid)
    );
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn process_description(pid: u32) -> String {
    Command::new("/bin/ps")
        .args(["-o", "pid=,ppid=,state=,command=", "-p", &pid.to_string()])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|error| format!("ps failed: {error}"))
}

fn process_exists(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
