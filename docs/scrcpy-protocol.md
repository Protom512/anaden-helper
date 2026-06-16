# scrcpy Wire Protocol 仕様 (v4.0)

Genymobile/scrcpy `v4.0` (tag `v4.0`, commit `2322868e9e256eb5fce0b3d659ab2a409f29bae1`) の
サーバ/クライアント間ワイヤープロトコル。公式ソース(raw.githubusercontent.com)を実読して抽出。
anaden-helper の常駐 capture 実装(自前 scrcpy クライアント)に必要な部分を中心に記述する。

ローカル環境:
- `scrcpy --version` → `4.0` (SDL 3.4.8, libavcodec 62.28.101)
- サーバ jar: `C:\Users\black\ scoop\apps\scrcpy\current\scrcpy-server` (732226 bytes, scoop manifest version 4.0)
- デバイス: Pixel 7a / Android 16 (SDK 36)

---

## 1. バージョン一致の強制 (最重要)

`Options.parse()` は **第1引数を client version として読み、サーバ jar の `BuildConfig.VERSION_NAME`
と一致しないと即 `IllegalArgumentException` で落ちる** (Options.java:323-327)。

```
adb shell CLASSPATH=/data/local/tmp/scrcpy-server.jar \
  app_process / com.genymobile.scrcpy.Server 4.0 ...
                                  ^^^^ 第1引数 = "4.0" 必須
```

`raw_stream=true` 等の他オプションを変えても、この第1引数は常に `"4.0"` に固定する。
自前クライアントとデバイス上の jar のマイナーバージョンがずれると動かない点に注意。

---

## 2. サーバ起動コマンド (execute_server, server.c:204-)

公式クライアントが組み立てるコマンドライン。自前実装も同じ形を再現する。

```
adb -s <serial> shell \
  CLASSPATH=/data/local/tmp/scrcpy-server.jar \
  app_process \
  /                                      # unused (arg0)
  com.genymobile.scrcpy.Server           # main class
  4.0                                    # args[0] = client version (必須)
  scid=<8桁hex>                          # args[1..] = key=value 形式
  log_level=info
  video=true                             # 省略時 true
  audio=false                            # capture 不要なら false 推奨
  video_codec=h264                       # 省略時 h264
  video_bit_rate=8000000                 # 省略時 8000000
  max_size=0                             # 省略時 0 (=制限なし)
  max_fps=0                              # 省略時 0 (=制限なし)
  tunnel_forward=true                    # 後述
  control=false                          # capture 専用なら false
  send_device_meta=true                  # 省略時 true
  send_frame_meta=true                   # 省略時 true
  send_stream_meta=true                  # 省略時 true
  send_dummy_byte=true                   # 省略時 true
```

### キー=key=value パーサの仕様 (Options.java:331-566)
- 全引数 `key=value` 形式。`=` が無いと `IllegalArgumentException`。
- 未知キーは警告のみで無視(default 句)。**壊れない**。
- 数値は基本10進。ただし `scid` だけは16進 (`Integer.parseInt(value, 0x10)`)。
- `raw_stream=true` を渡すと `send_device_meta/send_frame_meta/send_dummy_byte/send_stream_meta`
  が**全て false** になる (Options.java:554-562)。**生 H.264 Annex-B NAL 連続ストリーム**になり、
  実装が最も単純になる。後述。

### main class とクラスパス
- `CLASSPATH` = デバイス上の jar パス (公式は `/data/local/tmp/scrcpy-server.jar` に push)。
- エントリクラス = `com.genymobile.scrcpy.Server` (Server.java:212 `main(String...)`)。
- jar は server ソースツリーの `server/` がビルド対象 (`app/src/server/...` ではない。v4.0 で移動済)。

---

## 3. ソケット: 3本の独立ソケット方式 (v4.0)

**v4.0 は video / audio / control を 3 つの独立した abstract Unix ソケットで分離する。**
1ソケット多重化ではない (DesktopConnection.java:22-39)。

| ソケット | 役割 | 有無スイッチ |
|---|---|---|
| video | H.264/H.265/AV1 エンコード済みフレーム (サーバ→クライアント) | `video=true` (既定) |
| audio | OPUS/AAC/FLAC/RAW エンコード済みパケット (サーバ→クライアント) | `audio=true` (既定) |
| control | 入力注入イベント (クライアント→サーバ) + デバイス→クライアントのデバイスメッセージ | `control=true` (既定) |

