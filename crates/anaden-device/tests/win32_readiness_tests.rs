#![cfg(windows)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::net::{TcpListener, TcpStream};

use anaden_device::count_tcp_sockets;

fn self_pid() -> u32 {
    std::process::id()
}

#[test]
fn count_tcp_sockets_sees_own_listener_socket() {
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
    let bogus = u32::MAX - 1;
    let count = count_tcp_sockets(&[bogus]).expect("count tcp sockets for absent pid");
    assert_eq!(count, 0, "absent pid must own no TCP rows");
}

#[test]
fn count_tcp_sockets_empty_pid_list_is_zero() {
    let count = count_tcp_sockets(&[]).expect("count tcp sockets for empty pids");
    assert_eq!(count, 0);
}
