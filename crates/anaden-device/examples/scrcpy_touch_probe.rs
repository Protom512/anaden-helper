//! scrcpy control-touch 注入の実機検証プローブ。
//!
//! 目的: scrcpy-server を `control=true` で起動し、control ソケットへ
//! `TYPE_INJECT_TOUCH_EVENT`(ACTION_DOWN → ACTION_UP)を送って、
//! Another Eden が `adb shell input tap` とは別経路のタッチ注入を
//! 受け付けるかを**画面の MD5/差分**で決定的に判定する。
//!
//! 仕様は公式ソース(v4.0)を実読して確定:
//!   - `app/tests/test_control_msg_serialize.c::test_serialize_inject_touch_event`
//!   - `server/.../control/ControlMessage.java`
//!
//! TYPE_INJECT_TOUCH_EVENT ワイヤーフォーマット(32 byte):
//!   [0]     type        = 0x02
//!   [1]     action      = DOWN(0x00) / UP(0x01) / MOVE(0x02)
//!   [2..9]  pointer id  (uint64 BE)
//!   [10..13] position.x (uint32 BE)
//!   [14..17] position.y (uint32 BE)
//!   [18..19] screen w   (uint16 BE)
//!   [20..21] screen h   (uint16 BE)
//!   [22..23] pressure   (uint16 BE, 0xffff = 1.0f)
//!   [24..27] action button (uint32 BE, 0x01 = PRIMARY)
//!   [28..31] buttons       (uint32 BE, 0x01 = PRIMARY)
//!
//! **重要**: docs/scrcpy-protocol.md §6 は「Position uint16x2 BE」と書いているが
//! **誤り**。実プロトコルは Point(uint32 x, uint32 y) + screen_size(uint16 w, uint16 h)。
//! 本プローブは上記テスト expected 配列とバイト完全一致する実装。
//!
//! ## 使い方
//! ```bash
//! cargo run --release -p anaden-device --example scrcpy_touch_probe -- \
//!   --serial 33291JEHN27041 --x 970 --y 170 --screen-w 1080 --screen-h 2400
//! ```

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const REMOTE_JAR: &str = "/data/local/tmp/scrcpy-server.jar";
/// 既存 ScrcpyCapture と同じ scid(動作実証済み)。
const SCID: &str = "18310000";
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const CONNECT_RETRY_MAX: u32 = 5;

/// TYPE_INJECT_TOUCH_EVENT = 2 (ControlMessage.java)
const TYPE_INJECT_TOUCH_EVENT: u8 = 2;
/// MotionEvent.ACTION_DOWN = 0, ACTION_UP = 1, ACTION_MOVE = 2
const ACTION_DOWN: u8 = 0;
const ACTION_UP: u8 = 1;
/// AMOTION_EVENT_BUTTON_PRIMARY = 0x00000001
const BUTTON_PRIMARY: u32 = 0x0000_0001;
/// pressure = 1.0f → 0xffff
const PRESSURE_MAX: u16 = 0xffff;

#[derive(Debug)]
struct Args {
    serial: String,
    local_jar: String,
    x: u32,
    y: u32,
    screen_w: u32,
    screen_h: u32,
    /// touch DOWN→UP 間のホールド時間
    hold_ms: u64,
    /// 送信後に効果が出るのを待つ時間
    settle_ms: u64,
    /// 外部でサーバを起動済みの forward ポート(0=自分でサーバ起動)。
    /// このポートへ直接接続する(video→control の2本)。
    connect_port: Option<u16>,
}