各ソケットは **常に video → audio → control の順** で accept/connect される
(DesktopConnection.java:64-101, server.c:620-700)。順序を守らないと接続が交叉して壊れる。

### ソケット名 (abstract namespace)
`scrcpy_<scid_8桁hex>` (DesktopConnection.java:47-54, server.c:1066)。
- `scid=-1` (Options の既定値) のときは `scrcpy` (プレフィックスのみ)。
- 自前クライアントが scid を固定すれば名前衝突を回避できる。

### トンネル方式: forward / reverse

**方式 A — `adb forward` (tunnel_forward=true, DesktopConnection は LocalServerSocket で accept)**

1. クライアント側ホスト: 適当なローカル TCP ポート `P` を開く。
2. `adb forward tcp:P localabstract:scrcpy_<scid>` を実行。
3. デバイス側サーバは `LocalServerSocket("scrcpy_<scid>")` を開き、
   video/audio/control を順に `accept()` する。
4. クライアントは `127.0.0.1:P` に TCP 接続を **3 本** 張る(video→audio→control)。
   サーバ側 accept 順と 1:1 対応する。

**方式 B — `adb reverse` (tunnel_forward=false, DesktopConnection は LocalSocket で connect)**

1. クライアント側ホスト: 待受 TCP サーバソケットをポート `P` で開く。
2. `adb reverse localabstract:scrcpy_<scid> tcp:P` を実行。
3. デバイス側サーバは `LocalSocket` で `scrcpy_<scid>` に **3 本** connect する(video→audio→control)。
4. クライアントは待受ソケットで **3 回 accept** する。

> anaden-helper の Windows 環境では方式 A (`adb forward`) が最も安定。
> `connect_to_server` は 100ms 間隔・最大 100 回リトライする (server.c:502-531)。

---

## 4. ハンドシェイク (接続直後のやり取り)

`send_dummy_byte=true` (既定) のとき、**各ソケットの接続確立後、サーバは最初に 1 byte を送る**。
値は `0x00`。目的は「adb トンネルの裏側でサーバがまだ listen していないのに TCP 接続だけ成立して
しまう」問題の検出 (DesktopConnection.java:68-89, server.c:482-499)。

```
クライアントは接続ごとに:
  1. TCP 接続 (方式A) / accept (方式B)
  2. 1 byte 受信 (dummy byte=0x00) を待つ → 受信できなければサーバ未起動としてリトライ
```

その後、`send_device_meta=true` (既定) のとき、**最初のソケット(video 無ければ audio、更に無ければ control)**
から device name を **64 byte 固定長** で読む (DesktopConnection.java:155-165, server.c:588-602)。
UTF-8、末尾は 0x00 パディング。実デバイス名が入る。

```
最初のソケット(video)のバイト列:
  [0]      = 0x00           (dummy byte)
  [1..64]  = device name    (64 byte, UTF-8, 0パディング)
  --- ここからストリーム本体 (codec id, session header, packets) ---
```

> 自前 capture では video ソケットだけ使う(audio=false, control=false)のが最短経路。
> この場合 dummy byte 1 byte + device name 64 byte を読み捨ててからデマルチプレクス開始。

---

## 5. video ソケットのストリーム形式 (★★★ 最重要 ★★★)

**既定(`send_stream_meta=true`, `send_frame_meta=true`)は「メタデータ付きフレーム列」。**
**`raw_stream=true` は「生 H.264 Annex-B NAL 連続」。** 実装方針で形式が完全に変わる。

### 5-A. 既定形式 (推奨: 実装堅牢、リサイズ追従可能)

video ソケットのバイト列(directly after the dummy byte + 64-byte device name):

```
┌─────────────────────────────────────────────────────────────────┐
│ (1) codec id                : 4 byte, big-endian                │ ← writeVideoHeader()
├─────────────────────────────────────────────────────────────────┤
│ (2) session header          : 12 byte                           │ ← writeSessionMeta()
├─────────────────────────────────────────────────────────────────┤
│ (3) packet header + packet  : 12 byte header + N byte packet    │ ← writePacket() × 繰り返し
│    ... (3) の繰り返し ...                                         │
└─────────────────────────────────────────────────────────────────┘
```

