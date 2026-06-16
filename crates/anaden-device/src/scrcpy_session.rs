//! scrcpy control session(video + control 2ソケット)。
//!
//! `scrcpy.rs` の `ScrcpyCapture` は capture 専用(control=false, video 1本)だが、
//! 本モジュールは **control=true(video+control 2ソケット)** でセッションを確立し、
//! - `capture()`: video 側最新フレーム
//! - `send_touch(x,y,action)` / `tap(x,y)`: control 側へ TYPE_INJECT_TOUCH_EVENT 送信
//! を両立する。Another Eden のような `adb shell input tap` を無視するゲームで、
//! scrcpy 経由のタッチ注入を使うための本統合コア。
//!
//! ## v4.0 ハンドシェイク(実証済み・Python 完動版と同一)
//! tunnel_forward=true でサーバは LocalServerSocket を開き video→control の順に accept。
//!   1. video ソケット接続 → **dummy byte(0x00)は video ソケットでのみ受信**(1回のみ)
//!   2. control ソケット接続(video の後) → **dummy byte なし**
//!   3. **両ソケット accept 完了後**にサーバが sendDeviceMeta(): video ソケットから
//!      device name(64byte) + codec id(4byte)
//!   4. 以降 video ストリーム本体。control は TYPE_INJECT_TOUCH_EVENT 等を client→server へ送信。
//!
//! ## Windows 安定性の核心
//! `scrcpy.rs` 旧実装(`Command::spawn` で adb shell をフォアグラウンド保持)は Windows で
//! dummy byte 受信まで数秒遅延し、サーバ Java プロセスが早期終了する現象があった。
//! 完動 Python 版(`.agent/touch_test/scrcpy_touch_probe.py`)が即座に接続できる理由は
//! **サーバを nohup + バックグラウンド化して adb shell セッションを即座に切断**している
//! こと。本実装は同じ手法をとる:
//!   - `adb forward --remove-all` で stale forward 蓄積を掃除
//!   - jar を毎セッション `adb push` で再配置(消える現象対策)
//!   - サーバを `nohup sh -c "CLASSPATH=... app_process ... " &` でバックグラウンド起動
//!     (CLASSPATH を sh -c 内で展開するので app_process へ正しく伝播)
//!   - video ソケット接続をサーバ listen までリトライ(poll /proc/net/unix)
//!   - TCP_NODELAY 設定
//!
//! **feature gate**: `capture-scrcpy` 有効時のみコンパイルされる(openh264 依存)。

use std::io::Read;
use std::net::TcpStream;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageBuffer, Rgb};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use openh264::nal_units;
use tracing::{debug, info, warn};

use crate::client::{AdbClient, AdbError};

/// デバイス上に配置する scrcpy-server jar のパス(固定)。
const REMOTE_JAR: &str = "/data/local/tmp/scrcpy-server.jar";

/// H.264 codec id(`0x68323634` = ASCII "h264")。
const CODEC_ID_H264: u32 = 0x6832_3634;

/// ソケット接続リトライ間隔(サーバが listen するまで)。
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(50);
/// ソケット接続リトライの最大回数。
const CONNECT_RETRY_MAX: u32 = 200;

/// control ソケットの dummy byte 読込タイムアウト(両 accept を待つ猶予)。
const DUMMY_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// `capture()` 初回未到達時の grace タイムアウト。
const FIRST_FRAME_GRACE: Duration = Duration::from_secs(15);

// ---- control message 定数(ControlMessage.java / MotionEvent) ----

/// TYPE_INJECT_TOUCH_EVENT = 2
const TYPE_INJECT_TOUCH_EVENT: u8 = 2;
/// MotionEvent.ACTION_DOWN = 0
pub const ACTION_DOWN: u8 = 0;
/// MotionEvent.ACTION_UP = 1
pub const ACTION_UP: u8 = 1;
/// MotionEvent.ACTION_MOVE = 2
pub const ACTION_MOVE: u8 = 2;
/// AMOTION_EVENT_BUTTON_PRIMARY = 0x00000001
const BUTTON_PRIMARY: u32 = 0x0000_0001;
/// POINTER_ID_VIRTUAL_FINGER = -1L → 0xFFFF_FFFF_FFFF_FFFF
const POINTER_ID_VIRTUAL_FINGER: u64 = 0xFFFF_FFFF_FFFF_FFFF;
/// pressure = 1.0f → 0xffff
const PRESSURE_MAX: u16 = 0xffff;

