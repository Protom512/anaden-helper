//! scrcpy 常駐 capture backend。
//!
//! デバイス上の `scrcpy-server` jar を `adb push` → `app_process` で常駐起動し、
//! `adb forward` トンネル経由で video ソケットから H.264 ストリームを受信。
//! 専用 tokio タスクで openh264 デコードし、最新フレームを共有状態に都度上書きする。
//! [`ScrcpyCapture::capture`] は最新フレームのクローンを返す(初回未到達時は grace タイムアウト待機)。
//!
//! wire protocol の詳細は `docs/scrcpy-protocol.md` を参照。本実装は 5-A 既定形式
//! (codec id + session header + 12byte フレームヘッダ + packet)を採用する。
//!
//! **feature gate**: `capture-scrcpy` 有効時のみコンパイルされる。

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

/// scoop 既定のローカル scrcpy-server パス(Windows)。
const DEFAULT_LOCAL_JAR: &str = r"C:\Users\black\scoop\apps\scrcpy\current\scrcpy-server";

/// scrcpy サーバが listen を開始するまでのソケット接続リトライ間隔。
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// ソケット接続リトライの最大回数(docs 準拠 100 回)。
const CONNECT_RETRY_MAX: u32 = 100;

/// `capture()` 初回未到達時の grace タイムアウト(最初のフレーム到着を待つ上限)。
const FIRST_FRAME_GRACE: Duration = Duration::from_secs(10);

/// H.264 codec id(`0x68323634` = ASCII "h264")。
const CODEC_ID_H264: u32 = 0x6832_3634;

/// [`ScrcpyCapture`] の構成。
#[derive(Clone, Debug)]
pub struct ScrcpyConfig {
    /// ホスト側の scrcpy-server jar ファイルパス。デバイスへ push される。
    pub local_jar_path: String,
    /// abstract socket 名のサフィックスとなる scid(8桁 hex 文字列)。
    /// 同一ホストで複数セッションを立てる際の名前衝突回避に使う。
    pub scid: String,
    /// `adb forward` に使うローカル TCP ポート。0 のとき OS の空きポートを採番する。
    pub local_port: u16,
    /// エンコーダに指定する最大辺長。0 で制限なし(実機生解像度)。
    pub max_size: u32,
    /// エンコーダに指定する最大 fps。0 で制限なし。
    pub max_fps: u32,
    /// ビットレート(bps)。
    pub video_bit_rate: u64,
}

impl Default for ScrcpyConfig {
    fn default() -> Self {
        Self {
            local_jar_path: DEFAULT_LOCAL_JAR.to_string(),
            // 固定 scid(8桁 hex)でソケット名を決定する(`scrcpy_18310000`)。
            // scrcpy 4.0 は scid を Integer.parseInt(scid,16) で解釈するため
            // (1) 全桁が 0-9a-f、(2) 0x7FFFFFFF 以下(先頭桁 0-7)でなければならない。
            // これらを満たさないと NumberFormatException でサーバが即死しソケット EOF になる。
            scid: "18310000".to_string(),
            local_port: 0,
            max_size: 0,
            max_fps: 0,
            video_bit_rate: 8_000_000,
        }
    }
}

/// scrcpy 常駐 capture。
///
/// [`ScrcpyCapture::start`] でサーバ起動〜ソケット接続〜受信タスク起動までを行い、
/// 以降は [`ScrcpyCapture::capture`] で最新フレームを取得する。[`Drop`] でサーバ停止。
pub struct ScrcpyCapture {
    inner: Arc<Inner>,
    /// 受信タスクの join handle(`Drop` で abort)。
    recv_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// サーバを保持する adb shell 子プロセス(`Drop` で kill)。
    server_child: Mutex<Option<std::process::Child>>,
    /// adb forward の解除に使う localabstract 名(`Drop` で forward --remove)。
    forward_spec: String,
    /// adb パス(`Drop` でサーバ停止コマンドに使用)。
    adb_path: String,
    /// デバイスシリアル(`Drop` でサーバ停止コマンドに使用)。
    serial: String,
}

struct Inner {
    /// 最新デコード済みフレーム。未到達時は None。
    latest: Mutex<Option<DynamicImage>>,
    /// shutdown シグナル。
    shutdown: AtomicBool,
}