#### (1) codec id (4 byte, big-endian) — Streamer.writeVideoHeader (Streamer.java:48-55)
ASCII 4 文字を big-endian uint32 に詰めたもの (VideoCodec.java:9-12)。
0x00 埋めで 3 文字のコーデックもある。

| codec | id (hex)    | ASCII |
|-------|-------------|-------|
| H264  | `0x68323634`| `h264`|
| H265  | `0x68323635`| `h265`|
| AV1   | `0x00617631`| `\0av1`|

受信側はこれで H.264/H.265/AV1 を判別し、デコーダを選ぶ (demuxer.c:19-52)。
特殊値: id=`0x00000000` は「ストリーム明示的無効化」、id=`0x00000001` は「設定エラーで停止」
(Streamer.java:57-66, demuxer.c:183-195)。

#### (2) session header (12 byte) — Streamer.writeSessionMeta (Streamer.java:91-105)
エンコーダ起動直後に 1 回(リサイズごとに再送)送られる。

```
 byte 0   byte 1   byte 2   byte 3
 1....... ........ ........ .......c
 ^                                  ^
 `- bit63 = session packet flag     `- bit0 of byte3 = client_resized flag

 byte 4..7  = width  (big-endian uint32)
 byte 8..11 = height (big-endian uint32)
```

- `byte[0] & 0x80 != 0` が session packet の識別子 (demuxer.c:118-121)。
- `byte[3] & 0x01` が「クライアント要求によるリサイズ」フラグ。
- width/height は **符号付き 32 bit を big-endian で putInt** しているが、実用上は正の整数。

#### (3) packet header + packet — Streamer.writePacket / writeFrameMeta (Streamer.java:68-124)

12 byte header の後に packet 本体が続く。header の MSB が 0 であることが media packet の識別子。

```
 byte 0..7  = PTS + flags (big-endian uint64)
 byte 8..11 = packet size (big-endian uint32)  ← 続く packet のバイト長
 [size byteの生パケット]
```

PTS+flags のビットレイアウト (Streamer.java:17-19, demuxer.c:14-17):

```
 bit 63 : (0 = media packet)  ← session packet とはここで区別
 bit 62 : CONFIG フラグ (1 = MediaCodec BUFFER_FLAG_CODEC_CONFIG。CSD/SPS/PPS 等)
 bit 61 : KEY_FRAME フラグ (1 = I フレーム)
 bit 60..0 : PTS (presentationTimeUs、マイクロ秒)
```

- **config packet のとき PTS フィールドは `1<<62` 単体** (PTS 値は無意味、AV_NOPTS_VALUE として扱う)。
- H.264/H.265 では config packet (SPS/PPS、VPS) を**次の media packet の先頭に結合してから**
  デコーダに食わせる必要がある (demuxer.c:271-272, 308-315)。openh264 に渡す際も同様。
- packet 本体は **MediaCodec が出した生バイト列**。H.264 の場合は **Annex-B 形式**
  (`00 00 00 01` start code 付き NAL) が Android MediaCodec の既定出力。

### 5-B. raw_stream 形式 (実装最短、capture 専用で十分)

`raw_stream=true` をサーバ引数に渡すと (Options.java:554-562):
- `send_stream_meta=false` → codec id / session header が来ない
- `send_frame_meta=false` → 12 byte フレームヘッダが来ない
- `send_device_meta=false` → 64 byte device name が来ない
- `send_dummy_byte=false` → dummy byte が来ない

結果: **video ソケットは純粋な H.264 Annex-B NAL 連続バイトストリームのみ**。
SPS/PPS (config NAL) もフレームも start code 区切りでそのまま流れてくる。

```
video ソケット = [ H.264 Annex-B byte stream (無限) ]
  00 00 00 01 67 42 00 ...  (SPS)
  00 00 00 01 68 CE ...     (PPS)
  00 00 00 01 65 ...        (IDR slice)
  00 00 00 01 41 ...        (P slice)
  ...
```

これをそのまま openh264 クレートのデコーダ (`Decoder::decode_byte_stream` 相当) に流せばよい。
ただし **width/height を事前に知る手段が無い** ため、SPS から自前で解析するか、最初のフレームを
デコードして得られる解像度を使う。リサイズ追従もヘッダが無いため自力 NAL 解析が必要。

