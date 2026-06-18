//! ゲーム(アナザーエデン)アプリの起動・前景判定。
//!
//! デバイス接続時にゲームが起動している保証はないため、`ensure_app_open` で
//! 「非前景なら起動 → 前景化までポーリング待機」を行う。実機依存部(ADB shell 実行)と
//! 純粋な文字列解析部(dumpsys 解析・コマンド構築・ポーリング論理)を分離し、
//! 後者はデバイス不要で単体テスト可能。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::client::{AdbClient, AdbError};

/// アナザーエデンのパッケージ名。
pub const GAME_PACKAGE: &str = "net.wrightflyer.anothereden";

/// アナザーエデンのメイン Activity(FQN)。
pub const GAME_ACTIVITY: &str = "net.wrightflyer.toybox.AppActivity";

/// ポーリング間隔(前景化の再確認周期)。
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// boxed async フューチャの型エイリアス(`ensure_app_open_with` の注入点で使用)。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `ensure_app_open` のポーリング結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// 既に前景だった(起動不要)。
    AlreadyOpen,
    /// 起動し、`max_wait` 以内に前景化した。
    Launched,
    /// 起動したが `max_wait` 経過でも前景化しなかった。タイムアウト。
    Timeout,
}

/// アプリの前景判定・起動を行う。
#[derive(Clone)]
pub struct AppController {
    client: AdbClient,
}

impl AppController {
    pub fn new(client: AdbClient) -> Self {
        Self { client }
    }

    /// `dumpsys activity activities` を実行し、現在前景のパッケージ名を返す。
    pub async fn foreground_package(&self) -> Result<Option<String>, AdbError> {
        let raw = self.client.shell("dumpsys activity activities").await?;
        let text = String::from_utf8_lossy(&raw);
        Ok(parse_foreground_package(&text))
    }

    /// ゲームが前景にあるか。
    pub async fn is_app_foreground(&self) -> Result<bool, AdbError> {
        Ok(self.foreground_package().await?.as_deref() == Some(GAME_PACKAGE))
    }

    /// ゲームを起動する(`am start -n <pkg>/<activity>`)。
    pub async fn launch_app(&self) -> Result<(), AdbError> {
        let cmd = build_launch_command(GAME_PACKAGE, GAME_ACTIVITY);
        info!("launch_app: {}", cmd);
        self.client.shell(&cmd).await?;
        Ok(())
    }

    /// ゲームが前景になければ起動し、前景化するまで `max_wait` の間ポーリング待機する。
    ///
    /// 戻り値で [`EnsureOutcome::AlreadyOpen`] / [`EnsureOutcome::Launched`] /
    /// [`EnsureOutcome::Timeout`] を区別する。ポーリング論理そのものは
    /// [`ensure_app_open_with`] に委譲し、同関数はフェイク時刻/状態でデバイス不要にテストされる。
    pub async fn ensure_app_open(&self, max_wait: Duration) -> Result<EnsureOutcome, AdbError> {
        // `ensure_app_open_with` は `'static` な boxed フューチャを要求するため、
        // self の clone を Arc で共有し、クロージャへ持ち出す。
        let me = Arc::new(self.clone());
        let me_clone = me.clone();
        ensure_app_open_with(
            move || {
                let m = me.clone();
                Box::pin(async move { m.is_app_foreground().await })
            },
            move || {
                let m = me_clone.clone();
                Box::pin(async move { m.launch_app().await })
            },
            max_wait,
            Instant::now,
            |d| Box::pin(tokio::time::sleep(d)),
        )
        .await
    }
}

/// `dumpsys activity activities` 出力から前景パッケージを抽出する純粋関数。
///
/// Android のバージョン・OEM 実装差を吸収するため、以下の指標を順に試す:
/// (1) `ResumedActivity` / `mResumedActivity` / `topResumedActivity` 行
/// (2) `mCurrentFocus` / `mFocusedApp` 行(window manager 系)
///
/// いずれも取得できなければ [`None`]。
pub fn parse_foreground_package(dumpsys: &str) -> Option<String> {
    // (1) ResumedActivity 系。
    for line in dumpsys.lines() {
        let trimmed = line.trim();
        let key_hits = trimmed.contains("ResumedActivity")
            || trimmed.contains("mResumedActivity")
            || trimmed.contains("topResumedActivity");
        if key_hits {
            if let Some(pkg) = extract_package_from_line(trimmed) {
                return Some(pkg);
            }
        }
    }

    // (2) window manager 系フォーカス行。
    for line in dumpsys.lines() {
        let trimmed = line.trim();
        let key_hits = trimmed.contains("mCurrentFocus") || trimmed.contains("mFocusedApp");
        if key_hits {
            if let Some(pkg) = extract_package_from_line(trimmed) {
                return Some(pkg);
            }
        }
    }

    None
}

