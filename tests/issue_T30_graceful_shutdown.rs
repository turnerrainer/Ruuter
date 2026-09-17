//! h2ck.me v1 T-30 — Graceful shutdown on SIGTERM / SIGINT.
//!
//! Pre-fix, `src/main.rs` awaited `axum::serve(...)` (and each
//! multi-listener spawned task) without a shutdown-signal hook.
//! On Kubernetes rolling deploys (`SIGTERM` at pod terminate) or
//! `docker stop` (SIGTERM after a grace window), the process
//! either kept accepting for up to `terminationGracePeriodSeconds`
//! and then got SIGKILL'd mid-response, or — worse — returned a
//! torn HTTP response as tokio dropped tasks partway through a
//! DSL run.
//!
//! Post-fix: a single `tokio::sync::watch` shutdown signal is
//! flipped when SIGINT or SIGTERM arrives. Each `axum::serve`
//! consumes it via `with_graceful_shutdown` (which stops accepting
//! and waits for in-flight requests). The UDS accept loop selects
//! on the same signal, breaks out on shutdown, and drains a
//! `JoinSet` of in-flight per-connection tasks with a bounded
//! grace of `SHUTDOWN_GRACE_SECS` (15s today).
//!
//! Test written to try to BREAK the fix:
//! - Boot a ruuter subprocess.
//! - Fire an inbound request that itself sits in a slow upstream
//!   call (mock server sleeps 5s).
//! - After ~1s (well into the upstream wait), send SIGTERM.
//! - Assert:
//!   a. The in-flight request completes cleanly with the mocked
//!      upstream body (not a torn or aborted response).
//!   b. The ruuter process exits within the grace window (well
//!      under the SIGKILL that k8s would issue at 30s).
//!
//! The test uses `libc::kill(pid, SIGTERM)` via the `kill` shell
//! command so we don't need a new crate dependency. See
//! `send_sigterm` at the bottom of this file.

#![allow(clippy::field_reassign_with_default)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn ruuter_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ruuter-on-rust"))
}

fn free_port() -> u16 {
    // Bind-and-drop; small race window, acceptable for a test.
    let l = TcpListener::bind("127.0.0.1:0").expect("bind free port probe");
    let p = l.local_addr().expect("addr").port();
    drop(l);
    p
}

/// Spawn a stdlib mock upstream on `port` that sleeps `delay`
/// between the request line drain and the canned 200 body write.
/// Emits nothing over stderr; the parent thread joins via `stop_rx`.
fn spawn_slow_upstream(port: u16, delay: Duration) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind upstream");
        listener.set_nonblocking(false).ok();
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // Drain the request headers until "\r\n\r\n" so the
            // client sees a fully-parsed HTTP round-trip rather than
            // a mid-header disconnect.
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let _ = String::from_utf8_lossy(&buf[..n]);
            thread::sleep(delay);
            let body = b"{\"upstream_ok\":true}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(response.as_bytes());
            let _ = s.write_all(body);
            let _ = s.flush();
            // One-shot; the test doesn't need a persistent server.
            break;
        }
    })
}

fn wait_for_ruuter_ready(port: u16) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", port).parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn send_sigterm(pid: u32) {
    // Use /bin/kill so we don't need a new crate for a signal
    // wrapper. `Child::kill()` sends SIGKILL, which would defeat
    // the entire test — we specifically need SIGTERM.
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("spawn kill -TERM");
    assert!(status.success(), "kill -TERM {} failed", pid);
}