> **実装推奨**: anaden-helper の常駐 capture では **5-A 既定形式** を採る。
> codec id で H.264 を確認 → session header で width/height 取得 → フレームヘッダ(12byte)ごとに
> read して openh264 へ投下、という構造が最もデバッグしやすく、既に openh264 1.4ms/frame 実証済みの
> パスと親和する。`raw_stream` は width/height が取れない点で認識ROI固定用途に不利。

---

## 6. control ソケット (入力注入) — scrcpy-touch 統合

control=true のとき video とは別の独立ソケット。双方向(client→server: ControlMessage、
server→client: DeviceMessage)。tunnel_forward 方式では **video → control の順** で accept する。

### dummy byte と device meta の送信タイミング (★重要: §4 の誤り訂正)

§4 は「各ソケットに dummy byte が来る」「最初のソケットから device meta が来る」と書いていたが、
v4.0(`send_dummy_byte=true`, `send_device_meta=true`)の**実証済み**挙動は以下の通り:

1. **dummy byte(0x00)は video ソケットでのみ 1 回送られる**。control ソケットには来ない。
2. **device name(64byte) + codec id(4byte) は両ソケット accept 完了後に video ソケットから
   送られる**(`DesktopConnection` が video と control 両方の accept を終えた後に
   `sendDeviceMeta()` を呼ぶ)。従って device meta を読む前に control ソケットの accept まで
   済ませる必要がある。順序を間違えると device meta 受信で永遠にブロックする。

```
video ソケット接続後のバイト列:
  [0]      = 0x00                          (dummy byte, video のみ)
  [1..64]  = device name                   (64byte, UTF-8, 0x00 padding)
  --- ここからストリーム本体 ---
control ソケット接続後:
  (dummy byte なし・device meta なし。即 control message 送受信可能)
```

> 自前 capture(control=false) の場合は video ソケットだけで完結するが、
> **control=true のときは control ソケット接続を先に済ませてから** device meta を読むこと。

### クライアント→サーバ: ControlMessage (ControlMessage.java)
type 1 byte + 型固有ペイロード。主要 type 定数:

| 値 | type | 概要 |
|----|------|------|
| 0  | TYPE_INJECT_KEYCODE | キーイベント (action/keycode/repeat/metastate) |
| 1  | TYPE_INJECT_TEXT | UTF-8 テキスト注入 (length prefix + bytes) |
| 2  | TYPE_INJECT_TOUCH_EVENT | マルチタッチ (pointerId, action, Point, screen_size, pressure) |
| 3  | TYPE_INJECT_SCROLL_EVENT | ホイールスクロール |
| 4  | TYPE_BACK_OR_SCREEN_ON | BACK / 電源 |
| 17 | TYPE_RESET_VIDEO | video エンコーダ再起動要求 |
| 21 | TYPE_RESIZE_DISPLAY | 仮想ディスプレイリサイズ |

### TYPE_INJECT_TOUCH_EVENT (★座標は Point = uint32x2, screen_size = uint16x2)

**§4 旧記述の「Position = uint16x2」は誤り。** v4.0 の touch event は
`Point` (uint32 x, uint32 y) で座標を送り、続けて `screen_size` (uint16 w, uint16 h) を送る。
全フィールド big-endian。全体で **32 byte 固定長**。

```
 offset  size  field
 [0]     1     type = 2 (TYPE_INJECT_TOUCH_EVENT)
 [1]     1     action (MotionEvent: ACTION_DOWN=0 / UP=1 / MOVE=2)
 [2]     8     pointer_id (i64 BE; POINTER_ID_VIRTUAL_FINGER = -1L = 0xFFFFFFFFFFFFFFFF)
 [10]    4     position.x (u32 BE)
 [14]    4     position.y (u32 BE)
 [18]    2     screen_size.width  (u16 BE)
 [20]    2     screen_size.height (u16 BE)
 [22]    2     pressure (u16 BE; 1.0f → 0xFFFF = 65535)
 [24]    4     action_button (u32 BE; AMOTION_EVENT_BUTTON_PRIMARY = 0x00000001)
 [28]    4     buttons (u32 BE; 同上 0x00000001)
```