/// [`ScrcpySession`] の構成。
#[derive(Clone, Debug)]
pub struct ScrcpySessionConfig {
    /// ホスト側の scrcpy-server jar ファイルパス。
    pub local_jar_path: String,
    /// abstract socket 名のサフィックス(8桁 hex)。0x7FFFFFFF 以下・先頭桁 0-7。
    pub scid: String,
    /// `adb forward` に使うローカル TCP ポート。0 で固定 27183(完動 Python 版と同じ)。
    pub local_port: u16,
    pub max_size: u32,
    pub max_fps: u32,
    pub video_bit_rate: u64,
    /// サーバ log_level。
    pub log_level: String,
}

impl Default for ScrcpySessionConfig {
    fn default() -> Self {
        Self {
            local_jar_path: r"C:\Users\black\scoop\apps\scrcpy\current\scrcpy-server".to_string(),
            scid: "18310000".to_string(),
            local_port: 27183,
            max_size: 0,
            max_fps: 0,
            video_bit_rate: 8_000_000,
            log_level: "warn".to_string(),
        }
    }
}

/// video + control 2ソケットの scrcpy control session。
///
/// [`ScrcpySession::start`] でサーバ起動(nohup バックグラウンド)〜2ソケット確立〜
/// video 受信タスク起動までを行う。以降は [`ScrcpySession::capture`] で最新フレーム、
/// [`ScrcpySession::send_touch`] / [`ScrcpySession::tap`] でタッチ注入が可能。
/// [`Drop`] で forward 解除 + サーバ掃除を行う。
pub struct ScrcpySession {
    inner: Arc<Inner>,
    recv_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// control ソケット(send_touch 用)。受信はしないので Mutex<Option<TcpStream>>。
    control: Mutex<Option<TcpStream>>,
    forward_spec: String,
    adb_path: String,
    serial: String,
    /// touch メッセージに埋め込む画面サイズ(ネイティブ pixel)。
    /// [`ScrcpySession::set_screen_size`] で初回フレーム受信後に設定される。
    /// 未設定時は Pixel 7a 既定(1080x2400)。
    screen: Mutex<(u32, u32)>,
}

struct Inner {
    latest: Mutex<Option<DynamicImage>>,
    shutdown: AtomicBool,
}

/// タッチイベントの action。[`ScrcpySession::send_touch`] に渡す。
#[derive(Debug, Clone, Copy)]
pub enum TouchAction {
    Down,
    Up,
    Move,
}

impl TouchAction {
    fn as_byte(self) -> u8 {
        match self {
            TouchAction::Down => ACTION_DOWN,
            TouchAction::Up => ACTION_UP,
            TouchAction::Move => ACTION_MOVE,
        }
    }
}

