
param()

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$projectDir = Split-Path -Parent (Split-Path -Parent $scriptDir)

$debugLog = Join-Path $projectDir ".claude\hook-debug.log"
$MODEL = "claude-sonnet-4-20250514"
$API_KEY = $env:ANTHROPIC_API_KEY

function Write-DebugLog($msg) {
    try {
        $ts = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
        Add-Content -Path $debugLog -Value "[$ts] $msg" -Encoding UTF8
    } catch {}
}

try {
    $inputJson = $null
    try {
        $sr = [System.IO.StreamReader]::new([Console]::OpenStandardInput())
        $inputJson = $sr.ReadToEnd()
        $sr.Dispose()
    } catch {
        Write-DebugLog "update-claude-md: stdin unavailable: $_"
        exit 0
    }

    if ([string]::IsNullOrWhiteSpace($inputJson)) {
        Write-DebugLog "update-claude-md: stdin empty"
        exit 0
    }

    $input_data = $null
    try {
        $input_data = $inputJson | ConvertFrom-Json -ErrorAction Stop
    } catch {
        Write-DebugLog "update-claude-md: JSON parse error: $_"
        exit 0
    }

    $targetDir = $env:CLAUDE_PROJECT_DIR
    if (-not $targetDir) { $targetDir = $input_data.cwd }
    if (-not $targetDir) { $targetDir = $projectDir }
    if (-not $targetDir) {
        Write-DebugLog "update-claude-md: no project dir"
        exit 0
    }

    $claudeDir = Join-Path $targetDir ".claude"
    $logFile   = Join-Path $claudeDir "context-log.jsonl"
    $claudeMd  = Join-Path $targetDir "CLAUDE.md"

    if (-not (Test-Path $logFile)) {
        Write-DebugLog "update-claude-md: no context log"
        exit 0
    }

    $sessionId = $input_data.session_id
    $entries = Get-Content $logFile -Encoding UTF8 |
        ForEach-Object { $_ | ConvertFrom-Json } |
        Where-Object { $_.session_id -eq $sessionId }

    if (-not $entries) {
        Write-DebugLog "update-claude-md: no entries for session $sessionId"
        exit 0
    }

    $messages = $entries |
        Where-Object { $_.last_message } |
        Select-Object -ExpandProperty last_message |
        Select-Object -Unique

    if (-not $messages) {
        Write-DebugLog "update-claude-md: no messages captured"
        exit 0
    }

    $existingContent = ""
    if (Test-Path $claudeMd) {
        $existingContent = Get-Content $claudeMd -Raw -Encoding UTF8
    }

    if (-not $API_KEY) {
        Write-DebugLog "update-claude-md: no API key, raw append"
        Append-RawLearnings -claudeMd $claudeMd -messages $messages -existingContent $existingContent
        exit 0
    }

    $messagesText = $messages -join "`n`n---`n`n"
    $prompt = @"
以下は開発セッション中にClaudeが生成した応答の抜粋です。
再利用できるノウハウ・注意点・ベストプラクティスを簡潔な箇条書きで抽出してください。

条件:
- 具体的で実用的な内容だけ残す
- 曖昧・一般論は除外
- 最大10項目
- 各項目は「- 」で始め、1〜2文で完結させる
- セクションヘッダーや前置き文は出力しない

--- セッション応答 ---
$messagesText
"@

    $body = @{
        model      = $MODEL
        max_tokens = 1024
        messages   = @(@{ role = "user"; content = $prompt })
    } | ConvertTo-Json -Depth 5

    $headers = @{
        "x-api-key"         = $API_KEY
        "anthropic-version" = "2023-06-01"
        "content-type"      = "application/json"
    }

    $response = Invoke-RestMethod `
        -Uri "https://api.anthropic.com/v1/messages" `
        -Method POST `
        -Headers $headers `
        -Body $body `
        -TimeoutSec 30 `
        -ErrorAction Stop

    $learnings = $response.content[0].text.Trim()

    if (-not $learnings) {
        Write-DebugLog "update-claude-md: no learnings extracted"
        exit 0
    }

    $date = Get-Date -Format "yyyy-MM-dd HH:mm"
    $newSection = "`n## Session Learnings ($date)`n`n$learnings`n"

    $sectionPattern = "(?s)## Session Learnings \([\d\-: ]+\).*?(?=\n## |\z)"

    if ($existingContent -match $sectionPattern) {
        $updatedContent = [regex]::Replace($existingContent, $sectionPattern, { param($m) $newSection.TrimStart() })
    } else {
        $updatedContent = $existingContent.TrimEnd() + $newSection
    }

    Set-Content -Path $claudeMd -Value $updatedContent -Encoding UTF8 -NoNewline

    Write-DebugLog "update-claude-md: updated with learnings"
    exit 0
}
catch {
    Write-DebugLog "update-claude-md FATAL: $_"
    exit 0
}

function Append-RawLearnings {
    param($claudeMd, $messages, $existingContent)
    $date = Get-Date -Format "yyyy-MM-dd HH:mm"
    $items = $messages | ForEach-Object { "- $_" }
    $section = "`n## Session Learnings ($date) [raw]`n" + ($items -join "`n") + "`n"
    $updated = $existingContent.TrimEnd() + $section
    Set-Content -Path $claudeMd -Value $updated -Encoding UTF8 -NoNewline
}