screen_size には video フレームのネイティブ解像度(例: Pixel 7a 縦持ち 1080x2400)を入れる。
タップ = DOWN(action=0) → (hold) → UP(action=1) を同座標・同画面サイズで送る。
スワイプ = DOWN → MOVE を補間点で連続送信 → UP。

> 実証済み wire format: anaden-helper `scrcpy_session.rs::build_touch_msg` がこの形式を生成し、
> `test_control_msg_serialize.c` の期待バイト列と完全一致(同モジュールの
> `build_touch_msg_wire_format_down` テストで検証済み)。Another Eden のように
> `adb shell input tap` を無視するゲームでも、この scrcpy-touch 経由で実効する。

capture 専用なら `control=false` でソケット自体を張らない(video 1 本で完結)。

### サーバ→クライアント: DeviceMessage
type 1 byte + ペイロード。クリップボード・エラー・video サイズ変更等。
`DeviceMessageWriter` / `DeviceMessageSender` が書き出す。

---

## 7. 実装チェックリスト (capture クライアント最小構成)

1. **サーバ push & 起動**
   - `adb push scrcpy-server /data/local/tmp/scrcpy-server.jar`
   - `adb shell CLASSPATH=/data/local/tmp/scrcpy-server.jar app_process / com.genymobile.scrcpy.Server 4.0 ...`
   - 第1引数 `"4.0"` を忘れない(一致チェックで即死)。

2. **引数(capture 最小)**
   ```
   video=true audio=false control=false
   send_device_meta=true send_frame_meta=true send_stream_meta=true send_dummy_byte=true
   video_codec=h264 tunnel_forward=true scid=<固定hex>
   max_size=<ROIに余裕を持った値> max_fps=<必要fps>
   ```
   (`raw_stream=true` は width/height が取れないので非推奨、5-A を使う)

3. **ソケット確立(forward 方式)**
   - `adb forward tcp:<P> localabstract:scrcpy_<scid>`
   - `127.0.0.1:<P>` に TCP 接続 1 本(video のみ)。
   - 1 byte 受信(dummy=0x00) → 64 byte 受信(device name、破棄)。
   - 以降がストリーム本体。

4. **デマルチプレクス(5-A 既定形式)**
   - 4 byte 受信 → codec id。`0x68323634` (h264) を確認。
   - 12 byte 受信 → session header。先頭 bit=1 を確認、width/height 取得(byte4-7/8-11 big-endian)。
   - ループ:
     - 12 byte 受信 → header。先頭 bit=0 (media) を確認。
     - bit62=CONFIG / bit61=KEY_FRAME / 下位=PTS を解析。
     - 4 byte packet size を読み、その長さだけ生パケットを受信。
     - CONFIG パケットは次の media パケットに prepend してから openh264 へ。
   - openh264 デコーダで YUV420P → DynamicImage 変換(実証済み 1.4ms/frame)。

5. **終了**
   - ソケットを閉じるとサーバは `Broken pipe` を検知して `SurfaceEncoder` スレッドが終了し、
     `Looper` が quit して `app_process` が exit する (SurfaceEncoder.java:353-357, Server.java:170-194)。

---

## 8. 参照した公式ソース (raw.githubusercontent.com / tag v4.0)

サーバ(Java, `server/src/main/java/com/genymobile/scrcpy/`):
- `Server.java` — main, scrcpy(), AsyncProcessor 起動
- `Options.java` — コマンドライン key=value パーサ・全デフォルト値
- `device/DesktopConnection.java` — ソケット accept/connect, dummy byte, device meta
- `device/Streamer.java` — writeVideoHeader/writeSessionMeta/writeFrameMeta/writePacket
- `video/VideoCodec.java` — codec id 定数 (H264=0x68323634 等)
- `video/SurfaceEncoder.java` — ヘッダ/セッション/パケットの書き出し順序
- `control/ControlMessage.java` — control type 定数
- `util/IO.java` — writeFully
- `model/Codec.java` — Codec インターフェース

クライアント(C, `app/src/`):
- `server.c` — execute_server (app_process コマンド組み立て), connect_to_server, device_read_info
- `demuxer.c` — ワイヤーフォーマットの公式解説コメント + パース実装(最も権威ある記述)
- `decoder.h` / `decoder.c` — FFmpeg 側デコード
