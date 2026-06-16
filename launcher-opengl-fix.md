# ANOTHER EDEN Launcher 起動直後に消える問題 — 圀因と修正手順

> 環境: Windows 11 Home 25H2 (Build 26200.8655) / NVIDIA GeForce RTX 3050 / Dell S2721QS 4K
> 対象: `C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Launcher\Launcher.exe`
> 最終確定日: 2026-06-12（ダンプ解析による原因確定）

---

## 1. 現象

- `Launcher.exe` 起動後、`AnotherEden.exe`(ゲーム本体)がウィンドウを一瞬表示して **2〜3秒で 0xC0000005（アクセス違反）** で終了。
- Launcher.exe 自体は正常(Exit 0)。子プロセスの32bitゲーム本体がクラッシュ。
- stdout は OpenGL 初期化メッセージだけで停止:
  ```
  Ready for GLSL
  Ready for OpenGL 2.0
  deviceTextureFormat = DeviceTextureFormat::PNG(.png)
  ```
- Windows標準のApplication Errorイベントなし（ゲーム独自のBreakpadハンドラが捕捉）。

## 2. 真の原因（ダンプ解析で確定）

### フォールトモジュール: `C:\Windows\SysWOW64\ntdll.dll`（WoW64 32bit互換層）

procdump -x で取得したフルダンプを解析:

```
ExceptionCode:    0x4000001F (WoW64 single-step / 例外変換)
ExceptionAddress: 0x00000000772782D8
FAULT MODULE:     C:\Windows\SysWOW64\ntdll.dll  +0x1182D8  (base 0x77160000)
```

ロードモジュール群は WoW64スタック一色（wow64.dll / wow64cpu.dll / wow64win.dll / wow64base.dll / SysWOW64\ntdll.dll）。**クラッシュはWoW64サブシステム内**で発生。

### 因果関係

| 証拠 | 内容 |
|------|------|
| ゲーム | **32bit**（WoW64上で動作する古いcocos2d-xゲーム） |
| タイムライン | 6/7 正常 → **6/9 KB5094126 適用** → クラッシュ開始 |
| フォールト場所 | `SysWOW64\ntdll.dll`（WoW64層） |
| 結論 | **KB5094126 (OS Build 26200.8655) が WoW64層を変更し、この32bitゲームを壊した** |

Win11 24H2/25H2はWoW64層の大改造（"new WoW64"）を継続しており、KB5094126がその一環。古い32bitゲーム＋古いアンチチートドライバ(wfsdrv 2021製)との互換性が壊れた。

### ※却下した仮説

- **OpenGL ICD未登録説（誤診）**: nvoglv DLL未配置＋OpenGLDriversレジストリ空は実在した異常だが、ICDを手動登録してもクラッシュ継続 → 原因ではない。
- **アンチチートドライバ説**: wfsdrvは稼働中でエラー証拠なし。WoW64クラッシュの副次的要因の可能性はあるが主因は更新。

## 3. 修正方法: KB5094126 をアンインストール

更新は未削除（build 依然 26200.8655）。以下で6/7の状態(build 8524)に巻き戻す。

### 方法A: GUI（推奨・最も安全）
1. 設定 → Windows Update → 更新履歴 → **更新プログラムのアンインストール**
2. **KB5094126** を選択 → アンインストール
3. 再起動
> ※KB5094135（Servicing Stack）は一覧に出ない・削除不可だが正常。KB5094126だけでOK。

### 方法B: 管理者ターミナル（DISM）
```powershell
# Win+X → ターミナル(管理者)
dism /online /remove-package /packagename:Package_for_RollupFix~31bf3856ad364e35~amd64~~26100.8655.1.20 /norestart
Restart-Computer
```

### 修正後の検証
```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\black\Downloads\verify_opengl.ps1
# または単純にゲームを起動してウィンドウが維持されるか確認
```
build が 8524 に戻り、ゲームが起動し続ければ成功。

## 4. 今後の対応

KB5094126 をアンインストールしても、Windows Update は数日で再適用しようとする。再適用後にまた壊れる場合:
- **更新の一時停止**: 設定 → Windows Update → 更新プログラムの一時停止（最大5週間）
- **ゲーム側のアップデート待ち**: Wright Flyer Studios がWoW64互換性対応版を出すのを待つ
- wfsdrv アンチチートの新版提供を待つ

## 5. 調査で使用した主なコマンド

```powershell
# プロセス監視（exit code 取得）
$p = Start-Process 'C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Game\AnotherEden.exe' -WorkingDirectory 'C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Game' -PassThru
$p.WaitForExit(10000); '{0:X8}' -f $p.ExitCode   # -> C0000005

# クラッシュダンプ取得（デバッガ未検知の前にアタッチ）
procdump -accepteula -ma -e 1 -t -x 'C:\path\dumps' AnotherEden.exe

# minidump生解析（例外コード＋フォールトモジュール、外部ライブラリ不要）
# → parse2.py: ExceptionStream(type6) と ModuleListStream(type4) をstructでparse

# DISMパッケージ特定
dism /online /get-packages /format:table | findstr RollupFix
```

## 参考ソース
- [KB5094126 (OS Builds 26200.8655 / 26100.8655) — Microsoft Support](https://support.microsoft.com/en-us/topic/june-9-2026-kb5094126-os-builds-26200-8655-and-26100-8655-1a9bcba6-5f53-4075-8156-fe11ac631737)
- [Display Driver Uninstaller (DDU) — Wagnardsoft](https://www.wagnardsoft.com/display-driver-uninstaller-ddu)