impl ScrcpySession {
    /// サーバ起動〜2ソケット確立〜受信タスク起動を行い、capture/tap 可能なインスタンスを返す。
    pub async fn start(client: AdbClient, config: ScrcpySessionConfig) -> Result<Self, AdbError> {
        let serial = client.serial().to_string();
        let adb_path = client.adb_path().to_string();

        // (0) stale forward 掃除(蓄積でポート衝突する現象対策)。
        let _ = client
            .run_adb_raw(&["-s", &serial, "forward", "--remove-all"])
            .await;

        // (1) jar を毎セッション再 push(消える現象対策)。
        push_jar(&client, &config.local_jar_path).await?;

        // (2) localabstract 名 + forward 設定(固定ポート)。
        let localabstract = format!("scrcpy_{}", config.scid);
        let local_port = if config.local_port == 0 {
            27183
        } else {
            config.local_port
        };
        client
            .run_adb_raw(&[
                "-s",
                &serial,
                "forward",
                &format!("tcp:{local_port}"),
                &format!("localabstract:{localabstract}"),
            ])
            .await?;
        let local_addr = format!("127.0.0.1:{local_port}");

        // (3) サーバ起動(nohup バックグラウンド)。adb shell セッションを即座に切断。
        let server_cmd = build_server_command(&config);
        launch_server_background(&client, &server_cmd)?;

        // (4) サーバが LocalServerSocket を open するまで待つ(poll /proc/net/unix)。
        wait_for_listen(&client, &config.scid).await;

        // (5) ハンドシェイク: video(dummy byte 受信) → control(接続のみ) → device meta(video)。
        let (video_sock, control_sock) = connect_and_handshake(&local_addr)?;

        // (6) 共有状態 + video 受信タスク。
        let inner = Arc::new(Inner {
            latest: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });
        let inner_for_task = inner.clone();
        let recv_task = tokio::task::spawn_blocking(move || {
            run_recv_loop(inner_for_task, video_sock);
        });

        info!(
            "scrcpy session 起動完了(control=true): forward={} serial={}",
            local_addr, serial
        );

        Ok(Self {
            inner,
            recv_task: Mutex::new(Some(recv_task)),
            control: Mutex::new(Some(control_sock)),
            forward_spec: localabstract,
            adb_path,
            serial,
            screen: Mutex::new((1080, 2400)),
        })
    }