impl ScrcpyCapture {
    /// サーバ起動〜接続〜受信タスク起動を行い、capture 可能なインスタンスを返す。
    pub async fn start(client: AdbClient, config: ScrcpyConfig) -> Result<Self, AdbError> {
        let serial = client.serial().to_string();
        let adb_path = client.adb_path().to_string();

        // (1) jar をデバイスへ push。
        push_jar(&client, &config.local_jar_path).await?;

        // (2) localabstract 名と forward ローカルポートを決定。
        let localabstract = format!("scrcpy_{}", config.scid);
        let local_port = if config.local_port == 0 {
            pick_free_port().ok_or_else(|| AdbError::CommandFailed {
                message: "空き TCP ポートの採番に失敗".into(),
            })?
        } else {
            config.local_port
        };

        // (3) adb forward を設定。
        let local_addr = format!("127.0.0.1:{local_port}");
        client
            .run_adb_raw(&[
                "-s",
                &serial,
                "forward",
                &format!("tcp:{local_port}"),
                &format!("localabstract:{localabstract}"),
            ])
            .await?;

        // (4) app_process でサーバを起動(adb shell フォアグラウンド子プロセスとして保持)。
        //     spawn は非同期に返り、子プロセスは接続〜ストリーミングを続ける。
        let server_cmd = build_server_command(&config);
        let server_child = spawn_server(&client, &server_cmd)?;

        // (5) video ソケットへ接続 + ハンドシェイク(サーバの accept 待ちをリトライ)。
        //     dummy byte が届くまで接続をリトライする(プロトコル準拠)。
        let stream = connect_and_handshake(&local_addr)?;

        // (7) 共有状態 + 受信タスク起動。
        let inner = Arc::new(Inner {
            latest: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });
        let inner_for_task = inner.clone();
        let recv_task = tokio::task::spawn_blocking(move || {
            run_recv_loop(inner_for_task, stream);
        });

        info!(
            "scrcpy capture 起動完了: forward={} child_pid={} (serial={})",
            local_addr,
            server_child.id(),
            serial
        );

        Ok(Self {
            inner,
            recv_task: Mutex::new(Some(recv_task)),
            server_child: Mutex::new(Some(server_child)),
            forward_spec: localabstract,
            adb_path,
            serial,
        })
    }

