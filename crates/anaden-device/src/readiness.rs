//! ゲーム起動の readiness (splash-hang) 判定コア — プラットフォーム非依存の純粋ドメイン層。
//!
//! `ensure-open` の exit 0 はプロセス生存のみを主張し、ゲームが起動処理を完走したか
//! (ready か) を検証しない。2026-11-03/04 の live セッション (Issue #260 / #275) では
//! cold boot 5 回中 5 回が splash frame で hang してもプロセスは生存し続け、
//! 「ensure-open が green、しかし実体は splash hang」の vacuous green が構造的に
//! 発生した。本モジュールはその判定を純関数として置く (Issue #275 AC3: 判定は
//! ライブラリ層に純関数、CLI は薄くする)。
//!
//! splash-hang の実測シグネチャ (`.anaden-run/2026-11-04-live-e6e7/session/session-log.md`):
//! - frame MD5 不変: 5 captures over ~2.5 min すべて同一 (`2fad4aae...`)
//! - TCP ソケット 0 本 (`netstat -ano` 対象 PID に 0 行)
//!
//! 提供する API:
//! - [`frame_digest`]: キャプチャ画像 1 枚のフレームダイジェスト (変化検出用)
//! - [`classify_readiness`]: サンプル列からの [`ReadinessVerdict`] 判定
//!
//! I/O (キャプチャ・ソケット数計上) は `win32_readiness` (cfg(windows)) が担う。
//! 本モジュールは判定のみのため、実ゲームなしで単体テストできる (AC4)。

use std::hash::Hasher;

use image::DynamicImage;

/// フレームダイジェスト (newtype)。生ピクセルバイト列の SipHash-1-3 (64bit)。
///
/// 用途は同一性比較 (変化検出) のみで衝突耐性は不要なため、手動 probe の
/// PNG MD5 比較を依存追加なしで置き換える (Issue #275 は「MD5 または pixel diff」
/// を許容)。絶対値は md5sum とは一致しないが、変化検出の役割は同等である。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDigest(pub u64);

/// readiness サンプリング 1 回分の観測 (純データ)。
///
/// `digest` はその時点のキャプチャフレーム、`socket_count` は対象プロセス群が
/// 保持する TCP ソケット数 (v4+v6)。「frame 不変 + ソケット 0」の組が splash-hang
/// の判定入力になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadinessSample {
    /// キャプチャフレームのダイジェスト。
    pub digest: FrameDigest,
    /// 対象プロセス群の TCP ソケット数。
    pub socket_count: u32,
}

/// readiness 判定結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessVerdict {
    /// レンダ変化または TCP ソケット存在を観測 = 起動処理が進行中 (hang でない)。
    Active,
    /// 全サンプルでフレーム不変かつ TCP ソケット 0 = splash hang (not-ready)。
    SplashHang,
}

/// `DynamicImage` の生ピクセルバイトからフレームダイジェストを計算する。
///
/// 寸法 (幅・高さ) を先頭に混入する。これによりピクセルバイト長が同じ別レイアウト
/// (リサイズ等) も別ダイジェストとして分離される。
pub fn frame_digest(img: &DynamicImage) -> FrameDigest {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write_u32(img.width());
    hasher.write_u32(img.height());
    hasher.write(img.as_bytes());
    FrameDigest(hasher.finish())
}

/// サンプル列から readiness を判定する (純粋・決定論的・panic 無し)。
///
/// 判定契約 (Issue #275 AC1 — 手動 probe の実測基準をそのまま構造化):
///
/// - 全サンプルの `digest` が同一 **かつ** 全サンプルの `socket_count` が 0
///   → [`ReadinessVerdict::SplashHang`]
/// - それ以外 (digest 変化あり、またはいずれかのサンプルでソケット存在)
///   → [`ReadinessVerdict::Active`]
///
/// digest 変化はレンダ進行 (起動演出・ロード) を意味する。ソケット存在は
/// ネットワーク待ち (ダウンロード・ログイン) の可能性を意味し、frame 不変でも
/// hang とは断定しない。hang 断定は「frame 不変 + ソケット 0」の根拠が揃った
/// 場合のみ (fail-closed だが誤検出方向には保守的)。
///
/// サンプル数 2 未満の入力は「活性の証拠が無い」ため SplashHang 扱いになる
/// (fail-closed)。呼出側 (`win32_readiness::probe_readiness`) は必ず 2 以上の
/// サンプルを渡す。
pub fn classify_readiness(samples: &[ReadinessSample]) -> ReadinessVerdict {
    let Some(first) = samples.first() else {
        return ReadinessVerdict::SplashHang;
    };
    let render_frozen = samples.iter().all(|s| s.digest == first.digest);
    let all_offline = samples.iter().all(|s| s.socket_count == 0);
    if render_frozen && all_offline {
        ReadinessVerdict::SplashHang
    } else {
        ReadinessVerdict::Active
    }
}