    /// 最新フレームのクローンを返す。初回未到達時は grace 待機。
    pub async fn capture(&self) -> Result<DynamicImage, AdbError> {
        let deadline = Instant::now() + FIRST_FRAME_GRACE;
        loop {
            let snapshot = self
                .inner
                .latest
                .lock()
                .map(|g| g.as_ref().map(|img| img.clone()))
                .ok()
                .flatten();
            if let Some(img) = snapshot {
                // フレーム寸法を touch メッセージの画面サイズへ反映(向き・解像度変動対応)。
                self.set_screen_size(img.width(), img.height());
                return Ok(img);
            }
            if Instant::now() >= deadline {
                return Err(AdbError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 現在 control ソケットが確立しているか。
    pub fn control_ready(&self) -> bool {
        self.control.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// touch メッセージ埋め込み用の画面サイズ(ネイティブ)を返す。
    /// [`set_screen_size`] で実測フレームサイズへ更新されていればそれ、未設定時は既定。
    pub fn screen_size(&self) -> (u32, u32) {
        self.screen.lock().map(|g| *g).unwrap_or((1080, 2400))
    }

    /// touch メッセージ埋め込み用画面サイズを更新する(初回フレーム受信後など)。
    pub fn set_screen_size(&self, w: u32, h: u32) {
        if let Ok(mut g) = self.screen.lock() {
            *g = (w, h);
        }
    }

    /// TYPE_INJECT_TOUCH_EVENT を control ソケットへ送信(32byte)。
    /// screen_w/screen_h はデフォルトで 1080x2400(実機 Pixel 7a)。
    pub fn send_touch(
        &self,
        x: u32,
        y: u32,
        action: TouchAction,
        screen_w: u32,
        screen_h: u32,
    ) -> Result<(), AdbError> {
        let msg = build_touch_msg(action.as_byte(), x, y, screen_w, screen_h);
        let mut guard = self
            .control
            .lock()
            .map_err(|_| AdbError::CommandFailed { message: "control lock poisoned".into() })?;
        let sock = guard
            .as_mut()
            .ok_or_else(|| AdbError::CommandFailed {
                message: "control ソケット未確立".into(),
            })?;
        use std::io::Write;
        sock.write_all(&msg).map_err(|e| AdbError::CommandFailed {
            message: format!("control write 失敗: {e}"),
        })?;
        sock.flush().map_err(|e| AdbError::CommandFailed {
            message: format!("control flush 失敗: {e}"),
        })?;
        debug!("send_touch: action={:?} ({},{}) {}x{}", action, x, y, screen_w, screen_h);
        Ok(())
    }

    /// DOWN → (hold) → UP のタップを送る。画面サイズは [`screen_size`] の実測値を使用。
    pub fn tap(&self, x: u32, y: u32) -> Result<(), AdbError> {
        let (w, h) = self.screen_size();
        self.tap_with(x, y, w, h, Duration::from_millis(60))
    }

    /// 画面サイズ・ホールド時間指定のタップ。
    pub fn tap_with(
        &self,
        x: u32,
        y: u32,
        screen_w: u32,
        screen_h: u32,
        hold: Duration,
    ) -> Result<(), AdbError> {
        self.send_touch(x, y, TouchAction::Down, screen_w, screen_h)?;
        std::thread::sleep(hold);
        self.send_touch(x, y, TouchAction::Up, screen_w, screen_h)?;
        Ok(())
    }

    /// DOWN → MOVE 連続 → UP のスワイプを送る。
    ///
    /// `duration_ms` を `step` 数で分割し、各ステップ間を均等に sleep しながら
    /// 線形補間した中間点へ MOVE を送る。最後に UP を送る。
    /// `step` は最低 2(DOWN + UP)。画面サイズは [`screen_size`] の実測値。
    pub fn swipe(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u64,
    ) -> Result<(), AdbError> {
        let (w, h) = self.screen_size();
        self.swipe_with(x1, y1, x2, y2, duration_ms, w, h, 16)
    }

    /// 画面サイズ・ステップ数指定のスワイプ。
    pub fn swipe_with(
        &self,
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        duration_ms: u64,
        screen_w: u32,
        screen_h: u32,
        steps: u32,
    ) -> Result<(), AdbError> {
        let steps = steps.max(2);
        let per_step = Duration::from_millis(duration_ms / steps as u64);
        // DOWN at start.
        self.send_touch(x1, y1, TouchAction::Down, screen_w, screen_h)?;
        // MOVE を steps-1 回(終点含まず均等補間)。
        for i in 1..steps {
            let t = i as f32 / steps as f32;
            let x = lerp(x1, x2, t);
            let y = lerp(y1, y2, t);
            std::thread::sleep(per_step);
            self.send_touch(x, y, TouchAction::Move, screen_w, screen_h)?;
        }
        // UP at end.
        std::thread::sleep(per_step);
        self.send_touch(x2, y2, TouchAction::Up, screen_w, screen_h)?;
        Ok(())
    }

    /// 指定座標を長押しする(DOWN → duration 待ち → UP)。
    pub fn long_press(&self, x: u32, y: u32, duration_ms: u64) -> Result<(), AdbError> {
        let (w, h) = self.screen_size();
        self.tap_with(x, y, w, h, Duration::from_millis(duration_ms))
    }
}

/// 線形補間。`t` は [0.0, 1.0]。
fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let af = a as f32;
    let bf = b as f32;
    (af + (bf - af) * t).round() as u32
}

impl Drop for ScrcpySession {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);

        if let Ok(mut guard) = self.recv_task.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }

        // forward 解除(ベストエフォート)。
        let _ = std::process::Command::new(&self.adb_path)
            .args([
                "-s",
                &self.serial,
                "forward",
                "--remove",
                &format!("localabstract:{}", self.forward_spec),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        // サーバ掃除。pkill はフォアグラウンド adb でハングすることがあるため、
        // タイムアウト付きバックグラウンドサブシェルで飛ばす。
        let _ = std::process::Command::new(&self.adb_path)
            .args([
                "-s",
                &self.serial,
                "shell",
                "(pkill -f scrcpy.Server >/dev/null 2>&1 &) ; true",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        info!("scrcpy session 停止処理完了(serial={})", self.serial);
    }
}

// ---- 起動ヘルパ ----

async fn push_jar(client: &AdbClient, local_jar: &str) -> Result<(), AdbError> {
    if !std::path::Path::new(local_jar).exists() {
        return Err(AdbError::CommandFailed {
            message: format!("scrcpy-server jar が見つかりません: {local_jar}"),
        });
    }
    client
        .run_adb_raw(&["-s", client.serial(), "push", local_jar, REMOTE_JAR])
        .await?;
    debug!("scrcpy-server jar pushed: {local_jar} -> {REMOTE_JAR}");
    Ok(())
}

/// app_process へ渡すサーバ起動コマンドライン(args)を構築(control=true)。
fn build_server_command(config: &ScrcpySessionConfig) -> Vec<String> {
    vec![
        format!("CLASSPATH={REMOTE_JAR}"),
        "app_process".to_string(),
        "/".to_string(),
        "com.genymobile.scrcpy.Server".to_string(),
        "4.0".to_string(),
        format!("scid={}", config.scid),
        format!("log_level={}", config.log_level),
        "video=true".to_string(),
        "audio=false".to_string(),
        "control=true".to_string(),
        "video_codec=h264".to_string(),
        format!("video_bit_rate={}", config.video_bit_rate),
        format!("max_size={}", config.max_size),
        format!("max_fps={}", config.max_fps),
        "tunnel_forward=true".to_string(),
        "send_device_meta=true".to_string(),
        "send_frame_meta=true".to_string(),
        "send_stream_meta=true".to_string(),
        "send_dummy_byte=true".to_string(),
    ]
}

/// サーバを **バックグラウンド**(nohup + `&`)で起動する。
///
/// 完動 Python 版が即座に接続できる理由: サーバを `nohup sh -c "CLASSPATH=... cmd" &`
/// でバックグラウンド化し、adb shell セッションを即座に切断する。こうすると:
///   - CLASSPATH は sh -c の内部で展開されるので app_process へ正しく伝播
///     (旧 scrcpy.rs が懸念した `VAR=val cmd &` の伝播問題は sh -c で回避)
///   - adb shell セッションがサーバの stdin/stdout を握らないので、Java プロセスが
///     SIGPIPE/EOF で早期終了しない
///   - stdout/stderr を /data/local/tmp/scrcpy_srv.log へリダイレクトし、adb の
///     パイプバッファ詰まりによる遅延を防ぐ
fn launch_server_background(client: &AdbClient, args: &[String]) -> Result<(), AdbError> {
    // args[0] は "CLASSPATH=/data/local/tmp/scrcpy-server.jar"。
    // ★Android 16 実証: sh -c "CLASSPATH=... app_process ..." でも
    // sh -c "export CLASSPATH=...; app_process ..." でも app_process へ CLASSPATH が
    // 伝わらず `ClassNotFoundException: com.genymobile.scrcpy.Server` で SIGABRT する。
    // 唯一確実なのは `env CLASSPATH=... app_process ...` の fork/exec 形式
    // (env は execve(2) で環境変数を渡すため app_process の ART が確実に読む)。
    // sh -c で包むと sh の環境解釈が干渉するため包まず、`nohup env ... &` とする。
    let classpath_token = args
        .first()
        .map(|s| s.as_str())
        .unwrap_or("CLASSPATH=/data/local/tmp/scrcpy-server.jar");
    let rest = args
        .iter()
        .skip(1)
        .map(|a| a.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let bg = format!(
        "nohup env {classpath_token} {rest} >/data/local/tmp/scrcpy_srv.log 2>&1 < /dev/null &"
    );
    debug!("scrcpy server bg cmd: adb -s {} shell \"{}\"", client.serial(), bg);
    // MSYS_NO_PATHCONV を立てて bash のパス変換を抑止(C:\.. が /c/.. に化けるのを防ぐ)。
    let out = std::process::Command::new(client.adb_path())
        .args(["-s", client.serial(), "shell", &bg])
        .env("MSYS_NO_PATHCONV", "1")
        .output()
        .map_err(|e| AdbError::CommandFailed {
            message: format!("scrcpy server 起動失敗: {e}"),
        })?;
    if !out.status.success() {
        return Err(AdbError::CommandFailed {
            message: format!(
                "scrcpy server 起動 exit != 0: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        });
    }
    Ok(())
}

/// デバイス上で localabstract ソケット(scrcpy_<scid>)が LISTEN 状態になるまで待つ。
async fn wait_for_listen(client: &AdbClient, scid: &str) {
    let needle = format!("@scrcpy_{}", scid);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let out = std::process::Command::new(client.adb_path())
            .args(["-s", client.serial(), "shell", "cat /proc/net/unix"])
            .env("MSYS_NO_PATHCONV", "1")
            .output();
        if let Ok(o) = out {
            let txt = String::from_utf8_lossy(&o.stdout);
            if txt.contains(&needle) {
                debug!("listen confirmed: {needle}");
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    warn!("listen ソケット {} が10秒以内に確認できず", needle);
}

/// video+control 2ソケットを正しい順序で確立し、handshake を完了する。
///
/// 手順:
///   1. video 接続(サーバ listen までリトライ) → dummy byte 受信(短タイムアウト)
///   2. control 接続(同リトライ) → dummy byte なし
///   3. video から device name(64) + codec id(4) を読む(両 accept 後に送られる)
fn connect_and_handshake(local_addr: &str) -> Result<(TcpStream, TcpStream), AdbError> {
    // (1) video ソケット + dummy byte。
    let mut video = connect_video_with_dummy(local_addr)?;

    // (2) control ソケット接続(dummy byte 無し)。
    let control = connect_control(local_addr)?;

    // (3) 両 accept 後に送られる device meta を video から読む。
    let devname = read_exact(&mut video, 64).map_err(|e| AdbError::CommandFailed {
        message: format!("device name 読込失敗: {e}"),
    })?;
    let codec_buf = read_exact(&mut video, 4).map_err(|e| AdbError::CommandFailed {
        message: format!("codec id 読込失敗: {e}"),
    })?;
    let codec_id = u32::from_be_bytes([codec_buf[0], codec_buf[1], codec_buf[2], codec_buf[3]]);
    if codec_id != CODEC_ID_H264 {
        return Err(AdbError::CommandFailed {
            message: format!("未サポート codec id: 0x{codec_id:08x}"),
        });
    }
    let name = devname
        .split(|b| *b == 0)
        .next()
        .map(|s| String::from_utf8_lossy(s).to_string())
        .unwrap_or_default();
    info!(
        "scrcpy handshake 完了: codec=H.264 device=\"{}\" (video+control both accepted)",
        name
    );
    Ok((video, control))
}

/// video ソケットを接続し dummy byte を読む。
/// dummy byte が来ない接続(forward トンネルは listen 前から accept する)は切断→再接続。
fn connect_video_with_dummy(local_addr: &str) -> Result<TcpStream, AdbError> {
    let mut last_err = None;
    for attempt in 0..CONNECT_RETRY_MAX {
        match TcpStream::connect(local_addr) {
            Ok(mut s) => {
                let _ = s.set_nodelay(true);
                let _ = s.set_read_timeout(Some(DUMMY_READ_TIMEOUT));
                match s.read_exact(&mut [0u8; 1]) {
                    Ok(_) => {
                        // dummy byte 受信。以降はブロック読みで OK。
                        let _ = s.set_read_timeout(None);
                        debug!("video dummy byte 受信(試行={})", attempt + 1);
                        return Ok(s);
                    }
                    Err(e) => {
                        last_err = Some(format!(
                            "dummy byte 読込失敗(試行={}): {e}",
                            attempt + 1
                        ));
                        std::thread::sleep(CONNECT_RETRY_INTERVAL);
                    }
                }
            }
            Err(e) => {
                last_err = Some(format!("TCP 接続失敗(試行={}): {e}", attempt + 1));
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
        }
    }
    Err(AdbError::CommandFailed {
        message: format!(
            "video ソケット確立タイムアウト({local_addr}): {}",
            last_err.unwrap_or_else(|| "unknown".into())
        ),
    })
}

/// control ソケットを接続(dummy byte なし)。接続確立のみで制御メッセージ送受信可能。
fn connect_control(local_addr: &str) -> Result<TcpStream, AdbError> {
    let mut last_err = None;
    for attempt in 0..CONNECT_RETRY_MAX {
        match TcpStream::connect(local_addr) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                let _ = s.set_read_timeout(None);
                debug!("control ソケット接続(試行={})", attempt + 1);
                return Ok(s);
            }
            Err(e) => {
                last_err = Some(format!("control 接続失敗(試行={}): {e}", attempt + 1));
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
        }
    }
    Err(AdbError::CommandFailed {
        message: format!(
            "control ソケット確立タイムアウト({local_addr}): {}",
            last_err.unwrap_or_else(|| "unknown".into())
        ),
    })
}

fn read_exact(stream: &mut TcpStream, n: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

/// TYPE_INJECT_TOUCH_EVENT メッセージ(32byte)を構築。
///
/// wire format (test_control_msg_serialize.c と完全一致):
///   [0] type=2  [1] action  [2..9] ptr_id(u64 BE)  [10..13] x(u32 BE)
///   [14..17] y(u32 BE)  [18..19] w(u16 BE)  [20..21] h(u16 BE)
///   [22..23] pressure(u16 BE)  [24..27] action_button(u32 BE)  [28..31] buttons(u32 BE)
fn build_touch_msg(action: u8, x: u32, y: u32, screen_w: u32, screen_h: u32) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[0] = TYPE_INJECT_TOUCH_EVENT;
    buf[1] = action;
    buf[2..10].copy_from_slice(&POINTER_ID_VIRTUAL_FINGER.to_be_bytes());
    buf[10..14].copy_from_slice(&x.to_be_bytes());
    buf[14..18].copy_from_slice(&y.to_be_bytes());
    buf[18..20].copy_from_slice(&(screen_w as u16).to_be_bytes());
    buf[20..22].copy_from_slice(&(screen_h as u16).to_be_bytes());
    buf[22..24].copy_from_slice(&PRESSURE_MAX.to_be_bytes());
    buf[24..28].copy_from_slice(&BUTTON_PRIMARY.to_be_bytes());
    buf[28..32].copy_from_slice(&BUTTON_PRIMARY.to_be_bytes());
    buf
}

// ---- video 受信ループ(ScrcpyCapture と同一ロジック) ----

fn run_recv_loop(inner: Arc<Inner>, mut stream: TcpStream) {
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            warn!("openh264 Decoder 生成失敗: {e}");
            return;
        }
    };
    let mut pending_config: Option<Vec<u8>> = None;
    let mut session_seen = false;

    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            return;
        }
        if !session_seen {
            match read_exact(&mut stream, 12) {
                Ok(hdr) => {
                    if hdr[0] & 0x80 != 0 {
                        session_seen = true;
                    } else {
                        process_packet_header(
                            &mut stream,
                            &hdr,
                            &mut decoder,
                            &mut pending_config,
                            &inner,
                        );
                        session_seen = true;
                    }
                }
                Err(_) => {
                    if inner.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    return;
                }
            }
            continue;
        }
        match read_exact(&mut stream, 12) {
            Ok(hdr) => {
                process_packet_header(&mut stream, &hdr, &mut decoder, &mut pending_config, &inner);
            }
            Err(_) => {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                return;
            }
        }
    }
}

fn process_packet_header(
    stream: &mut TcpStream,
    hdr: &[u8],
    decoder: &mut Decoder,
    pending_config: &mut Option<Vec<u8>>,
    inner: &Arc<Inner>,
) {
    if hdr[0] & 0x80 != 0 {
        return;
    }
    let pts_flags = u64::from_be_bytes([
        hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
    ]);
    let size = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    let is_config = pts_flags & (1u64 << 62) != 0;
    if size == 0 || size > 32 * 1024 * 1024 {
        return;
    }
    let packet = match read_exact(stream, size) {
        Ok(p) => p,
        Err(_) => return,
    };
    if is_config {
        *pending_config = Some(packet);
        return;
    }
    let to_decode: Vec<u8> = match pending_config.take() {
        Some(cfg) => {
            let mut v = cfg;
            v.extend_from_slice(&packet);
            v
        }
        None => packet,
    };
    decode_and_publish(decoder, &to_decode, inner);
}

fn decode_and_publish(decoder: &mut Decoder, bytes: &[u8], inner: &Arc<Inner>) {
    let mut produced: Option<(u32, u32, Vec<u8>)> = None;
    for nal in nal_units(bytes) {
        match decoder.decode(nal) {
            Ok(Some(yuv)) => {
                let (w, h) = yuv.dimensions();
                if w == 0 || h == 0 {
                    continue;
                }
                let mut rgb = vec![0u8; w * h * 3];
                yuv.write_rgb8(&mut rgb);
                produced = Some((w as u32, h as u32, rgb));
            }
            Ok(None) => {}
            Err(e) => {
                debug!("decode NAL エラー: {e}");
            }
        }
    }
    if let Some((w, h, rgb)) = produced {
        if let Some(buf) = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(w, h, rgb) {
            let img = DynamicImage::ImageRgb8(buf);
            if let Ok(mut guard) = inner.latest.lock() {
                *guard = Some(img);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TYPE_INJECT_TOUCH_EVENT ワイヤー形式(32byte)が仕様通りに構築されるか。
    /// test_control_msg_serialize.c の期待値と完全一致することを検証する。
    #[test]
    fn build_touch_msg_wire_format_down() {
        let msg = build_touch_msg(ACTION_DOWN, 540, 1700, 1080, 2400);
        assert_eq!(msg.len(), 32);
        // type
        assert_eq!(msg[0], TYPE_INJECT_TOUCH_EVENT);
        // action
        assert_eq!(msg[1], ACTION_DOWN);
        // pointer id = -1L (0xFFFF_FFFF_FFFF_FFFF)
        assert_eq!(
            u64::from_be_bytes([
                msg[2], msg[3], msg[4], msg[5], msg[6], msg[7], msg[8], msg[9]
            ]),
            POINTER_ID_VIRTUAL_FINGER
        );
        // x = 540
        assert_eq!(u32::from_be_bytes([msg[10], msg[11], msg[12], msg[13]]), 540);
        // y = 1700
        assert_eq!(
            u32::from_be_bytes([msg[14], msg[15], msg[16], msg[17]]),
            1700
        );
        // screen w/h = 1080/2400 (u16)
        assert_eq!(u16::from_be_bytes([msg[18], msg[19]]), 1080);
        assert_eq!(u16::from_be_bytes([msg[20], msg[21]]), 2400);
        // pressure = 0xffff
        assert_eq!(u16::from_be_bytes([msg[22], msg[23]]), PRESSURE_MAX);
        // action_button = buttons = 0x01
        assert_eq!(
            u32::from_be_bytes([msg[24], msg[25], msg[26], msg[27]]),
            BUTTON_PRIMARY
        );
        assert_eq!(
            u32::from_be_bytes([msg[28], msg[29], msg[30], msg[31]]),
            BUTTON_PRIMARY
        );
    }

    /// action バイトだけが DOWN/UP/MOVE で変わる(残りは同座標で同一)。
    #[test]
    fn build_touch_msg_action_byte_differs() {
        let down = build_touch_msg(ACTION_DOWN, 100, 200, 1080, 2400);
        let up = build_touch_msg(ACTION_UP, 100, 200, 1080, 2400);
        let mv = build_touch_msg(ACTION_MOVE, 100, 200, 1080, 2400);
        assert_eq!(down[1], 0);
        assert_eq!(up[1], 1);
        assert_eq!(mv[1], 2);
        // type 以外は座標同一なので、[2..] は全メッセージで一致すべき。
        assert_eq!(&down[2..], &up[2..]);
        assert_eq!(&down[2..], &mv[2..]);
    }

    /// Tap = DOWN(x,y) → UP(x,y) のメッセージペアが同座標になること(TouchAction 変換含む)。
    #[test]
    fn touch_action_byte_mapping() {
        assert_eq!(TouchAction::Down.as_byte(), ACTION_DOWN);
        assert_eq!(TouchAction::Up.as_byte(), ACTION_UP);
        assert_eq!(TouchAction::Move.as_byte(), ACTION_MOVE);
    }

    /// 線形補間が端点で正確、中間で妥当な値を返すこと。
    #[test]
    fn lerp_endpoints_and_midpoint() {
        assert_eq!(lerp(0, 100, 0.0), 0);
        assert_eq!(lerp(0, 100, 1.0), 100);
        assert_eq!(lerp(0, 100, 0.5), 50);
        // 逆向き
        assert_eq!(lerp(1000, 0, 1.0), 0);
        assert_eq!(lerp(1000, 0, 0.0), 1000);
    }
}