/// Regression pin for T-30.
///
/// Scenario:
/// - Bind two free ports, `p_ruuter` and `p_upstream`.
/// - Spawn a slow-upstream that will sleep 5s on the first
///   connection then reply with `{"upstream_ok":true}`.
/// - Write a temp DSL directory containing one route (`GET
///   /svc/slow`) that calls `http://127.0.0.1:<p_upstream>/`.
/// - Spawn ruuter-on-rust binding to `127.0.0.1:<p_ruuter>`.
/// - Wait for ruuter to accept.
/// - Send `GET /svc/slow` on a background thread.
/// - Sleep ~1.5s so the request is deep in the upstream wait.
/// - Send SIGTERM to ruuter.
/// - Await the response body — must be 200 with the mocked shape.
/// - Await ruuter exit — must terminate within the grace window.
#[test]
fn sigterm_drains_inflight_request_and_exits_within_grace() {
    let p_ruuter = free_port();
    let p_upstream = free_port();
    assert_ne!(p_ruuter, p_upstream, "free_port collision — retry the test");

    let _upstream = spawn_slow_upstream(p_upstream, Duration::from_secs(5));

    // Temp workspace layout:
    //   $tmp/DSL/svc/GET/slow.yml   (route)
    //   $tmp/ruuter.yaml            (config)
    let tmp = std::env::temp_dir().join(format!(
        "ruuter-t30-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dsl_dir = tmp.join("DSL");
    std::fs::create_dir_all(dsl_dir.join("svc/GET")).unwrap();
    std::fs::write(
        dsl_dir.join("svc/GET/slow.yml"),
        format!(
            r#"
fetch:
  call: http.get
  args:
    url: "http://127.0.0.1:{p}/"
  result: upstream
  next: reply
reply:
  return:
    ok: true
    upstream_ok: ${{upstream.response.body.upstream_ok}}
  next: end
"#,
            p = p_upstream
        ),
    )
    .unwrap();

    let cfg_path = tmp.join("ruuter.yaml");
    // `block_private_networks: false` is required only because the
    // slow-upstream mock binds to 127.0.0.1 — SSRF policy correctly
    // refuses private-net outbound in prod. The block_private_networks
    // WARN and CSRF WARN etc. still fire at boot; the test doesn't
    // interpret them.
    std::fs::write(
        &cfg_path,
        format!(
            r#"
port: {p_ruuter}
config_path: "{dsl}"
scripting:
  engine: none
internal_requests:
  block_private_networks: false
dsl:
  warn_on_missing_declaration: false
"#,
            p_ruuter = p_ruuter,
            dsl = dsl_dir.display()
        ),
    )
    .unwrap();

    // Spawn ruuter. Wrap in a `Reap` guard so any early panic below
    // still SIGKILLs + wait()s the child and we don't leak a
    // background process. `child.wait()` is called in every exit
    // path — the guard on panic, and the polling loop's terminal
    // arm on success — satisfying `clippy::zombie_processes`.
    struct Reap(std::process::Child);
    impl Reap {
        fn as_mut(&mut self) -> &mut std::process::Child {
            &mut self.0
        }
        fn id(&self) -> u32 {
            self.0.id()
        }
    }
    impl Drop for Reap {
        fn drop(&mut self) {
            // If the child hasn't exited yet, kill it. Ignoring the
            // Result is fine — an already-exited child returns Err
            // on kill, which is exactly the state we accept.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let child_raw = Command::new(ruuter_path())
        .arg("--config")
        .arg(&cfg_path)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ruuter");
    let mut child = Reap(child_raw);
    let pid = child.id();

    assert!(
        wait_for_ruuter_ready(p_ruuter),
        "ruuter did not open the listen socket on 127.0.0.1:{} within 30s",
        p_ruuter
    );

    // Fire the inbound request on a background thread; keep the
    // response future alive so we can inspect it after SIGTERM.
    let (tx, rx) = mpsc::channel::<(u16, String)>();
    thread::spawn(move || {
        let resp = ureq_get(&format!("http://127.0.0.1:{}/svc/slow", p_ruuter));
        let _ = tx.send(resp);
    });

    // Give the request time to reach the upstream and begin its
    // 5s wait. 1.5s is well past the ~50ms handshake and well
    // before the 5s upstream reply.
    thread::sleep(Duration::from_millis(1500));

    let sigterm_at = Instant::now();
    send_sigterm(pid);

    // Wait for the in-flight response — the drain window is 15s
    // in ruuter today. Add a small buffer to the wait deadline.
    let (status, body) = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("in-flight response must complete within grace window");

    assert_eq!(
        status, 200,
        "in-flight request must complete cleanly with 200; got status {}, body {:?}",
        status, body
    );
    // Body arrives as `{"response":{"ok":true,"upstream_ok":true},...}` —
    // we only assert the upstream_ok round-trip made it back to the
    // caller, which is the "no torn response" signal.
    assert!(
        body.contains("upstream_ok"),
        "response body must carry the mocked upstream payload; got: {}",
        body
    );

    // Now the process should exit. Poll with `try_wait` so the test
    // fails cleanly rather than hanging past the grace window.
    let exit_deadline = Instant::now() + Duration::from_secs(20);
    let mut exit_status = None;
    while Instant::now() < exit_deadline {
        match child.as_mut().try_wait() {
            Ok(Some(s)) => {
                exit_status = Some(s);
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("try_wait error: {}", e),
        }
    }
    let status = exit_status.unwrap_or_else(|| {
        panic!(
            "ruuter did not exit within 20s of SIGTERM (grace window is {}s)",
            15
        )
    });
    let elapsed = sigterm_at.elapsed();
    assert!(
        status.success() || status.code() == Some(0),
        "ruuter must exit cleanly on SIGTERM; got {:?}",
        status
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "ruuter took too long to exit after SIGTERM: {:?}",
        elapsed
    );
}

// Minimal HTTP GET without adding a new crate — just enough to
// read the status line and body for a fully-arrived response.
fn ureq_get(url: &str) -> (u16, String) {
    let parsed = url::Url::parse(url).expect("valid url");
    let host = parsed.host_str().expect("host").to_string();
    let port = parsed.port_or_known_default().unwrap_or(80);
    let path = parsed.path().to_string();
    let path = if path.is_empty() { "/".into() } else { path };
    let mut s = std::net::TcpStream::connect((host.as_str(), port))
        .expect("connect ruuter for inbound request");
    // Give the socket plenty of time — the test's outer wait
    // recv_timeout is the real deadline.
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        path, host, port
    );
    s.write_all(req.as_bytes()).expect("send request");
    s.flush().ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    // Status line: "HTTP/1.1 200 OK\r\n..."
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}
