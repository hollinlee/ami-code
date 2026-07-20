mod support;

use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use support::pty::PtyHarness;

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
    app.expect_screen("right-click Paste menu", |screen| screen.contains("Paste"));

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

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    false
}
