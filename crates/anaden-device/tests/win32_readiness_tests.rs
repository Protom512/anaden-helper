//! readiness probe の Win32 I/O プリミティブ検証 (Issue #275)。
//!
//! 実ゲームは使わない。`count_tcp_sockets` を自プロセスの実ソケット
//! (テスト内で開く TcpListener/TcpStream = deterministic な偽ゲームプロセス) で
//! 検証する。`probe_readiness` のサンプリングループ自体は実ウィンドウが必要なため
//! ここでは扱わない (判定コアは tests/readiness_tests.rs が純関数として固定)。

#![cfg(windows)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::net::{TcpListener, TcpStream};

use anaden_device::count_tcp_sockets;

/// 自プロセスの PID。
fn self_pid() -> u32 {
    std::process::id()
}

#[test]
fn count_tcp_sockets_sees_own_listener_socket() {
    // LISTEN ソケット 1 本を開く = 所有 PID の TCP 行が 1 つ増える。
    // netstat -ano 行数カウント相当の計上に LISTEN が含まれることの検証。
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let count = count_tcp_sockets(&[self_pid()]).expect("count tcp sockets");
    assert!(
        count >= 1,
        "own LISTEN socket must be counted (got {count})"
    );
    drop(listener);
}

#[test]
fn count_tcp_sockets_sees_own_established_pair() {
    // 接続確立で LISTEN + ESTABLISHED (両端とも自 PID) が観測できる。
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect to own listener");
    let server = listener.accept().expect("accept own connection");
    let count = count_tcp_sockets(&[self_pid()]).expect("count tcp sockets");
    assert!(
        count >= 3,
        "LISTEN + ESTABLISHED pair (both ends owned by self) must be counted (got {count})"
    );
    drop(client);
    drop(server);
    drop(listener);
}

#[test]
fn count_tcp_sockets_returns_zero_for_absent_pid() {
    // 存在しない PID には行が紐付かない (フィルタが機能していることの検証)。
    // u32::MAX は Windows の PID 空間 (SYSTEM_PROCESS_INFORMATION では
    // ULONG_MAX が Idle プロセス扱い) の外の実在しない PID を意図する。
    let bogus = u32::MAX - 1;
    let count = count_tcp_sockets(&[bogus]).expect("count tcp sockets for absent pid");
    assert_eq!(count, 0, "absent pid must own no TCP rows");
}

#[test]
fn count_tcp_sockets_empty_pid_list_is_zero() {
    // 空リスト = 対象なし。全行がフィルタで落ちる。
    let count = count_tcp_sockets(&[]).expect("count tcp sockets for empty pids");
    assert_eq!(count, 0);
}