    /// 最新フレームのクローンを返す。
    ///
    /// 初回フレーム未到達時は `FIRST_FRAME_GRACE` まで到着を待つ。
    /// 期限切れ・受信タスク終了時は [`AdbError::Timeout`] を返す。
    pub async fn capture(&self) -> Result<DynamicImage, AdbError> {
        // grace 待ち: 100ms ごとにポーリング。
        let deadline = Instant::now() + FIRST_FRAME_GRACE;
        loop {
            // std::sync::Mutex を await 跨ぎで保持しないよう、即 clone → drop。
            let snapshot = self
                .inner
                .latest
                .lock()
                .map(|g| g.as_ref().map(|img| img.clone()))
                .ok()
                .flatten();
            if let Some(img) = snapshot {
                return Ok(img);
            }
            if Instant::now() >= deadline {
                return Err(AdbError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for ScrcpyCapture {
    fn drop(&mut self) {
        // shutdown シグナル。受信タスクは次ループで自終了するが、念のため abort も行う。
        self.inner.shutdown.store(true, Ordering::SeqCst);

        // 受信タスクの abort(同期的に await できないので abort のみ)。
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

        // サーバ停止。フォアグラウンド adb 子プロセスを kill すると、
        // 連動してデバイス側の scrcpy サーバも終了する(adb shell セッション切断)。
        if let Ok(mut guard) = self.server_child.try_lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        // 最終防護: scrcpy サーバプロセスを名前で掃除。
        let _ = std::process::Command::new(&self.adb_path)
            .args([
                "-s",
                &self.serial,
                "shell",
                "pkill -f com.genymobile.scrcpy.Server 2>/dev/null; true",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        info!("scrcpy capture 停止処理完了(serial={})", self.serial);
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
        .run_adb_raw(&[
            "-s",
            client.serial(),
            "push",
            local_jar,
            REMOTE_JAR,
        ])
        .await?;
    debug!("scrcpy-server jar pushed: {local_jar} -> {REMOTE_JAR}");
    Ok(())
}

/// app_process へ渡すサーバ起動コマンドライン(args)を構築する。
fn build_server_command(config: &ScrcpyConfig) -> Vec<String> {
    // 第1引数は client version("4.0")。一致チェックで即死するため固定。
    vec![
        format!("CLASSPATH={REMOTE_JAR}"),
        "app_process".to_string(),
        "/".to_string(),
        "com.genymobile.scrcpy.Server".to_string(),
        "4.0".to_string(),
        format!("scid={}", config.scid),
        "log_level=info".to_string(),
        "video=true".to_string(),
        "audio=false".to_string(),
        "control=false".to_string(),
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

/// `adb shell app_process ...` を起動し、**フォアグラウンドの adb 子プロセス**として返す。
///
/// 公式 scrcpy クライアントと同じく、サーバプロセスは adb shell セッションが生きている間
/// 動き続ける。本関数は adb 子プロセスをブロックさせたまま返し、呼び出し側が [`Drop`] で
/// kill するまで保持する。
///
/// なぜバックグラウンド化(`&`)しないか: Android の toybox/mksh シェルは `VAR=val cmd &`
/// (および `export VAR=...; cmd &` / `env VAR=... cmd &`)のバックグラウンド化で、VAR を
/// サブシェルへ伝播させず app_process が CLASSPATH を見逃す。結果として
/// `ClassNotFoundException: com.genymobile.scrcpy.Server` でサーバが即死する。
/// フォアグラウンドなら環境変数が正しく伝播する(実機 logcat で確認済み)。
fn spawn_server(client: &AdbClient, args: &[String]) -> Result<std::process::Child, AdbError> {
    // args[0] は `CLASSPATH=...`。adb へ shell 文字列として1つに結合して渡す。
    let shell_cmd = args
        .iter()
        .map(|a| a.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let child = std::process::Command::new(client.adb_path())
        .args(["-s", client.serial(), "shell", &shell_cmd])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AdbError::CommandFailed {
            message: format!("scrcpy server 起動(spawn)失敗: {e}"),
        })?;
    debug!("scrcpy server adb 子プロセス起動: pid={}", child.id());
    Ok(child)
}

/// video ソケットへ接続し、プロトコル準拠のハンドシェイクを行う。
///
/// scrcpy tunnel_forward=true の正しい確立手順(demuxer.c / DesktopConnection.java 準拠):
///   1. TCP 接続(サーバが listen するまでリトライ)
///   2. dummy byte(1) を読む — **読めなければサーバ未起動/未accept として切断し再接続**
///   3. device name(64) を読み捨て
///   4. codec id(4) を読んで H.264 を確認
///
/// ステップ2が重要: adb forward トンネルはサーバが listen する前から TCP 接続を受け付ける。
/// したがって「TCP 接続成功」だけでは確立と見なせず、dummy byte が届いて初めてサーバが
/// accept したと判断できる。dummy byte が来ない接続は破棄して再接続する。
fn connect_and_handshake(local_addr: &str) -> Result<TcpStream, AdbError> {
    let mut last_err = None;
    for attempt in 0..CONNECT_RETRY_MAX {
        match TcpStream::connect(local_addr) {
            Ok(mut s) => {
                let _ = s.set_nodelay(true);
                // dummy byte を短タイムアウト(500ms)で読む。読めなければ切断→再接続。
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                match s.read_exact(&mut [0u8; 1]) {
                    Ok(_) => {
                        // dummy byte 受信 = サーバ accept 確立。残りハンドシェイクへ。
                        // 以降はブロック読みで OK(タイムアウト解除)。
                        let _ = s.set_read_timeout(None);
                        complete_handshake(&mut s)?;
                        debug!(
                            "video ソケット確立(dummy byte 受信、試行={})",
                            attempt + 1
                        );
                        return Ok(s);
                    }
                    Err(e) => {
                        // 接続は成立したがサーバが未accept/即切断。破棄してリトライ。
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

/// dummy byte 受信後の残りハンドシェイク: device name(64) + codec id(4)。
fn complete_handshake(stream: &mut TcpStream) -> Result<(), AdbError> {
    // device name(64)。破棄。
    read_exact(stream, 64)?;

    // codec id(4, big-endian)。
    let codec_buf = read_exact(stream, 4)?;
    let codec_id = u32::from_be_bytes([
        codec_buf[0],
        codec_buf[1],
        codec_buf[2],
        codec_buf[3],
    ]);
    if codec_id != CODEC_ID_H264 {
        // 特殊値(0x00 / 0x01)もここで捕捉される。
        return Err(AdbError::CommandFailed {
            message: format!(
                "未サポートまたはエラーの codec id: 0x{codec_id:08x}(H.264=0x68323634 期待)"
            ),
        });
    }
    debug!("handshake 完了: codec=H.264");
    Ok(())
}

fn read_exact(stream: &mut TcpStream, n: usize) -> Result<Vec<u8>, AdbError> {
    let mut buf = vec![0u8; n];
    stream
        .read_exact(&mut buf)
        .map_err(|e| AdbError::CommandFailed {
            message: format!("ソケット読込失敗({n} byte): {e}"),
        })?;
    Ok(buf)
}

/// 空き TCP ポートを採番する。listener を bind して即 close する簡易実装。
fn pick_free_port() -> Option<u16> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

// ---- 受信ループ(spawn_blocking 上で動く同期ループ) ----

fn run_recv_loop(inner: Arc<Inner>, mut stream: TcpStream) {
    // 受信バッファ。フレームヘッダ単位でパースする。
    // session header(12) → packet header(12) + packet を順に読む。
    let mut decoder = match Decoder::new() {
        Ok(d) => d,
        Err(e) => {
            warn!("openh264 Decoder 生成失敗: {e}");
            return;
        }
    };

    // config packet(SPS/PPS)を次の media packet に prepend するための保留バッファ。
    let mut pending_config: Option<Vec<u8>> = None;

    // session header(12byte)。初回またはリサイズ時に来る。
    let mut session_seen = false;

    loop {
        if inner.shutdown.load(Ordering::SeqCst) {
            debug!("recv loop: shutdown シグナル検知、終了");
            return;
        }

        // session header が未読なら読む。
        if !session_seen {
            match read_exact(&mut stream, 12) {
                Ok(hdr) => {
                    // 先頭 bit が 1 なら session packet(demuxer.c:118-121)。
                    if hdr[0] & 0x80 != 0 {
                        let _w = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
                        let _h = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
                        debug!("session header 受信: {}x{}", _w, _h);
                        session_seen = true;
                    } else {
                        // session 無しでいきなり media packet が来るケース(念のため処理続行)。
                        // この 12byte を media フレームヘッダとして扱う。
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
                Err(e) => {
                    if inner.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    warn!("recv loop: session header 読込失敗: {e}");
                    return;
                }
            }
            continue;
        }

        // 12byte フレームヘッダを読む。
        match read_exact(&mut stream, 12) {
            Ok(hdr) => {
                process_packet_header(&mut stream, &hdr, &mut decoder, &mut pending_config, &inner);
            }
            Err(e) => {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                warn!("recv loop: packet header 読込失敗(サーバ停止?): {e}");
                return;
            }
        }
    }
}

/// 12byte フレームヘッダを処理し、後続 packet をデコードして最新フレームを更新する。
fn process_packet_header(
    stream: &mut TcpStream,
    hdr: &[u8],
    decoder: &mut Decoder,
    pending_config: &mut Option<Vec<u8>>,
    inner: &Arc<Inner>,
) {
    // bit63(先頭 byte の MSB)が 1 のとき session packet(リサイズ等)。
    if hdr[0] & 0x80 != 0 {
        // session packet はフレーム本体を持たないので何も読まない。
        debug!("recv: session packet を検出(リサイズ候補)、スキップ");
        return;
    }

    // PTS+flags(big-endian u64)と packet size(u32)を取り出す。
    let pts_flags = u64::from_be_bytes([
        hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
    ]);
    let size = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    let is_config = pts_flags & (1u64 << 62) != 0;
    let _is_keyframe = pts_flags & (1u64 << 61) != 0;

    if size == 0 || size > 32 * 1024 * 1024 {
        // 異常サイズ。破棄して次へ。
        warn!("recv: 異常 packet size={size}、スキップ");
        return;
    }

    let packet = match read_exact(stream, size) {
        Ok(p) => p,
        Err(e) => {
            warn!("recv: packet 本体読込失敗(size={size}): {e}");
            return;
        }
    };

    // CONFIG パケット(SPS/PPS 等)は次の media packet の先頭に結合する(demuxer.c:271-272 準拠)。
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

    // openh264 は Annex-B start code 付き NAL を期待。MediaCodec 出力は Annex-B 既定。
    // 結合済みバイト列内の各 NAL を順に食わせ、最後に出力された YUV を採用する。
    decode_and_publish(decoder, &to_decode, inner);
}

/// NAL 単位でデコードし、得られた最新 YUV フレームを RGB DynamicImage にして共有状態へ格納する。
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
        match ImageBuffer::<Rgb<u8>, Vec<u8>>::from_raw(w, h, rgb) {
            Some(buf) => {
                let img = DynamicImage::ImageRgb8(buf);
                // 共有状態へ上書き。短時間保持なので std::sync::Mutex で OK。
                if let Ok(mut guard) = inner.latest.lock() {
                    *guard = Some(img);
                    debug!(
                        "scrcpy frame decoded+poked: {}x{} at {:?}",
                        w,
                        h,
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    );
                }
            }
            None => {
                debug!("RGB buffer size mismatch({w}x{h})、フレーム破棄");
            }
        }
    }
}