/// 1行から `package/activity` 形式のパッケージ部分を取り出すヘルパ。
///
/// 対応パターン(いずれもスラッシュ区切りの左側がパッケージ):
/// - `cmp=net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity`
/// - `net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity`
/// - `component={net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity}`
///
/// パッケージ名は `[a-zA-Z0-9_.]+` を許容する(先頭のドットは不可)。
fn extract_package_from_line(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() {
            // 識別子の開始可能性。どこまで続くか確認し、直後が '/' なら採用。
            let mut j = i;
            while j < bytes.len() {
                let c = bytes[j];
                if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
                    j += 1;
                } else {
                    break;
                }
            }
            if j < bytes.len() && bytes[j] == b'/' {
                let candidate = &line[i..j];
                if is_valid_package(candidate) {
                    return Some(candidate.to_string());
                }
            }
            // この識別子は不採用。スラッシュの次から再開。
            i = j + 1;
        } else {
            i += 1;
        }
    }
    None
}

/// パッケージ名として妥当か(空でなく、英数字/._ のみ、先頭がドットでない)。
fn is_valid_package(pkg: &str) -> bool {
    if pkg.is_empty() {
        return false;
    }
    let first = pkg.as_bytes()[0];
    if first == b'.' {
        return false;
    }
    pkg.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

/// `am start -n <package>/<activity>` コマンド文字列を構築する純粋関数。
pub fn build_launch_command(package: &str, activity: &str) -> String {
    format!("am start -n {package}/{activity}")
}

/// ポーリング論理の本体。前景チェック・起動・時刻取得・待機を注入可能で、
/// デバイスなしでテストできる。
///
/// 契約:
/// - `check_foreground`: 現在前景なら `Ok(true)`。
/// - `launch`: ゲームを起動する(副作用)。
/// - `now`: 現在時刻。
/// - `sleep`: 指定 duration 待機する(async)。
///
/// 流れ:
/// 1. 最初の `check_foreground` で前景なら [`EnsureOutcome::AlreadyOpen`]。
/// 2. そうでなければ `launch` 1回。
/// 3. `max_wait` のデッドラインまで `POLL_INTERVAL` ごとに再チェックし、
///    前景化すれば [`EnsureOutcome::Launched`]、期限切れなら [`EnsureOutcome::Timeout`]。
pub async fn ensure_app_open_with<F, L, S, N>(
    mut check_foreground: F,
    launch: L,
    max_wait: Duration,
    now: N,
    sleep: S,
) -> Result<EnsureOutcome, AdbError>
where
    F: FnMut() -> BoxFuture<'static, Result<bool, AdbError>>,
    L: FnOnce() -> BoxFuture<'static, Result<(), AdbError>>,
    S: Fn(Duration) -> BoxFuture<'static, ()>,
    N: Fn() -> Instant + Send + 'static,
{
    // 既に前景なら即完了。
    if check_foreground().await? {
        return Ok(EnsureOutcome::AlreadyOpen);
    }

    info!(
        "game not foreground; launching {} / {}",
        GAME_PACKAGE, GAME_ACTIVITY
    );
    launch().await?;

    let deadline = now() + max_wait;
    loop {
        if check_foreground().await? {
            info!("game is now foreground");
            return Ok(EnsureOutcome::Launched);
        }
        if now() >= deadline {
            warn!(
                "game did not reach foreground within {:?} (timeout)",
                max_wait
            );
            return Ok(EnsureOutcome::Timeout);
        }
        sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // ---- parse_foreground_package ----

    #[test]
    fn parse_resumed_activity_block() {
        let dump = "\
ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)
  Display #0 (activities from the global root)
    ResumedActivity: ActivityRecord{2a3b u0 net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity t123}
    topResumedActivity=ActivityRecord{2a3b u0 net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity t123}
";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("net.wrightflyer.anothereden")
        );
    }

    #[test]
    fn parse_cmp_form() {
        let dump = "  mResumedActivity: ActivityRecord{abc u0 cmp=net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity}";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("net.wrightflyer.anothereden")
        );
    }

    #[test]
    fn parse_window_manager_focus_fallback() {
        // ResumedActivity 行無し → mCurrentFocus へフォールバック。
        let dump = "  mCurrentFocus=Window{abc u0 net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity}";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("net.wrightflyer.anothereden")
        );
    }

    #[test]
    fn parse_launcher_not_game() {
        let dump = "  ResumedActivity: ActivityRecord{1 u0 com.android.launcher3/com.android.launcher3.Launcher t1}";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("com.android.launcher3")
        );
    }

    #[test]
    fn parse_component_curly_brace() {
        let dump = "  ResumedActivity: ActivityRecord{x u0 component={net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity}}";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("net.wrightflyer.anothereden")
        );
    }

    #[test]
    fn parse_mfocused_app() {
        let dump = "  mFocusedApp=AppWindowToken{abc token=Token{net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity}}";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("net.wrightflyer.anothereden")
        );
    }

    #[test]
    fn parse_returns_none_for_empty_or_noise() {
        assert_eq!(parse_foreground_package(""), None);
        assert_eq!(
            parse_foreground_package("no useful lines here\njust noise"),
            None
        );
    }

    #[test]
    fn parse_strips_trailing_non_identifier() {
        // スラッシュ後の activity にタグが付いていてもパッケージ部は正しく抽出。
        let dump = "  ResumedActivity=ActivityRecord{x u0 net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity}";
        assert_eq!(
            parse_foreground_package(dump).as_deref(),
            Some("net.wrightflyer.anothereden")
        );
    }

    // ---- build_launch_command ----

    #[test]
    fn build_launch_command_format() {
        assert_eq!(
            build_launch_command(GAME_PACKAGE, GAME_ACTIVITY),
            "am start -n net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity"
        );
    }

    #[test]
    fn build_launch_command_arbitrary() {
        assert_eq!(
            build_launch_command("com.example.app", "com.example.app.MainActivity"),
            "am start -n com.example.app/com.example.app.MainActivity"
        );
    }

    // ---- constants ----

    #[test]
    fn constants_match_existing_launch_subcommand() {
        // 既存 template_tool launch サブコマンドと同一の component 指定。
        assert_eq!(GAME_PACKAGE, "net.wrightflyer.anothereden");
        assert_eq!(GAME_ACTIVITY, "net.wrightflyer.toybox.AppActivity");
    }

    // ---- ensure_app_open_with (polling logic, no device) ----

    #[tokio::test]
    async fn ensure_already_open_skips_launch() {
        let launches = Arc::new(AtomicUsize::new(0));
        let launches_clone = launches.clone();

        let outcome = ensure_app_open_with(
            || Box::pin(async { Ok(true) }),
            move || {
                let l = launches_clone.clone();
                Box::pin(async move {
                    l.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
            Duration::from_secs(10),
            Instant::now,
            |_| Box::pin(async {}),
        )
        .await
        .unwrap();

        assert_eq!(outcome, EnsureOutcome::AlreadyOpen);
        assert_eq!(
            launches.load(Ordering::SeqCst),
            0,
            "must not launch when already open"
        );
    }

    #[tokio::test]
    async fn ensure_launches_then_foreground_after_polls() {
        // 1回目の check(AlreadyOpen 判定) = false → launch →
        // 2回目 = false → 3回目 = true で Launched。
        let states = Arc::new(Mutex::new(vec![false, false, true]));
        let states_clone = states.clone();

        let outcome = ensure_app_open_with(
            move || {
                let s = states_clone.clone();
                Box::pin(async move {
                    let mut guard = s.lock().unwrap();
                    if let Some(v) = guard.first().copied() {
                        if guard.len() > 1 {
                            guard.remove(0);
                        }
                        Ok(v)
                    } else {
                        Ok(true)
                    }
                })
            },
            || Box::pin(async { Ok(()) }),
            Duration::from_secs(60),
            Instant::now,
            |_| Box::pin(async {}),
        )
        .await
        .unwrap();

        assert_eq!(outcome, EnsureOutcome::Launched);
    }

    #[tokio::test]
    async fn ensure_timeout_when_never_foreground() {
        // 常に false → max_wait 経過で Timeout。制御可能な now() を用意:
        // 呼ばれるたびに 3 秒進む時計(max_wait=10s → 数回で期限到達)。
        let secs = Arc::new(AtomicU64::new(0));
        let secs_clone = secs.clone();
        let now = move || {
            let s = secs_clone.fetch_add(3, Ordering::SeqCst);
            Instant::now() + Duration::from_secs(s)
        };

        let outcome = ensure_app_open_with(
            || Box::pin(async { Ok(false) }),
            || Box::pin(async { Ok(()) }),
            Duration::from_secs(10),
            now,
            |_| Box::pin(async {}),
        )
        .await
        .unwrap();

        assert_eq!(outcome, EnsureOutcome::Timeout);
    }

    #[tokio::test]
    async fn ensure_propagates_check_error() {
        let outcome: Result<EnsureOutcome, AdbError> = ensure_app_open_with(
            || {
                Box::pin(async {
                    Err(AdbError::CommandFailed {
                        message: "dumpsys failed".into(),
                    })
                })
            },
            || Box::pin(async { Ok(()) }),
            Duration::from_secs(10),
            Instant::now,
            |_| Box::pin(async {}),
        )
        .await;
        assert!(outcome.is_err());
    }

    #[tokio::test]
    async fn ensure_launch_called_exactly_once() {
        // 起動は最大1回。複数ポーリング後の前景化でも2回目の launch は無い。
        let launches = Arc::new(AtomicUsize::new(0));
        let launches_clone = launches.clone();

        let states = Arc::new(Mutex::new(vec![false, true]));
        let states_clone = states.clone();

        let outcome = ensure_app_open_with(
            move || {
                let s = states_clone.clone();
                Box::pin(async move {
                    let mut guard = s.lock().unwrap();
                    if let Some(v) = guard.first().copied() {
                        if guard.len() > 1 {
                            guard.remove(0);
                        }
                        Ok(v)
                    } else {
                        Ok(true)
                    }
                })
            },
            move || {
                let l = launches_clone.clone();
                Box::pin(async move {
                    l.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            },
            Duration::from_secs(60),
            Instant::now,
            |_| Box::pin(async {}),
        )
        .await
        .unwrap();

        assert_eq!(outcome, EnsureOutcome::Launched);
        assert_eq!(launches.load(Ordering::SeqCst), 1);
    }
}