fn parse_args() -> Result<Args, String> {
    let mut serial =
        std::env::var("ANADEN_SERIAL").unwrap_or_else(|_| "33291JEHN27041".to_string());
    let mut local_jar = r"C:\Users\black\scoop\apps\scrcpy\current\scrcpy-server".to_string();
    let mut x: u32 = 970;
    let mut y: u32 = 170;
    let mut screen_w: u32 = 1080;
    let mut screen_h: u32 = 2400;
    let mut hold_ms: u64 = 60;
    let mut settle_ms: u64 = 1200;
    let mut connect_port: Option<u16> = None;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--serial" => {
                serial = args.get(i + 1).cloned().ok_or("--serial needs value")?;
                i += 2;
            }
            "--jar" => {
                local_jar = args.get(i + 1).cloned().ok_or("--jar needs value")?;
                i += 2;
            }
            "--x" => {
                x = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--x needs number")?;
                i += 2;
            }
            "--y" => {
                y = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--y needs number")?;
                i += 2;
            }
            "--screen-w" => {
                screen_w = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--screen-w needs number")?;
                i += 2;
            }
            "--screen-h" => {
                screen_h = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--screen-h needs number")?;
                i += 2;
            }
            "--hold-ms" => {
                hold_ms = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--hold-ms needs number")?;
                i += 2;
            }
            "--settle-ms" => {
                settle_ms = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .ok_or("--settle-ms needs number")?;
                i += 2;
            }
            "--connect-port" => {
                connect_port = Some(
                    args.get(i + 1)
                        .and_then(|v| v.parse().ok())
                        .ok_or("--connect-port needs number")?,
                );
                i += 2;
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    Ok(Args {
        serial,
        local_jar,
        x,
        y,
        screen_w,
        screen_h,
        hold_ms,
        settle_ms,
        connect_port,
    })
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("arg error: {e}");
            std::process::exit(2);
        }
    };
    println!("=== scrcpy control-touch probe ===");
    println!("serial={} jar={}", args.serial, args.local_jar);
    println!(
        "target=({}, {}) screen={}x{}",
        args.x, args.y, args.screen_w, args.screen_h
    );

    let local_addr;

    if let Some(port) = args.connect_port {
        // 外部サーバモード: サーバ起動・forward は外部(bash 等)で済み。
        // 指定ポートへ直接接続する。
        local_addr = format!("127.0.0.1:{port}");
        println!("--connect-port={port}: 外部サーバへ接続(サーバ起動スキップ)");
    } else {
        // (1) jar push
        push_jar(&args);

        // (2) forward 設定(サーバ起動後に行う)
        let localabstract = format!("scrcpy_{SCID}");
        let local_port = pick_free_port().expect("free port");
        local_addr = format!("127.0.0.1:{local_port}");

        // (3) サーバ起動。control=true で video→control の2ソケット。
        let server_cmd = build_server_command();
        let (_child, _server_stderr) = spawn_server(&args.serial, &server_cmd);

        // (3b) forward 設定(サーバ起動後)。
        let _ = Command::new("adb")
            .args([
                "-s",
                &args.serial,
                "forward",
                &format!("tcp:{local_port}"),
                &format!("localabstract:{localabstract}"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        println!("forward tcp:{local_port} -> localabstract:{localabstract}");
    }

    // (4) 接続前にサーバが listen していることを /proc/net/unix で確認してから接続する。
    wait_for_listen(&args.serial, SCID);

    // プロトコル(DdesktopConnection.open 実読):
    //   1. video 接続 → dummy byte 受信
    //   2. control 接続(dummy byte 無し)
    //   3. video から device meta(64) + codec id(4) を読む(両 accept 後に送られる)
    println!("connecting video socket...");
    let mut video_sock = connect_and_handshake(&local_addr).expect("video socket");
    println!("connecting control socket...");
    let mut control_sock = connect_control(&local_addr).expect("control socket");
    println!("reading video device meta...");
    read_video_meta(&mut video_sock).expect("video meta");
    println!("both sockets established");

    // connect-port モードでは touch だけ送って終了(サーバは外部管理)。
    if args.connect_port.is_some() {
        let md5_before = screencap_md5(&args.serial);
        println!("MD5 before touch : {md5_before}");
        send_touch_down(&mut control_sock, &args);
        std::thread::sleep(Duration::from_millis(args.hold_ms));
        send_touch_up(&mut control_sock, &args);
        println!(
            "sent DOWN@({},{})->hold {}ms->UP",
            args.x, args.y, args.hold_ms
        );
        std::thread::sleep(Duration::from_millis(args.settle_ms));
        let md5_after = screencap_md5(&args.serial);
        println!("MD5 after  touch : {md5_after}");
        if md5_before != md5_after {
            println!("\n>>> RESULT: SCREEN CHANGED (scrcpy touch WORKS)");
        } else {
            println!("\n>>> RESULT: NO CHANGE (scrcpy touch IGNORED)");
        }
        return;
    }

    // (5) ターゲット座標へ touch DOWN/UP 注入。前後で screencap MD5 を比較。
    let md5_before = screencap_md5(&args.serial);
    println!("MD5 before touch : {md5_before}");

    // control メッセージ送信
    send_touch_down(&mut control_sock, &args);
    std::thread::sleep(Duration::from_millis(args.hold_ms));
    send_touch_up(&mut control_sock, &args);
    println!(
        "sent DOWN@({},{})->hold {}ms->UP",
        args.x, args.y, args.hold_ms
    );

    // 効果が出るのを待つ
    std::thread::sleep(Duration::from_millis(args.settle_ms));

    let md5_after = screencap_md5(&args.serial);
    println!("MD5 after  touch : {md5_after}");

    if md5_before != md5_after {
        println!("\n>>> RESULT: SCREEN CHANGED (scrcpy touch injection WORKS)");
    } else {
        println!("\n>>> RESULT: NO CHANGE (scrcpy touch injection IGNORED)");
    }

    // (6) 対照実験: adb shell input tap 同座標(既知:効かない)
    println!("\n--- control: adb shell input tap (known to be ignored) ---");
    let md5_pre_input = screencap_md5(&args.serial);
    let _ = Command::new("adb")
        .args([
            "-s",
            &args.serial,
            "shell",
            &format!("input tap {} {}", args.x, args.y),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    std::thread::sleep(Duration::from_millis(args.settle_ms));
    let md5_post_input = screencap_md5(&args.serial);
    println!("MD5 before input : {md5_pre_input}");
    println!("MD5 after  input : {md5_post_input}");
    if md5_pre_input != md5_post_input {
        println!(">>> input tap CHANGED screen (unexpected!)");
    } else {
        println!(">>> input tap NO CHANGE (as expected)");
    }

    // クリーンアップ。connect-port モードでは forward は外部管理なので解除しない。
    if args.connect_port.is_none() {
        let localabstract = format!("scrcpy_{SCID}");
        let _ = Command::new("adb")
            .args([
                "-s",
                &args.serial,
                "forward",
                "--remove",
                &format!("localabstract:{localabstract}"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // サーバ掃除
        let _ = Command::new("adb")
            .args([
                "-s",
                &args.serial,
                "shell",
                "pkill -f com.genymobile.scrcpy.Server 2>/dev/null; true",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    // video/control sock を明示 drop
    let _ = video_sock.read(&mut [0u8; 0]);
    drop(video_sock);
    drop(control_sock);
}

fn push_jar(args: &Args) {
    if !std::path::Path::new(&args.local_jar).exists() {
        eprintln!("jar not found: {}", args.local_jar);
        std::process::exit(1);
    }
    let out = Command::new("adb")
        .args(["-s", &args.serial, "push", &args.local_jar, REMOTE_JAR])
        .output()
        .expect("adb push");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    println!(
        "adb push exit={} stdout={} stderr={}",
        out.status.code().unwrap_or(-1),
        stdout.trim(),
        stderr.trim()
    );
    assert!(out.status.success(), "adb push failed: {stderr}");
    println!("jar pushed -> {REMOTE_JAR}");
    // 配置確認
    let chk = Command::new("adb")
        .args(["-s", &args.serial, "shell", &format!("ls -la {REMOTE_JAR}")])
        .output()
        .expect("adb shell ls");
    println!("verify: {}", String::from_utf8_lossy(&chk.stdout).trim());
}

fn build_server_command() -> Vec<String> {
    vec![
        format!("CLASSPATH={REMOTE_JAR}"),
        "app_process".into(),
        "/".into(),
        "com.genymobile.scrcpy.Server".into(),
        "4.0".into(),
        format!("scid={SCID}"),
        "log_level=debug".into(),
        "video=true".into(),
        "audio=false".into(),
        "control=true".into(), // ★ control 有効
        "video_codec=h264".into(),
        "video_bit_rate=8000000".into(),
        "max_size=0".into(),
        "max_fps=0".into(),
        "tunnel_forward=true".into(),
        "send_device_meta=true".into(),
        "send_frame_meta=true".into(),
        "send_stream_meta=true".into(),
        "send_dummy_byte=true".into(),
    ]
}

fn spawn_server(serial: &str, args: &[String]) -> (Child, std::fs::File) {
    let shell_cmd = args.join(" ");
    println!("server cmd: adb -s {serial} shell \"{shell_cmd}\"");
    // stderr をファイルへ落として、サーバが例外で即死した場合に原因を掴めるようにする。
    let stderr_path = std::env::temp_dir().join("scrcpy_probe_server_stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr log");
    let child = Command::new("adb")
        .args(["-s", serial, "shell", &shell_cmd])
        .stdout(Stdio::from(stderr_file.try_clone().expect("clone stderr")))
        .stderr(Stdio::from(stderr_file.try_clone().expect("clone stderr2")))
        .spawn()
        .expect("spawn server");
    println!(
        "scrcpy server spawned (adb child pid={}, stderr={})",
        child.id(),
        stderr_path.display()
    );
    // サーバが LocalServerSocket を open するまで少し待つ(約300ms)。
    std::thread::sleep(Duration::from_millis(400));
    // プロセス生存確認
    let alive = Command::new("adb")
        .args([
            serial,
            "shell",
            "pgrep -f com.genymobile.scrcpy.Server || echo NONE",
        ])
        .output();
    if let Ok(o) = alive {
        println!(
            "server proc alive check: {}",
            String::from_utf8_lossy(&o.stdout).trim()
        );
    }
    (child, stderr_file)
}

/// video ソケット接続 + dummy byte 受信のみ。
///
/// **プロトコル(DesktopConnection.open 実読):**
///   - tunnel_forward=true でサーバは LocalServerSocket を開き、video → (audio) → control
///     の順に accept する。
///   - `sendDummyByte` は **video ソケットでのみ**送信される(1回のみフラグ)。
///     control ソケットは dummy byte を送らない。
///   - device name(64) + codec id(4) は DesktopConnection.open 完了後(両ソケットの
///     accept 後)に sendDeviceMeta() で video ソケットから送られる。
///
/// したがって正しいシーケンスは:
///   1. video 接続 → dummy byte(1) 受信
///   2. control 接続(dummy byte 無し)
///   3. video から device name(64) + codec id(4) を読む
///
/// 本関数はステップ1のみ(dummy byte 受信)。device meta は両ソケット接続後に読む。
fn connect_and_handshake(local_addr: &str) -> Result<TcpStream, String> {
    let addr: std::net::SocketAddr = local_addr
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let mut last = String::new();
    for attempt in 0..CONNECT_RETRY_MAX {
        let addr2 = addr;
        let (tx, rx) = std::sync::mpsc::channel::<Result<TcpStream, String>>();
        std::thread::spawn(move || {
            let inner = (|| -> Result<TcpStream, String> {
                let mut s = TcpStream::connect(addr2).map_err(|e| format!("connect: {e}"))?;
                let _ = s.set_nodelay(true);
                let mut b = [0u8; 1];
                s.read_exact(&mut b)
                    .map_err(|e| format!("dummy read: {e}"))?;
                eprintln!("[video] dummy byte received: {:#x}", b[0]);
                Ok(s)
            })();
            let _ = tx.send(inner);
        });
        match recv_timeout(&rx, Duration::from_secs(15)) {
            Some(Ok(s)) => return Ok(s),
            Some(Err(e)) => {
                last = format!("attempt {}: {e}", attempt + 1);
                eprintln!("[video] {last}");
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
            None => {
                last = format!("attempt {}: thread blocked 15s", attempt + 1);
                eprintln!("[video] {last}");
                return Err(format!(
                    "video socket: {last} (サーバが accept しても dummy byte が来ない)"
                ));
            }
        }
    }
    Err(format!("video socket timeout: {last}"))
}

/// video ソケット接続後に device meta(64) + codec id(4) を読む。
/// これは control ソケット接続完了後(DdesktopConnection.open 完了後)に呼ぶ。
fn read_video_meta(video: &mut TcpStream) -> Result<(), String> {
    let _devname = read_full(video, 64).map_err(|e| format!("devname: {e}"))?;
    let codec = read_full(video, 4).map_err(|e| format!("codec: {e}"))?;
    let id = u32::from_be_bytes([codec[0], codec[1], codec[2], codec[3]]);
    eprintln!("[video] codec id = {:#010x}", id);
    Ok(())
}

/// デバイス上で localabstract ソケット(scrcpy_<scid>)が LISTEN 状態になるまで待つ。
/// /proc/net/unix をポーリングし、@scrcpy_<scid> エントリが出現したら返す。
fn wait_for_listen(serial: &str, scid: &str) {
    let needle = format!("@scrcpy_{}", scid);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let out = Command::new("adb")
            .args(["-s", serial, "shell", "cat /proc/net/unix"])
            .output();
        if let Ok(o) = out {
            let txt = String::from_utf8_lossy(&o.stdout);
            if txt.contains(&needle) {
                println!("listen confirmed: {needle}");
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "[warn] listen ソケット {} が15秒以内に確認できませんでした",
        needle
    );
}

/// mpsc::Receiver のタイムアウト付き受信(std の recv_timeout のラッパ)。
fn recv_timeout<T>(rx: &std::sync::mpsc::Receiver<T>, timeout: Duration) -> Option<T> {
    use std::sync::mpsc::RecvTimeoutError;
    match rx.recv_timeout(timeout) {
        Ok(v) => Some(v),
        Err(RecvTimeoutError::Timeout) => None,
        Err(RecvTimeoutError::Disconnected) => None,
    }
}

/// control ソケット接続。control ソケットは dummy byte を送らない(sendDummyByte
/// は video のみ)。接続確立のみで、すぐに制御メッセージ送受信可能。
fn connect_control(local_addr: &str) -> Result<TcpStream, String> {
    let addr: std::net::SocketAddr = local_addr
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let mut last = String::new();
    for attempt in 0..CONNECT_RETRY_MAX {
        let addr2 = addr;
        let (tx, rx) = std::sync::mpsc::channel::<Result<TcpStream, String>>();
        std::thread::spawn(move || {
            let inner = (|| -> Result<TcpStream, String> {
                let s = TcpStream::connect(addr2).map_err(|e| format!("connect: {e}"))?;
                let _ = s.set_nodelay(true);
                eprintln!("[control] connected (no dummy byte expected)");
                Ok(s)
            })();
            let _ = tx.send(inner);
        });
        match recv_timeout(&rx, Duration::from_secs(10)) {
            Some(Ok(s)) => return Ok(s),
            Some(Err(e)) => {
                last = format!("control attempt {}: {e}", attempt + 1);
                eprintln!("[control] {last}");
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
            None => {
                last = format!("control attempt {}: thread timeout", attempt + 1);
                eprintln!("[control] {last}");
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
        }
    }
    Err(format!("control socket timeout: {last}"))
}

fn read_full(s: &mut TcpStream, n: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

/// TYPE_INJECT_TOUCH_EVENT メッセージ(32 byte)を構築。
fn build_touch_msg(action: u8, args: &Args) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0] = TYPE_INJECT_TOUCH_EVENT;
    buf[1] = action;
    // pointer id = 0xffffffffffffffff (公式クライアントは POINTER_ID_VIRTUAL_FINGER = -1L)
    let pointer_id: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    buf[2..10].copy_from_slice(&pointer_id.to_be_bytes());
    buf[10..14].copy_from_slice(&args.x.to_be_bytes());
    buf[14..18].copy_from_slice(&args.y.to_be_bytes());
    buf[18..20].copy_from_slice(&(args.screen_w as u16).to_be_bytes());
    buf[20..22].copy_from_slice(&(args.screen_h as u16).to_be_bytes());
    buf[22..24].copy_from_slice(&PRESSURE_MAX.to_be_bytes());
    buf[24..28].copy_from_slice(&BUTTON_PRIMARY.to_be_bytes());
    buf[28..32].copy_from_slice(&BUTTON_PRIMARY.to_be_bytes());
    buf
}

fn send_touch_down(sock: &mut TcpStream, args: &Args) {
    let msg = build_touch_msg(ACTION_DOWN, args);
    sock.write_all(&msg).expect("write DOWN");
    sock.flush().expect("flush DOWN");
}

fn send_touch_up(sock: &mut TcpStream, args: &Args) {
    let msg = build_touch_msg(ACTION_UP, args);
    sock.write_all(&msg).expect("write UP");
    sock.flush().expect("flush UP");
}

fn pick_free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// `adb exec-out screencap -p` を取得し PNG の MD5 を返す。
fn screencap_md5(serial: &str) -> String {
    let out = Command::new("adb")
        .args(["-s", serial, "exec-out", "screencap -p"])
        .output()
        .expect("screencap");
    let png = &out.stdout;
    md5_hex(png)
}

/// 簡易 MD5(RFC 1321)。依存追加を避けるため自前実装。
fn md5_hex(data: &[u8]) -> String {
    let digest = md5_compute(data);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn md5_compute(input: &[u8]) -> [u8; 16] {
    // 定数
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    // パディング
    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for j in 0..16 {
            m[j] = u32::from_le_bytes([
                chunk[j * 4],
                chunk[j * 4 + 1],
                chunk[j * 4 + 2],
                chunk[j * 4 + 3],
            ]);
        }
        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g): (u32, usize) = if i < 16 {
                ((b & c) | (!b & d), i)
            } else if i < 32 {
                ((d & b) | (!d & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(k[i])
                    .wrapping_add(m[g])
                    .rotate_left(s[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

#[allow(dead_code)]
fn _unused_instant() -> Instant {
    Instant::now()
}
